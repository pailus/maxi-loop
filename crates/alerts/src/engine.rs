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

use crate::delivery::{AlertPayload, DeliveryPolicy, PerformanceAlertContext, dispatch_alert};
use crate::model::{AlertChannelType, AlertTriggerType, SpikeMetric};
use maxi_loop_performance::histogram::estimate_percentile;

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
    spike_metric: String,
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
                    if let Err(err) =
                        evaluate_performance_rules(&pool, &public_base_url, &policy).await
                    {
                        error!(error = %err, "failed evaluating performance alert rules");
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
            issue_id: Some(signal.issue_id),
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
            performance: None,
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
                r.spike_metric, r.cooldown_minutes
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

        let metric = SpikeMetric::parse(&rule.spike_metric).unwrap_or(SpikeMetric::Events);
        let mut tx = begin_worker_tx(pool).await?;
        let candidates: Vec<SpikeCandidateRow> = match metric {
            SpikeMetric::Events => sqlx::query_as(
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
        .await,
            // Distinct affected users whose latest occurrence falls inside the window (`F018`).
            SpikeMetric::Users => sqlx::query_as(
                "SELECT i.project_id, p.slug AS project_slug, i.id AS issue_id,
                        i.title, i.exception_type, i.exception_value, i.culprit,
                        i.level, i.last_environment, i.last_release,
                        i.event_count AS total_event_count,
                        COUNT(u.user_hash)::BIGINT AS window_event_count
                 FROM issues i
                 JOIN projects p ON p.tenant_id = i.tenant_id AND p.id = i.project_id
                 JOIN issue_users u
                   ON u.tenant_id = i.tenant_id AND u.project_id = i.project_id AND u.issue_id = i.id
                 WHERE i.tenant_id = $1
                   AND i.status = 'unresolved'
                   AND u.last_seen >= $2
                   AND ($3::BIGINT IS NULL OR p.workspace_id = $3)
                   AND ($4::BIGINT IS NULL OR i.project_id = $4)
                   AND ($5::TEXT IS NULL OR LOWER(i.last_environment) = LOWER($5))
                 GROUP BY i.project_id, p.slug, i.id, i.title, i.exception_type, i.exception_value,
                          i.culprit, i.level, i.last_environment, i.last_release, i.event_count
                 HAVING COUNT(u.user_hash) >= $6
                 ORDER BY window_event_count DESC
                 LIMIT 50",
            )
            .bind(rule.tenant_id)
            .bind(window_start)
            .bind(rule.workspace_id)
            .bind(rule.project_id)
            .bind(&rule.environment_filter)
            .bind(i64::from(rule.spike_threshold))
            .fetch_all(&mut *tx)
            .await,
        }
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
                issue_id: Some(cand.issue_id),
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
                    "{} {} observed (threshold {} in {}m window)",
                    cand.window_event_count,
                    match metric {
                        SpikeMetric::Events => "events",
                        SpikeMetric::Users => "affected users",
                    },
                    rule.spike_threshold,
                    rule.spike_window_minutes
                )),
                performance: None,
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

#[derive(FromRow)]
struct PerfRuleRow {
    rule_id: i64,
    tenant_id: i64,
    workspace_id: Option<i64>,
    project_id: Option<i64>,
    channel_id: i64,
    channel_type: String,
    channel_config: Value,
    environment_filter: Option<String>,
    trigger_type: String,
    transaction_name: Option<String>,
    perf_threshold: f64,
    perf_window_minutes: i32,
    perf_min_count: i32,
    cooldown_minutes: i32,
}

#[derive(FromRow)]
struct PerfWindowRow {
    project_id: i64,
    project_slug: String,
    total_count: i64,
    error_count: i64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
}

