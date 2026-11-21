use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use maxi_loop_platform::alert_notify::{AlertSignal, AlertSignalObserver};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::begin_worker_tx;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, warn};

use crate::delivery::{AlertPayload, DeliveryPolicy, dispatch_alert};
use crate::model::AlertChannelType;

/// Non-blocking handle implementing `AlertSignalObserver` for `crates/ingest`.
#[derive(Clone)]
pub struct AlertSignalSender {
    tx: mpsc::Sender<AlertSignal>,
}

impl AlertSignalSender {
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<AlertSignal>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (Self { tx }, rx)
    }
}

impl AlertSignalObserver for AlertSignalSender {
    fn on_alert_signal(&self, signal: AlertSignal) {
        if let Err(err) = self.tx.try_send(signal) {
            warn!(error = %err, "alert signal channel full or closed; dropping real-time signal");
        }
    }
}

#[derive(FromRow)]
struct MatchingRuleRow {
    rule_id: i64,
    channel_id: i64,
    channel_type: String,
    channel_config: Value,
    environment_filter: Option<String>,
    cooldown_minutes: i32,
}

#[derive(FromRow)]
struct SpikeRuleRow {
    rule_id: i64,
    tenant_id: i64,
    workspace_id: Option<i64>,
    project_id: Option<i64>,
    channel_id: i64,
    channel_type: String,
    channel_config: Value,
    environment_filter: Option<String>,
    spike_threshold: i32,
    spike_window_minutes: i32,
    cooldown_minutes: i32,
}

#[derive(FromRow)]
struct SpikeCandidateRow {
    project_id: i64,
    project_slug: String,
    issue_id: i64,
    title: String,
    exception_type: String,
    exception_value: String,
    culprit: String,
    level: String,
    last_environment: String,
    last_release: Option<String>,
    total_event_count: i64,
    window_event_count: i64,
}

/// Spawn the background alert engine handling:
/// 1. Real-time `new_issue` and `regression` signals emitted by the ingest worker (< 1s latency).
/// 2. Periodic `spike` rule evaluation every `spike_interval` (default 60s).
pub fn spawn_alert_engine(
    pool: PgPool,
    public_base_url: String,
    mut rx: mpsc::Receiver<AlertSignal>,
    policy: DeliveryPolicy,
    spike_interval: Duration,
) -> JoinHandle<()> {
    let interval = spike_interval.max(Duration::from_secs(5));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume immediate first tick so startup does not rush spike checks.
        ticker.tick().await;

        loop {
            tokio::select! {
                maybe_signal = rx.recv() => {
                    match maybe_signal {
                        Some(signal) => {
                            if let Err(err) = evaluate_alert_signal(
                                &pool,
                                &public_base_url,
                                &signal,
                                &policy,
                            ).await {
                                error!(
                                    tenant_id = signal.tenant_id,
                                    project_id = signal.project_id,
                                    issue_id = signal.issue_id,
                                    error = %err,
                                    "failed evaluating real-time alert signal"
                                );
                            }
                        }
                        None => break,
                    }
                }
                _ = ticker.tick() => {
                    if let Err(err) = evaluate_spike_rules(&pool, &public_base_url, &policy).await {
                        error!(error = %err, "failed evaluating spike alert rules");
                    }
                }
            }
        }
    })
}

