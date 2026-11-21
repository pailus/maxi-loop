use std::time::Duration as StdDuration;

use chrono::{DateTime, Days, Duration, NaiveDate, Utc};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::begin_worker_tx;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::model::{RetentionConfig, RetentionReport};

#[derive(Clone, Copy, Debug, FromRow)]
struct ProjectRetentionRow {
    tenant_id: i64,
    project_id: i64,
    tenant_retention_days: i16,
    workspace_retention_override: Option<i16>,
    project_retention_override: Option<i16>,
}

/// Compute the effective retention in days for a project given global, tenant, workspace,
/// and project overrides, always capped at `<= global_retention_days` per `D003`.
pub fn compute_effective_retention_days(
    global_retention_days: u16,
    tenant_retention_days: i16,
    workspace_override: Option<i16>,
    project_override: Option<i16>,
) -> u16 {
    let global = global_retention_days.max(1);
    let tenant_days = u16::try_from(tenant_retention_days.max(1))
        .unwrap_or(global)
        .min(global);
    let ws_days = workspace_override
        .and_then(|d| u16::try_from(d).ok())
        .filter(|&d| d >= 1)
        .unwrap_or(tenant_days)
        .min(tenant_days);
    project_override
        .and_then(|d| u16::try_from(d).ok())
        .filter(|&d| d >= 1)
        .unwrap_or(ws_days)
        .min(ws_days)
}

/// Compute the partition cutoff date (`reference_date - global_retention_days`).
/// Any daily partition `events_YYYY_MM_DD` with date `< cutoff_date` is expired.
pub fn compute_partition_cutoff_date(
    reference_date: NaiveDate,
    global_retention_days: u16,
) -> Result<NaiveDate, AppError> {
    reference_date
        .checked_sub_days(Days::new(u64::from(global_retention_days.max(1))))
        .ok_or_else(|| AppError::Validation("retention cutoff date underflow".to_owned()))
}

/// Spawn the background retention maintenance scheduler task.
pub fn spawn_retention_scheduler(pool: PgPool, cfg: RetentionConfig) -> JoinHandle<()> {
    let interval = cfg.schedule_interval.max(StdDuration::from_secs(60));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let now = Utc::now();
            match run_retention_cycle(&pool, &cfg, now).await {
                Ok(report) => {
                    info!(
                        ensured_partitions = report.ensured_partitions.len(),
                        dropped_partitions = ?report.dropped_partitions,
                        ensured_trace_partitions = report.ensured_trace_partitions.len(),
                        dropped_trace_partitions = ?report.dropped_trace_partitions,
                        override_deleted_events = report.override_deleted_events,
                        override_deleted_hourly_counters = report.override_deleted_hourly_counters,
                        override_deleted_issues = report.override_deleted_issues,
                        override_deleted_traces = report.override_deleted_traces,
                        override_deleted_tx_metrics = report.override_deleted_tx_metrics,
                        global_deleted_hourly_counters = report.global_deleted_hourly_counters,
                        global_deleted_issues = report.global_deleted_issues,
                        global_deleted_tx_metrics = report.global_deleted_tx_metrics,
                        deleted_expired_sessions = report.deleted_expired_sessions,
                        "completed scheduled retention maintenance cycle"
                    );
                }
                Err(err) => {
                    error!(error = %err, "scheduled retention maintenance cycle failed");
                }
            }
        }
    })
}