/// Metric value of a performance window (`F019`): p95 in ms or error rate in percent.
pub fn performance_metric_value(
    trigger: AlertTriggerType,
    total_count: i64,
    error_count: i64,
    histogram: &[i64],
    min_ms: f64,
    max_ms: f64,
) -> Option<f64> {
    if total_count <= 0 {
        return None;
    }
    match trigger {
        AlertTriggerType::PerfP95 => Some(estimate_percentile(histogram, min_ms, max_ms, 0.95)),
        AlertTriggerType::PerfErrorRate => {
            Some(((error_count as f64) * 10_000.0 / (total_count as f64)).round() / 100.0)
        }
        _ => None,
    }
}

/// Evaluate enabled `perf_p95` / `perf_error_rate` rules across tenants from
/// `transaction_minute_metrics` (every tick, default 60s). A rule fires per project in its scope
/// whose window value is strictly above `perf_threshold` with at least `perf_min_count`
/// transactions; repeats are suppressed by `cooldown_minutes` per (rule, project, transaction).
pub async fn evaluate_performance_rules(
    pool: &PgPool,
    public_base_url: &str,
    policy: &DeliveryPolicy,
) -> Result<usize, AppError> {
    let mut tx = begin_worker_tx(pool).await?;
    let rules: Vec<PerfRuleRow> = sqlx::query_as(
        "SELECT r.id AS rule_id, r.tenant_id, r.workspace_id, r.project_id,
                r.channel_id, c.channel_type, c.config AS channel_config,
                r.environment_filter, r.trigger_type, r.transaction_name,
                r.perf_threshold, r.perf_window_minutes, r.perf_min_count, r.cooldown_minutes
         FROM alert_rules r
         JOIN alert_channels c ON c.tenant_id = r.tenant_id AND c.id = r.channel_id
         WHERE r.trigger_type IN ('perf_p95', 'perf_error_rate')
           AND r.is_enabled = TRUE
           AND c.is_enabled = TRUE
           AND r.perf_threshold IS NOT NULL
           AND r.perf_window_minutes IS NOT NULL
         ORDER BY r.tenant_id ASC, r.id ASC
         LIMIT 500",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load performance rules: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit load performance rules: {err}")))?;

    let mut dispatched = 0usize;
    for rule in rules {
        let Ok(trigger) = AlertTriggerType::parse(&rule.trigger_type) else {
            continue;
        };
        let Ok(channel_type) = AlertChannelType::parse(&rule.channel_type) else {
            continue;
        };
        let window_start = truncate_to_minute(
            Utc::now() - ChronoDuration::minutes(i64::from(rule.perf_window_minutes.max(1))),
        );

        let mut tx = begin_worker_tx(pool).await?;
        let windows: Vec<PerfWindowRow> = sqlx::query_as(
            "SELECT m.project_id, p.slug AS project_slug,
                    SUM(m.total_count)::BIGINT AS total_count,
                    SUM(m.error_count)::BIGINT AS error_count,
                    MIN(m.duration_min_ms) AS duration_min_ms,
                    MAX(m.duration_max_ms) AS duration_max_ms,
                    sum_histogram_buckets(m.histogram_buckets) AS histogram_buckets
             FROM transaction_minute_metrics m
             JOIN projects p ON p.tenant_id = m.tenant_id AND p.id = m.project_id
             WHERE m.tenant_id = $1
               AND m.minute_bucket >= $2
               AND ($3::BIGINT IS NULL OR p.workspace_id = $3)
               AND ($4::BIGINT IS NULL OR m.project_id = $4)
               AND ($5::TEXT IS NULL OR LOWER(m.environment) = LOWER($5))
               AND ($6::TEXT IS NULL OR m.transaction_name = $6)
             GROUP BY m.project_id, p.slug
             HAVING SUM(m.total_count) >= $7
             ORDER BY m.project_id ASC
             LIMIT 200",
        )
        .bind(rule.tenant_id)
        .bind(window_start)
        .bind(rule.workspace_id)
        .bind(rule.project_id)
        .bind(&rule.environment_filter)
        .bind(&rule.transaction_name)
        .bind(i64::from(rule.perf_min_count.max(1)))
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("query performance window: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("commit performance window: {err}")))?;

        for win in windows {
            let Some(value) = performance_metric_value(
                trigger,
                win.total_count,
                win.error_count,
                &win.histogram_buckets,
                win.duration_min_ms,
                win.duration_max_ms,
            ) else {
                continue;
            };
            if value <= rule.perf_threshold {
                continue;
            }
            if is_within_perf_cooldown(
                pool,
                rule.tenant_id,
                win.project_id,
                rule.rule_id,
                rule.transaction_name.as_deref(),
                rule.cooldown_minutes,
            )
            .await?
            {
                continue;
            }

            let scope_label = rule
                .transaction_name
                .clone()
                .unwrap_or_else(|| "all transactions".to_owned());
            let (metric, title) = match trigger {
                AlertTriggerType::PerfP95 => (
                    "p95_ms",
                    format!(
                        "p95 {value:.0} ms > {:.0} ms on {scope_label}",
                        rule.perf_threshold
                    ),
                ),
                _ => (
                    "error_rate_pct",
                    format!(
                        "Error rate {value:.2}% > {}% on {scope_label}",
                        rule.perf_threshold
                    ),
                ),
            };
            let title = truncate_chars(&title, 500);
            let url = format!(
                "{}/?view=performance&project={}",
                public_base_url.trim_end_matches('/'),
                url::form_urlencoded::byte_serialize(win.project_slug.as_bytes())
                    .collect::<String>()
            );
            let payload = AlertPayload {
                event: trigger.as_str().to_owned(),
                tenant_id: rule.tenant_id,
                project_id: win.project_id,
                project_slug: win.project_slug,
                issue_id: None,
                title: title.clone(),
                exception_type: String::new(),
                exception_value: String::new(),
                culprit: rule.transaction_name.clone().unwrap_or_default(),
                level: "warning".to_owned(),
                environment: rule
                    .environment_filter
                    .clone()
                    .unwrap_or_else(|| "all".to_owned()),
                release: None,
                event_count: win.total_count,
                issue_url: url,
                triggered_at: Utc::now(),
                details: Some(format!(
                    "{} transactions ({} errors) in the last {}m",
                    win.total_count, win.error_count, rule.perf_window_minutes
                )),
                performance: Some(PerformanceAlertContext {
                    metric: metric.to_owned(),
                    value,
                    threshold: rule.perf_threshold,
                    window_minutes: rule.perf_window_minutes,
                    transaction: rule.transaction_name.clone(),
                    transaction_count: win.total_count,
                }),
            };

            let outcome =
                dispatch_alert(channel_type, &rule.channel_config, &payload, policy).await;
            record_perf_delivery(
                pool,
                PerfDelivery {
                    tenant_id: rule.tenant_id,
                    project_id: win.project_id,
                    rule_id: rule.rule_id,
                    channel_id: rule.channel_id,
                    trigger_type: trigger.as_str(),
                    channel_type: channel_type.as_str(),
                    status: if outcome.ok { "sent" } else { "failed" },
                    http_status: outcome.http_status,
                    error_message: outcome.error_message.as_deref(),
                    transaction_name: rule.transaction_name.as_deref(),
                    metric_value: value,
                    subject: &title,
                },
            )
            .await?;
            dispatched += 1;
        }
    }
    Ok(dispatched)
}