/// Evaluate all matching enabled inherited rules (`new_issue` or `regression`) for a single signal,
/// honoring `environment_filter` and `cooldown_minutes`, dispatching via Webhook/Telegram/Email,
/// and recording each attempt in `alert_deliveries`.
pub async fn evaluate_alert_signal(
    pool: &PgPool,
    public_base_url: &str,
    signal: &AlertSignal,
    policy: &DeliveryPolicy,
) -> Result<usize, AppError> {
    let trigger_str = signal.kind.as_str();
    let mut tx = begin_worker_tx(pool).await?;

    let project_slug: Option<String> =
        sqlx::query_scalar("SELECT slug FROM projects WHERE tenant_id = $1 AND id = $2")
            .bind(signal.tenant_id)
            .bind(signal.project_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("lookup project slug for alert: {err}"))
            })?;

    let Some(project_slug) = project_slug else {
        tx.commit().await.ok();
        return Ok(0);
    };

    let rules: Vec<MatchingRuleRow> = sqlx::query_as(
        "SELECT r.id AS rule_id, r.channel_id, c.channel_type, c.config AS channel_config,
                r.environment_filter, r.cooldown_minutes
         FROM alert_rules r
         JOIN alert_channels c ON c.tenant_id = r.tenant_id AND c.id = r.channel_id
         WHERE r.tenant_id = $1
           AND r.trigger_type = $2
           AND r.is_enabled = TRUE
           AND c.is_enabled = TRUE
           AND (r.workspace_id IS NULL OR r.workspace_id = $3)
           AND (r.project_id IS NULL OR r.project_id = $4)
         ORDER BY r.id ASC",
    )
    .bind(signal.tenant_id)
    .bind(trigger_str)
    .bind(signal.workspace_id)
    .bind(signal.project_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load matching alert rules: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit alert rule lookup: {err}")))?;

    let mut dispatched = 0usize;
    let issue_url = format!(
        "{}/?issue={}",
        public_base_url.trim_end_matches('/'),
        signal.issue_id
    );

    for rule in rules {
        if let Some(env_filter) = rule.environment_filter.as_deref()
            && !env_filter.eq_ignore_ascii_case(&signal.environment)
        {
            continue;
        }

        if is_within_cooldown(
            pool,
            signal.tenant_id,
            signal.project_id,
            rule.rule_id,
            signal.issue_id,
            rule.cooldown_minutes,
        )
        .await?
        {
            continue;
        }

        let Ok(channel_type) = AlertChannelType::parse(&rule.channel_type) else {
            continue;
        };

        let payload = AlertPayload {
            event: trigger_str.to_owned(),
            tenant_id: signal.tenant_id,
            project_id: signal.project_id,
            project_slug: project_slug.clone(),
            issue_id: signal.issue_id,
            title: signal.title.clone(),
            exception_type: signal.exception_type.clone(),
            exception_value: signal.exception_value.clone(),
            culprit: signal.culprit.clone(),
            level: signal.level.clone(),
            environment: signal.environment.clone(),
            release: signal.release.clone(),
            event_count: signal.event_count,
            issue_url: issue_url.clone(),
            triggered_at: Utc::now(),
            details: None,
        };

        let outcome = dispatch_alert(channel_type, &rule.channel_config, &payload, policy).await;
        record_delivery(
            pool,
            signal.tenant_id,
            signal.project_id,
            rule.rule_id,
            rule.channel_id,
            signal.issue_id,
            trigger_str,
            channel_type.as_str(),
            if outcome.ok { "sent" } else { "failed" },
            outcome.http_status,
            outcome.error_message.as_deref(),
        )
        .await?;
        dispatched += 1;
    }

    Ok(dispatched)
}