/// Run one full retention maintenance cycle at `reference_now`:
/// 1. Pre-create upcoming daily `events_YYYY_MM_DD` partitions for `[today, today + days_ahead]`.
/// 2. Drop expired daily `events_YYYY_MM_DD` partitions older than `global_retention_days` (`DROP TABLE`).
/// 3. For any project whose effective retention (from tenant, workspace, or project override) is shorter
///    than `global_retention_days`, run bounded batch deletes on `events`, `issue_hourly_counters`, and
///    stale `issues`.
/// 4. Clean up global expired `issue_hourly_counters`, stale `issues`, and expired `sessions` via bounded
///    batch deletes.
pub async fn run_retention_cycle(
    pool: &PgPool,
    cfg: &RetentionConfig,
    reference_now: DateTime<Utc>,
) -> Result<RetentionReport, AppError> {
    let today = reference_now.date_naive();
    let global_days = cfg.global_retention_days.max(1);
    let batch_limit = cfg.batch_delete_limit.max(1);
    let max_batches = cfg.max_batches_per_run.max(1);

    let mut report = RetentionReport::default();

    // 1. Pre-create upcoming daily partitions & drop expired daily partitions older than global retention.
    {
        let mut tx = begin_worker_tx(pool).await?;
        for offset in 0..=u64::from(cfg.days_ahead) {
            let target_date = today
                .checked_add_days(Days::new(offset))
                .ok_or_else(|| AppError::Validation("partition date overflow".to_owned()))?;
            let part_name = format!("events_{}", target_date.format("%Y_%m_%d"));
            let regclass_name = format!("public.{part_name}");
            let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(&regclass_name)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!("check partition {regclass_name}: {err}"))
                })?;

            if !exists {
                let created: String = sqlx::query_scalar("SELECT ensure_events_partition($1)")
                    .bind(target_date)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|err| {
                        AppError::DatabaseQuery(format!(
                            "ensure_events_partition({target_date}): {err}"
                        ))
                    })?;
                report.ensured_partitions.push(created);
            } else {
                report.ensured_partitions.push(part_name);
            }

            let trace_part_name = format!("traces_{}", target_date.format("%Y_%m_%d"));
            let trace_regclass = format!("public.{trace_part_name}");
            let trace_exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(&trace_regclass)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!("check partition {trace_regclass}: {err}"))
                })?;

            if !trace_exists {
                let created: String = sqlx::query_scalar("SELECT ensure_traces_partition($1)")
                    .bind(target_date)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|err| {
                        AppError::DatabaseQuery(format!(
                            "ensure_traces_partition({target_date}): {err}"
                        ))
                    })?;
                report.ensured_trace_partitions.push(created);
            } else {
                report.ensured_trace_partitions.push(trace_part_name);
            }
        }

        let cutoff_date = compute_partition_cutoff_date(today, global_days)?;
        let dropped: Vec<String> = sqlx::query_scalar("SELECT drop_expired_events_partitions($1)")
            .bind(cutoff_date)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!(
                    "drop_expired_events_partitions({cutoff_date}): {err}"
                ))
            })?;
        report.dropped_partitions = dropped;

        let dropped_traces: Vec<String> =
            sqlx::query_scalar("SELECT drop_expired_traces_partitions($1)")
                .bind(cutoff_date)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!(
                        "drop_expired_traces_partitions({cutoff_date}): {err}"
                    ))
                })?;
        report.dropped_trace_partitions = dropped_traces;

        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit partition maintenance: {err}"))
        })?;
    }

    // 2. Load projects and identify those with shorter effective retention overrides (`< global_days`).
    let project_rows: Vec<ProjectRetentionRow> = {
        let mut tx = begin_worker_tx(pool).await?;
        let rows = sqlx::query_as(
            "SELECT p.tenant_id,
                    p.id AS project_id,
                    t.retention_days AS tenant_retention_days,
                    w.retention_days_override AS workspace_retention_override,
                    p.retention_days_override AS project_retention_override
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             JOIN tenants t ON t.id = p.tenant_id
             ORDER BY p.tenant_id ASC, p.id ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("load project retention overrides: {err}"))
        })?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit load project retention: {err}"))
        })?;
        rows
    };

    for row in project_rows {
        let effective_days = compute_effective_retention_days(
            global_days,
            row.tenant_retention_days,
            row.workspace_retention_override,
            row.project_retention_override,
        );
        if effective_days >= global_days {
            continue;
        }

        let project_cutoff_ts = reference_now - Duration::days(i64::from(effective_days));

        report.override_deleted_events += bounded_delete_project_events(
            pool,
            row.tenant_id,
            row.project_id,
            project_cutoff_ts,
            batch_limit,
            max_batches,
        )
        .await?;

        report.override_deleted_hourly_counters += bounded_delete_project_hourly_counters(
            pool,
            row.tenant_id,
            row.project_id,
            project_cutoff_ts,
            batch_limit,
            max_batches,
        )
        .await?;

        report.override_deleted_issues += bounded_delete_project_issues(
            pool,
            row.tenant_id,
            row.project_id,
            project_cutoff_ts,
            batch_limit,
            max_batches,
        )
        .await?;

        report.override_deleted_traces += bounded_delete_project_traces(
            pool,
            row.tenant_id,
            row.project_id,
            project_cutoff_ts,
            batch_limit,
            max_batches,
        )
        .await?;

        report.override_deleted_tx_metrics += bounded_delete_project_tx_metrics(
            pool,
            row.tenant_id,
            row.project_id,
            project_cutoff_ts,
            batch_limit,
            max_batches,
        )
        .await?;
    }

    // 3. Global cutoff cleanup for counters, metrics, stale `issues`, and expired `sessions`.
    let global_cutoff_ts = reference_now - Duration::days(i64::from(global_days));
    report.global_deleted_hourly_counters =
        bounded_delete_global_hourly_counters(pool, global_cutoff_ts, batch_limit, max_batches)
            .await?;
    report.global_deleted_issues =
        bounded_delete_global_issues(pool, global_cutoff_ts, batch_limit, max_batches).await?;
    report.global_deleted_tx_metrics =
        bounded_delete_global_tx_metrics(pool, global_cutoff_ts, batch_limit, max_batches).await?;
    report.deleted_expired_sessions =
        bounded_delete_expired_sessions(pool, reference_now, batch_limit, max_batches).await?;

    Ok(report)
}