async fn is_within_perf_cooldown(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    rule_id: i64,
    transaction_name: Option<&str>,
    cooldown_minutes: i32,
) -> Result<bool, AppError> {
    let cutoff = Utc::now() - ChronoDuration::minutes(i64::from(cooldown_minutes.max(1)));
    let mut tx = begin_worker_tx(pool).await?;
    let recent: Option<i64> = sqlx::query_scalar(
        "SELECT id
         FROM alert_deliveries
         WHERE tenant_id = $1
           AND rule_id = $2
           AND project_id = $3
           AND issue_id IS NULL
           AND transaction_name IS NOT DISTINCT FROM $4
           AND status = 'sent'
           AND created_at >= $5
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .bind(project_id)
    .bind(transaction_name)
    .bind(cutoff)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("check performance cooldown: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit performance cooldown: {err}")))?;
    Ok(recent.is_some())
}

struct PerfDelivery<'a> {
    tenant_id: i64,
    project_id: i64,
    rule_id: i64,
    channel_id: i64,
    trigger_type: &'a str,
    channel_type: &'a str,
    status: &'a str,
    http_status: Option<i32>,
    error_message: Option<&'a str>,
    transaction_name: Option<&'a str>,
    metric_value: f64,
    subject: &'a str,
}

async fn record_perf_delivery(pool: &PgPool, d: PerfDelivery<'_>) -> Result<(), AppError> {
    let mut tx = begin_worker_tx(pool).await?;
    sqlx::query(
        "INSERT INTO alert_deliveries (
             tenant_id, project_id, rule_id, channel_id, issue_id,
             trigger_type, channel_type, status, http_status, error_message,
             transaction_name, metric_value, subject
         ) VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(d.tenant_id)
    .bind(d.project_id)
    .bind(d.rule_id)
    .bind(d.channel_id)
    .bind(d.trigger_type)
    .bind(d.channel_type)
    .bind(d.status)
    .bind(d.http_status)
    .bind(d.error_message)
    .bind(d.transaction_name)
    .bind(d.metric_value)
    .bind(d.subject)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("record performance delivery: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit performance delivery: {err}")))?;
    Ok(())
}

