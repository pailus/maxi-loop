use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::begin_worker_tx;
use sqlx::{PgPool, Postgres, QueryBuilder};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, warn};

use crate::fingerprint::{ProcessedEvent, process_queued_event};
use crate::model::{QueuedEnvelope, QueuedEvent};

#[derive(Clone, Copy, Debug)]
pub struct IngestWorkerConfig {
    pub flush_interval: Duration,
    pub max_batch_events: usize,
    pub max_full_events_per_issue: u32,
}

impl IngestWorkerConfig {
    pub fn from_config(config: &Config) -> Self {
        Self {
            flush_interval: Duration::from_millis(100),
            max_batch_events: 256,
            max_full_events_per_issue: config.max_full_events_per_issue,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchFlushStats {
    pub processed_events: usize,
    pub upserted_issues: usize,
    pub regressed_issues: usize,
    pub inserted_full_events: usize,
    pub updated_hourly_buckets: usize,
}

#[derive(Clone, Debug)]
struct EventToInsert {
    tenant_id: i64,
    project_id: i64,
    issue_id: i64,
    event_id: String,
    received_at: DateTime<Utc>,
    environment: String,
    release: Option<String>,
    level: String,
    platform: String,
    exception_type: String,
    exception_value: String,
    stack_frames: serde_json::Value,
    tags: serde_json::Value,
    payload: serde_json::Value,
    is_sample: bool,
}

#[derive(sqlx::FromRow)]
struct ExistingIssueRow {
    id: i64,
    status: String,
    is_regressed: bool,
    stored_full_events: i32,
    sample_event_id: Option<String>,
}

/// Truncate a UTC timestamp to the start of its UTC hour (`YYYY-MM-DDTHH:00:00Z`).
pub fn truncate_to_hour_utc(ts: DateTime<Utc>) -> DateTime<Utc> {
    let secs = ts.timestamp();
    let hour_secs = secs - secs.rem_euclid(3600);
    DateTime::<Utc>::from_timestamp(hour_secs, 0).unwrap_or(ts)
}

/// Spawn the asynchronous ingest batch worker that drains `QueuedEnvelope` items from the bounded
/// `tokio::sync::mpsc` channel, scrubs PII, computes fingerprints, and flushes batches to PostgreSQL
/// every `100ms`–`1s` (or when `max_batch_events` is reached).
pub fn spawn_ingest_worker(
    pool: PgPool,
    mut rx: mpsc::Receiver<QueuedEnvelope>,
    worker_config: IngestWorkerConfig,
) -> JoinHandle<()> {
    let flush_interval = worker_config
        .flush_interval
        .clamp(Duration::from_millis(10), Duration::from_secs(1));
    let max_batch_events = worker_config.max_batch_events.max(1);
    let max_full_events_per_issue = worker_config.max_full_events_per_issue.max(1);

    tokio::spawn(async move {
        let mut pending: Vec<ProcessedEvent> = Vec::with_capacity(max_batch_events);

        loop {
            let first = match rx.recv().await {
                Some(envelope) => envelope,
                None => break,
            };

            append_envelope_events(first, &mut pending);

            let deadline = tokio::time::Instant::now() + flush_interval;
            while pending.len() < max_batch_events {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(envelope)) => {
                        append_envelope_events(envelope, &mut pending);
                    }
                    Ok(None) | Err(_) => break,
                }
            }

            if !pending.is_empty() {
                if let Err(err) =
                    flush_processed_batch(&pool, &pending, max_full_events_per_issue).await
                {
                    error!(
                        batch_events = pending.len(),
                        error = %err,
                        "failed to flush ingest event batch to PostgreSQL"
                    );
                }
                pending.clear();
            }
        }
    })
}

fn append_envelope_events(envelope: QueuedEnvelope, out: &mut Vec<ProcessedEvent>) {
    for raw_ev in envelope.events {
        match process_queued_event(raw_ev) {
            Ok(processed) => out.push(processed),
            Err(err) => {
                warn!(
                    tenant_id = envelope.tenant_id,
                    project_id = envelope.project_id,
                    envelope_id = %envelope.envelope_id,
                    error = %err,
                    "dropping malformed event item in worker"
                );
            }
        }
    }
}

/// Process and flush a slice of raw `QueuedEvent` items in a single worker batch transaction.
pub async fn flush_queued_events(
    pool: &PgPool,
    events: Vec<QueuedEvent>,
    max_full_events_per_issue: u32,
) -> Result<BatchFlushStats, AppError> {
    let mut processed = Vec::with_capacity(events.len());
    for ev in events {
        processed.push(process_queued_event(ev)?);
    }
    flush_processed_batch(pool, &processed, max_full_events_per_issue).await
}

/// Flush a batch of `ProcessedEvent` items to PostgreSQL inside a single `maxi_loop_worker` transaction:
/// 1. Ensures target daily `events_YYYY_MM_DD` partitions exist.
/// 2. Upserts `issues` grouped by `(tenant_id, project_id, fingerprint)` and auto-reopens `resolved`
///    issues on regression (`status = 'unresolved'`, `is_regressed = TRUE`).
/// 3. Caps full event payloads stored in `events` at `max_full_events_per_issue` (+ sample on first
///    event or regression after cap).
/// 4. Batch-inserts stored `events` and batch-upserts `issue_hourly_counters`.
pub async fn flush_processed_batch(
    pool: &PgPool,
    batch: &[ProcessedEvent],
    max_full_events_per_issue: u32,
) -> Result<BatchFlushStats, AppError> {
    if batch.is_empty() {
        return Ok(BatchFlushStats::default());
    }

    let cap = i32::try_from(max_full_events_per_issue.max(1)).unwrap_or(50);
    let mut tx = begin_worker_tx(pool).await?;

    // 1. Ensure daily partitions exist for all event dates present in this batch.
    // Check `to_regclass` first so we do not execute DDL (`AccessExclusiveLock`) when the
    // daily partition already exists.
    let distinct_dates: BTreeSet<NaiveDate> =
        batch.iter().map(|ev| ev.received_at.date_naive()).collect();
    for target_date in distinct_dates {
        let part_regclass = format!("public.events_{}", target_date.format("%Y_%m_%d"));
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(&part_regclass)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("check partition {part_regclass}: {err}"))
            })?;
        if !exists {
            sqlx::query("SELECT ensure_events_partition($1)")
                .bind(target_date)
                .execute(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!(
                        "ensure_events_partition({target_date}): {err}"
                    ))
                })?;
        }
    }

    // 2. Group events by `(tenant_id, project_id, fingerprint)` in deterministic BTreeMap order.
    let mut grouped: BTreeMap<(i64, i64, String), Vec<&ProcessedEvent>> = BTreeMap::new();
    for ev in batch {
        grouped
            .entry((ev.tenant_id, ev.project_id, ev.fingerprint.clone()))
            .or_default()
            .push(ev);
    }

    let mut events_to_insert: Vec<EventToInsert> = Vec::new();
    let mut hourly_increments: BTreeMap<(i64, i64, i64, DateTime<Utc>), i64> = BTreeMap::new();
    let mut regressed_issues = 0usize;

    for ((tenant_id, project_id, fingerprint), group_events) in &grouped {
        let batch_event_count = i64::try_from(group_events.len()).unwrap_or(1);
        let first_seen = group_events
            .iter()
            .map(|e| e.received_at)
            .min()
            .unwrap_or_else(Utc::now);
        let latest_ev = group_events
            .iter()
            .max_by_key(|e| e.received_at)
            .copied()
            .unwrap_or(group_events[0]);

        let existing: Option<ExistingIssueRow> = sqlx::query_as(
            "SELECT id, status, is_regressed, stored_full_events, sample_event_id \
             FROM issues \
             WHERE tenant_id = $1 AND project_id = $2 AND fingerprint = $3 \
             FOR UPDATE",
        )
        .bind(*tenant_id)
        .bind(*project_id)
        .bind(fingerprint)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("select issue for update: {err}")))?;

        let (issue_id, prev_stored, was_resolved, mut sample_event_id) = match existing {
            Some(row) => {
                let is_regression = row.status == "resolved";
                if is_regression {
                    regressed_issues += 1;
                }
                let next_status = if is_regression {
                    "unresolved"
                } else {
                    row.status.as_str()
                };
                let next_regressed = row.is_regressed || is_regression;

                (
                    Some((row.id, next_status.to_owned(), next_regressed)),
                    row.stored_full_events.max(0),
                    is_regression,
                    row.sample_event_id,
                )
            }
            None => (None, 0, false, None),
        };

        // Determine how many events from this batch can be stored within the regular `cap`
        // plus whether we should capture an extra regression sample event if `prev_stored >= cap`.
        let remaining_regular_slots = usize::try_from((cap - prev_stored).max(0)).unwrap_or(0);
        let regular_to_store = group_events.len().min(remaining_regular_slots);
        let new_stored_full_events = prev_stored + i32::try_from(regular_to_store).unwrap_or(0);

        let store_extra_regression_sample = was_resolved && regular_to_store == 0;

        if sample_event_id.is_none() {
            sample_event_id = Some(group_events[0].event_id.clone());
        } else if was_resolved {
            // Point `sample_event_id` to the event that triggered the regression reopen.
            let sample_ev = if regular_to_store > 0 {
                group_events[regular_to_store - 1]
            } else {
                latest_ev
            };
            sample_event_id = Some(sample_ev.event_id.clone());
        }

        let resolved_issue_id: i64 = match issue_id {
            Some((id, next_status, next_regressed)) => {
                sqlx::query(
                    "UPDATE issues \
                     SET title = $4, \
                         exception_type = $5, \
                         exception_value = $6, \
                         culprit = CASE WHEN $7 <> '' THEN $7 ELSE culprit END, \
                         level = $8, \
                         status = $9, \
                         is_regressed = $10, \
                         event_count = event_count + $11, \
                         stored_full_events = $12, \
                         first_seen = LEAST(first_seen, $13), \
                         last_seen = GREATEST(last_seen, $14), \
                         last_environment = $15, \
                         last_release = COALESCE($16, last_release), \
                         sample_event_id = COALESCE($17, sample_event_id), \
                         updated_at = NOW() \
                     WHERE tenant_id = $1 AND project_id = $2 AND id = $3",
                )
                .bind(*tenant_id)
                .bind(*project_id)
                .bind(id)
                .bind(&latest_ev.title)
                .bind(&latest_ev.exception_type)
                .bind(&latest_ev.exception_value)
                .bind(&latest_ev.culprit)
                .bind(&latest_ev.level)
                .bind(&next_status)
                .bind(next_regressed)
                .bind(batch_event_count)
                .bind(new_stored_full_events)
                .bind(first_seen)
                .bind(latest_ev.received_at)
                .bind(&latest_ev.environment)
                .bind(&latest_ev.release)
                .bind(&sample_event_id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("update issue {id}: {err}")))?;
                id
            }
            None => {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO issues ( \
                        tenant_id, project_id, fingerprint, title, exception_type, exception_value, \
                        culprit, level, status, is_regressed, event_count, stored_full_events, \
                        first_seen, last_seen, last_environment, last_release, sample_event_id \
                     ) VALUES ( \
                        $1, $2, $3, $4, $5, $6, \
                        $7, $8, 'unresolved', FALSE, $9, $10, \
                        $11, $12, $13, $14, $15 \
                     ) RETURNING id",
                )
                .bind(*tenant_id)
                .bind(*project_id)
                .bind(fingerprint)
                .bind(&latest_ev.title)
                .bind(&latest_ev.exception_type)
                .bind(&latest_ev.exception_value)
                .bind(&latest_ev.culprit)
                .bind(&latest_ev.level)
                .bind(batch_event_count)
                .bind(new_stored_full_events)
                .bind(first_seen)
                .bind(latest_ev.received_at)
                .bind(&latest_ev.environment)
                .bind(&latest_ev.release)
                .bind(&sample_event_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("insert issue: {err}")))?
            }
        };

        // Queue capped full events (up to `regular_to_store`) for multi-row batch insert.
        for (idx, ev) in group_events.iter().take(regular_to_store).enumerate() {
            let is_first_for_issue = prev_stored == 0 && idx == 0;
            let is_regression_sample = was_resolved && idx + 1 == regular_to_store;
            events_to_insert.push(EventToInsert {
                tenant_id: *tenant_id,
                project_id: *project_id,
                issue_id: resolved_issue_id,
                event_id: ev.event_id.clone(),
                received_at: ev.received_at,
                environment: ev.environment.clone(),
                release: ev.release.clone(),
                level: ev.level.clone(),
                platform: ev.platform.clone(),
                exception_type: ev.exception_type.clone(),
                exception_value: ev.exception_value.clone(),
                stack_frames: ev.stack_frames.clone(),
                tags: ev.tags.clone(),
                payload: ev.payload.clone(),
                is_sample: is_first_for_issue || is_regression_sample,
            });
        }

        // If a resolved issue regressed AFTER already reaching `max_full_events_per_issue`,
        // store 1 sample event (`is_sample = true`) for the regression occurrence ("N + sample").
        if store_extra_regression_sample {
            events_to_insert.push(EventToInsert {
                tenant_id: *tenant_id,
                project_id: *project_id,
                issue_id: resolved_issue_id,
                event_id: latest_ev.event_id.clone(),
                received_at: latest_ev.received_at,
                environment: latest_ev.environment.clone(),
                release: latest_ev.release.clone(),
                level: latest_ev.level.clone(),
                platform: latest_ev.platform.clone(),
                exception_type: latest_ev.exception_type.clone(),
                exception_value: latest_ev.exception_value.clone(),
                stack_frames: latest_ev.stack_frames.clone(),
                tags: latest_ev.tags.clone(),
                payload: latest_ev.payload.clone(),
                is_sample: true,
            });
        }

        // Aggregate hourly counters for ALL events in the group (even when full payloads are capped).
        for ev in group_events {
            let hour_bucket = truncate_to_hour_utc(ev.received_at);
            *hourly_increments
                .entry((*tenant_id, *project_id, resolved_issue_id, hour_bucket))
                .or_insert(0) += 1;
        }
    }

    // 3. Multi-row batch insert into `events` (for events under the per-issue cap + sample).
    if !events_to_insert.is_empty() {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO events ( \
                tenant_id, project_id, issue_id, event_id, received_at, \
                environment, release, level, platform, exception_type, \
                exception_value, stack_frames, tags, payload, is_sample \
             ) ",
        );
        qb.push_values(&events_to_insert, |mut b, ev| {
            b.push_bind(ev.tenant_id)
                .push_bind(ev.project_id)
                .push_bind(ev.issue_id)
                .push_bind(&ev.event_id)
                .push_bind(ev.received_at)
                .push_bind(&ev.environment)
                .push_bind(&ev.release)
                .push_bind(&ev.level)
                .push_bind(&ev.platform)
                .push_bind(&ev.exception_type)
                .push_bind(&ev.exception_value)
                .push_bind(&ev.stack_frames)
                .push_bind(&ev.tags)
                .push_bind(&ev.payload)
                .push_bind(ev.is_sample);
        });
        qb.push(" ON CONFLICT (tenant_id, project_id, received_at, event_id) DO NOTHING");
        qb.build()
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("batch insert events: {err}")))?;
    }

    // 4. Multi-row batch upsert into `issue_hourly_counters`.
    if !hourly_increments.is_empty() {
        let rows: Vec<(i64, i64, i64, DateTime<Utc>, i64)> = hourly_increments
            .iter()
            .map(|(&(t, p, i, bucket), &cnt)| (t, p, i, bucket, cnt))
            .collect();

        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO issue_hourly_counters ( \
                tenant_id, project_id, issue_id, hour_bucket, event_count \
             ) ",
        );
        qb.push_values(&rows, |mut b, row| {
            b.push_bind(row.0)
                .push_bind(row.1)
                .push_bind(row.2)
                .push_bind(row.3)
                .push_bind(row.4);
        });
        qb.push(
            " ON CONFLICT (tenant_id, project_id, issue_id, hour_bucket) \
              DO UPDATE SET event_count = issue_hourly_counters.event_count + EXCLUDED.event_count",
        );
        qb.build().execute(&mut *tx).await.map_err(|err| {
            AppError::DatabaseQuery(format!("batch upsert issue_hourly_counters: {err}"))
        })?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit worker batch tx: {err}")))?;

    Ok(BatchFlushStats {
        processed_events: batch.len(),
        upserted_issues: grouped.len(),
        regressed_issues,
        inserted_full_events: events_to_insert.len(),
        updated_hourly_buckets: hourly_increments.len(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn truncates_timestamp_to_utc_hour_boundary() {
        let ts = Utc
            .with_ymd_and_hms(2026, 11, 21, 14, 37, 59)
            .single()
            .expect("valid timestamp");
        let expected = Utc
            .with_ymd_and_hms(2026, 11, 21, 14, 0, 0)
            .single()
            .expect("valid hour timestamp");
        assert_eq!(truncate_to_hour_utc(ts), expected);
    }
}