async fn bounded_delete_project_events(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let deleted = delete_project_events_batch_in_tx(
            &mut tx,
            tenant_id,
            project_id,
            cutoff_ts,
            batch_limit,
        )
        .await?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit project events delete: {err}"))
        })?;
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn delete_project_events_batch_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    project_id: i64,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
) -> Result<u64, AppError> {
    let res = sqlx::query(
        "WITH doomed AS (
             SELECT received_at, event_id
             FROM events
             WHERE tenant_id = $1
               AND project_id = $2
               AND received_at < $3
             LIMIT $4
         )
         DELETE FROM events e
         USING doomed d
         WHERE e.tenant_id = $1
           AND e.project_id = $2
           AND e.received_at = d.received_at
           AND e.event_id = d.event_id",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(cutoff_ts)
    .bind(batch_limit)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("batch delete override events: {err}")))?;

    Ok(res.rows_affected())
}

async fn bounded_delete_project_hourly_counters(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT issue_id, hour_bucket
                 FROM issue_hourly_counters
                 WHERE tenant_id = $1
                   AND project_id = $2
                   AND hour_bucket < $3
                 LIMIT $4
             )
             DELETE FROM issue_hourly_counters c
             USING doomed d
             WHERE c.tenant_id = $1
               AND c.project_id = $2
               AND c.issue_id = d.issue_id
               AND c.hour_bucket = d.hour_bucket",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(cutoff_ts)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("batch delete override hourly counters: {err}"))
        })?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit override hourly counters delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn bounded_delete_project_issues(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT id
                 FROM issues
                 WHERE tenant_id = $1
                   AND project_id = $2
                   AND last_seen < $3
                 LIMIT $4
             )
             DELETE FROM issues i
             USING doomed d
             WHERE i.tenant_id = $1
               AND i.project_id = $2
               AND i.id = d.id",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(cutoff_ts)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("batch delete override issues: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit override issues delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn bounded_delete_global_hourly_counters(
    pool: &PgPool,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT tenant_id, project_id, issue_id, hour_bucket
                 FROM issue_hourly_counters
                 WHERE hour_bucket < $1
                 LIMIT $2
             )
             DELETE FROM issue_hourly_counters c
             USING doomed d
             WHERE c.tenant_id = d.tenant_id
               AND c.project_id = d.project_id
               AND c.issue_id = d.issue_id
               AND c.hour_bucket = d.hour_bucket",
        )
        .bind(cutoff_ts)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("batch delete global hourly counters: {err}"))
        })?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit global hourly counters delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn bounded_delete_global_issues(
    pool: &PgPool,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT tenant_id, project_id, id
                 FROM issues
                 WHERE last_seen < $1
                 LIMIT $2
             )
             DELETE FROM issues i
             USING doomed d
             WHERE i.tenant_id = d.tenant_id
               AND i.project_id = d.project_id
               AND i.id = d.id",
        )
        .bind(cutoff_ts)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("batch delete global issues: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit global issues delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn bounded_delete_expired_sessions(
    pool: &PgPool,
    now: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT tenant_id, id
                 FROM sessions
                 WHERE expires_at < $1
                 LIMIT $2
             )
             DELETE FROM sessions s
             USING doomed d
             WHERE s.tenant_id = d.tenant_id
               AND s.id = d.id",
        )
        .bind(now)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("batch delete expired sessions: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit expired sessions delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn bounded_delete_project_traces(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT received_at, event_id
                 FROM traces
                 WHERE tenant_id = $1
                   AND project_id = $2
                   AND received_at < $3
                 LIMIT $4
             )
             DELETE FROM traces t
             USING doomed d
             WHERE t.tenant_id = $1
               AND t.project_id = $2
               AND t.received_at = d.received_at
               AND t.event_id = d.event_id",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(cutoff_ts)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("batch delete override traces: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit override traces delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn bounded_delete_project_tx_metrics(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT transaction_name, op, environment, release, minute_bucket
                 FROM transaction_minute_metrics
                 WHERE tenant_id = $1
                   AND project_id = $2
                   AND minute_bucket < $3
                 LIMIT $4
             )
             DELETE FROM transaction_minute_metrics m
             USING doomed d
             WHERE m.tenant_id = $1
               AND m.project_id = $2
               AND m.transaction_name = d.transaction_name
               AND m.op = d.op
               AND m.environment = d.environment
               AND m.release = d.release
               AND m.minute_bucket = d.minute_bucket",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(cutoff_ts)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("batch delete override tx metrics: {err}"))
        })?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit override tx metrics delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