/// Evaluate all enabled `spike` alert rules across tenants using `issue_hourly_counters`
/// (with `events` fallback for sub-hour windows) and dispatch notifications when threshold is met.
pub async fn evaluate_spike_rules(
    pool: &PgPool,
    public_base_url: &str,
    policy: &DeliveryPolicy,
) -> Result<usize, AppError> {
    let mut tx = begin_worker_tx(pool).await?;
    let rules: Vec<SpikeRuleRow> = sqlx::query_as(
        "SELECT r.id AS rule_id, r.tenant_id, r.workspace_id, r.project_id,
                r.channel_id, c.channel_type, c.config AS channel_config,
                r.environment_filter, r.spike_threshold, r.spike_window_minutes,
                r.cooldown_minutes
         FROM alert_rules r
         JOIN alert_channels c ON c.tenant_id = r.tenant_id AND c.id = r.channel_id
         WHERE r.trigger_type = 'spike'
           AND r.is_enabled = TRUE
           AND c.is_enabled = TRUE
           AND r.spike_threshold IS NOT NULL
           AND r.spike_window_minutes IS NOT NULL
         ORDER BY r.tenant_id ASC, r.id ASC
         LIMIT 500",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load spike rules: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit load spike rules: {err}")))?;

    let mut dispatched = 0usize;
    for rule in rules {
        let window_start =
            Utc::now() - ChronoDuration::minutes(i64::from(rule.spike_window_minutes));
        let hour_bucket_start = truncate_to_hour(window_start);

        let mut tx = begin_worker_tx(pool).await?;
        let candidates: Vec<SpikeCandidateRow> = sqlx::query_as(
            "SELECT i.project_id, p.slug AS project_slug, i.id AS issue_id,
                    i.title, i.exception_type, i.exception_value, i.culprit,
                    i.level, i.last_environment, i.last_release,
                    i.event_count AS total_event_count,
                    COALESCE(SUM(h.event_count), 0)::BIGINT AS window_event_count
             FROM issues i
             JOIN projects p ON p.tenant_id = i.tenant_id AND p.id = i.project_id
             JOIN issue_hourly_counters h
               ON h.tenant_id = i.tenant_id AND h.project_id = i.project_id AND h.issue_id = i.id
             WHERE i.tenant_id = $1
               AND i.status = 'unresolved'
               AND h.hour_bucket >= $2
               AND ($3::BIGINT IS NULL OR p.workspace_id = $3)
               AND ($4::BIGINT IS NULL OR i.project_id = $4)
               AND ($5::TEXT IS NULL OR LOWER(i.last_environment) = LOWER($5))
             GROUP BY i.project_id, p.slug, i.id, i.title, i.exception_type, i.exception_value,
                      i.culprit, i.level, i.last_environment, i.last_release, i.event_count
             HAVING COALESCE(SUM(h.event_count), 0) >= $6
             ORDER BY window_event_count DESC
             LIMIT 50",
        )
        .bind(rule.tenant_id)
        .bind(hour_bucket_start)
        .bind(rule.workspace_id)
        .bind(rule.project_id)
        .bind(&rule.environment_filter)
        .bind(i64::from(rule.spike_threshold))
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("query spike candidates: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("commit spike candidates: {err}")))?;

        let Ok(channel_type) = AlertChannelType::parse(&rule.channel_type) else {
            continue;
        };

        for cand in candidates {
            if is_within_cooldown(
                pool,
                rule.tenant_id,
                cand.project_id,
                rule.rule_id,
                cand.issue_id,
                rule.cooldown_minutes,
            )
            .await?
            {
                continue;
            }

            let issue_url = format!(
                "{}/?issue={}",
                public_base_url.trim_end_matches('/'),
                cand.issue_id
            );
            let payload = AlertPayload {
                event: "spike".to_owned(),
                tenant_id: rule.tenant_id,
                project_id: cand.project_id,
                project_slug: cand.project_slug,
                issue_id: cand.issue_id,
                title: cand.title,
                exception_type: cand.exception_type,
                exception_value: cand.exception_value,
                culprit: cand.culprit,
                level: cand.level,
                environment: cand.last_environment,
                release: cand.last_release,
                event_count: cand.total_event_count,
                issue_url,
                triggered_at: Utc::now(),
                details: Some(format!(
                    "{} events observed (threshold {} in {}m window)",
                    cand.window_event_count, rule.spike_threshold, rule.spike_window_minutes
                )),
            };

            let outcome =
                dispatch_alert(channel_type, &rule.channel_config, &payload, policy).await;
            record_delivery(
                pool,
                rule.tenant_id,
                cand.project_id,
                rule.rule_id,
                rule.channel_id,
                cand.issue_id,
                "spike",
                channel_type.as_str(),
                if outcome.ok { "sent" } else { "failed" },
                outcome.http_status,
                outcome.error_message.as_deref(),
            )
            .await?;
            dispatched += 1;
        }
    }

    Ok(dispatched)
}

async fn is_within_cooldown(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    rule_id: i64,
    issue_id: i64,
    cooldown_minutes: i32,
) -> Result<bool, AppError> {
    let cutoff = Utc::now() - ChronoDuration::minutes(i64::from(cooldown_minutes.max(1)));
    let mut tx = begin_worker_tx(pool).await?;
    let recent: Option<i64> = sqlx::query_scalar(
        "SELECT id
         FROM alert_deliveries
         WHERE tenant_id = $1
           AND project_id = $2
           AND rule_id = $3
           AND issue_id = $4
           AND status = 'sent'
           AND created_at >= $5
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(rule_id)
    .bind(issue_id)
    .bind(cutoff)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("check alert cooldown: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit alert cooldown: {err}")))?;
    Ok(recent.is_some())
}

#[allow(clippy::too_many_arguments)]
async fn record_delivery(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    rule_id: i64,
    channel_id: i64,
    issue_id: i64,
    trigger_type: &str,
    channel_type: &str,
    status: &str,
    http_status: Option<i32>,
    error_message: Option<&str>,
) -> Result<(), AppError> {
    let mut tx = begin_worker_tx(pool).await?;
    sqlx::query(
        "INSERT INTO alert_deliveries (
             tenant_id, project_id, rule_id, channel_id, issue_id,
             trigger_type, channel_type, status, http_status, error_message
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(rule_id)
    .bind(channel_id)
    .bind(issue_id)
    .bind(trigger_type)
    .bind(channel_type)
    .bind(status)
    .bind(http_status)
    .bind(error_message)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("record alert_delivery: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit alert_delivery: {err}")))?;
    Ok(())
}

fn truncate_to_hour(ts: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    let secs = ts.timestamp();
    let hour_secs = secs - secs.rem_euclid(3600);
    chrono::DateTime::<Utc>::from_timestamp(hour_secs, 0).unwrap_or(ts)
}