fn truncate_to_minute(ts: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    let secs = ts.timestamp();
    chrono::DateTime::<Utc>::from_timestamp(secs - secs.rem_euclid(60), 0).unwrap_or(ts)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}

fn truncate_to_hour(ts: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    let secs = ts.timestamp();
    let hour_secs = secs - secs.rem_euclid(3600);
    chrono::DateTime::<Utc>::from_timestamp(hour_secs, 0).unwrap_or(ts)
}

#[cfg(test)]
mod tests {
    use maxi_loop_performance::histogram::{NUM_HISTOGRAM_BUCKETS, observe_duration};

    use super::*;

    fn histogram(durations: &[f64]) -> Vec<i64> {
        let mut buckets = [0i64; NUM_HISTOGRAM_BUCKETS];
        for &d in durations {
            observe_duration(&mut buckets, d);
        }
        buckets.to_vec()
    }

    #[test]
    fn p95_and_error_rate_from_window() {
        let mut durations = vec![20.0; 90];
        durations.extend(vec![900.0; 10]);
        let hist = histogram(&durations);
        let p95 = performance_metric_value(AlertTriggerType::PerfP95, 100, 0, &hist, 20.0, 900.0)
            .expect("p95");
        assert!(p95 > 500.0, "p95 should land in the slow tail, got {p95}");

        let rate =
            performance_metric_value(AlertTriggerType::PerfErrorRate, 300, 7, &hist, 20.0, 900.0)
                .expect("rate");
        assert!((rate - 2.33).abs() < f64::EPSILON, "{rate}");

        assert!(performance_metric_value(AlertTriggerType::PerfP95, 0, 0, &[], 0.0, 0.0).is_none());
        assert!(
            performance_metric_value(AlertTriggerType::Spike, 10, 1, &hist, 1.0, 2.0).is_none()
        );
    }

    #[test]
    fn perf_threshold_validation_per_trigger() {
        assert!(
            AlertTriggerType::PerfP95
                .validate_perf_threshold(0.5)
                .is_err()
        );
        assert!(
            AlertTriggerType::PerfP95
                .validate_perf_threshold(500.0)
                .is_ok()
        );
        assert!(
            AlertTriggerType::PerfP95
                .validate_perf_threshold(f64::NAN)
                .is_err()
        );
        assert!(
            AlertTriggerType::PerfErrorRate
                .validate_perf_threshold(0.0)
                .is_ok()
        );
        assert!(
            AlertTriggerType::PerfErrorRate
                .validate_perf_threshold(100.0)
                .is_err()
        );
        assert!(
            AlertTriggerType::Spike
                .validate_perf_threshold(5.0)
                .is_err()
        );
    }
}