async fn bounded_delete_global_tx_metrics(
    pool: &PgPool,
    cutoff_ts: DateTime<Utc>,
    batch_limit: i64,
    max_batches: usize,
) -> Result<u64, AppError> {
    let mut total_deleted = 0u64;
    for _ in 0..max_batches {
        let mut tx = begin_worker_tx(pool).await?;
        let res = sqlx::query(
            "WITH doomed AS (
                 SELECT tenant_id, project_id, transaction_name, op, environment, release, minute_bucket
                 FROM transaction_minute_metrics
                 WHERE minute_bucket < $1
                 LIMIT $2
             )
             DELETE FROM transaction_minute_metrics m
             USING doomed d
             WHERE m.tenant_id = d.tenant_id
               AND m.project_id = d.project_id
               AND m.transaction_name = d.transaction_name
               AND m.op = d.op
               AND m.environment = d.environment
               AND m.release = d.release
               AND m.minute_bucket = d.minute_bucket",
        )
        .bind(cutoff_ts)
        .bind(batch_limit)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("batch delete global tx metrics: {err}"))
        })?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit global tx metrics delete: {err}"))
        })?;
        let deleted = res.rows_affected();
        total_deleted += deleted;
        if deleted < u64::try_from(batch_limit).unwrap_or(1) {
            break;
        }
    }
    Ok(total_deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_effective_retention_days_capped_by_hierarchy_and_global() {
        assert_eq!(compute_effective_retention_days(30, 30, None, None), 30);
        assert_eq!(compute_effective_retention_days(30, 30, Some(14), None), 14);
        assert_eq!(
            compute_effective_retention_days(30, 30, Some(14), Some(7)),
            7
        );
        // Overrides higher than parent or global are always clamped to the parent/global cap.
        assert_eq!(
            compute_effective_retention_days(30, 45, Some(60), Some(90)),
            30
        );
        assert_eq!(
            compute_effective_retention_days(30, 21, Some(14), Some(20)),
            14
        );
    }

    #[test]
    fn computes_partition_cutoff_date() {
        let ref_date = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let cutoff = compute_partition_cutoff_date(ref_date, 30).unwrap();
        assert_eq!(cutoff, NaiveDate::from_ymd_opt(2026, 10, 22).unwrap());
    }
}
