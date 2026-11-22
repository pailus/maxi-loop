use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use maxi_loop_performance::ingest_proc::{
    TraceSamplingPolicy, flush_processed_transactions_in_tx, process_queued_transaction_with_rules,
};
use maxi_loop_performance::model::{ProcessedTransaction, TransactionFlushStats};
use maxi_loop_platform::alert_notify::{AlertSignal, AlertSignalKind, AlertSignalObserver};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::begin_worker_tx;
use maxi_loop_sourcemaps::resolver::SourceMapResolver;
use sqlx::{PgPool, Postgres, QueryBuilder};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, warn};

use crate::fingerprint::{
    ProcessedEvent, process_queued_event_with_rules, process_queued_event_with_rules_and_sourcemaps,
};
use crate::model::{QueuedEnvelope, QueuedEvent};
use crate::perf_issue::performance_problem_events;
use crate::scrub_cache::ScrubRulesCache;

#[derive(Clone, Copy, Debug)]
pub struct IngestWorkerConfig {
    pub flush_interval: Duration,
    pub max_batch_events: usize,
    pub max_full_events_per_issue: u32,
    pub trace_sampling: TraceSamplingPolicy,
}

impl IngestWorkerConfig {
    pub fn from_config(config: &Config) -> Self {
        Self {
            flush_interval: Duration::from_millis(100),
            max_batch_events: 256,
            max_full_events_per_issue: config.max_full_events_per_issue,
            trace_sampling: TraceSamplingPolicy {
                slow_threshold_ms: f64::from(config.trace_slow_threshold_ms),
                random_sample_rate: config.trace_random_sample_rate,
            },
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
    fingerprint: String,
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

/// Spawn the asynchronous ingest batch worker with default (baseline-only) PII scrubbing.
pub fn spawn_ingest_worker(
    pool: PgPool,
    rx: mpsc::Receiver<QueuedEnvelope>,
    worker_config: IngestWorkerConfig,
) -> JoinHandle<()> {
    spawn_ingest_worker_with_hooks(pool, rx, worker_config, None, None, None)
}

/// Spawn the asynchronous ingest batch worker with inherited configurable PII scrubbing (`ScrubRulesCache`),
/// real-time alert signal emission (`AlertSignalObserver`), and bounded-LRU JS source map resolution.
pub fn spawn_ingest_worker_with_hooks(
    pool: PgPool,
    mut rx: mpsc::Receiver<QueuedEnvelope>,
    worker_config: IngestWorkerConfig,
    scrub_cache: Option<ScrubRulesCache>,
    alert_observer: Option<Arc<dyn AlertSignalObserver>>,
    sourcemap_resolver: Option<SourceMapResolver>,
) -> JoinHandle<()> {
    let flush_interval = worker_config
        .flush_interval
        .clamp(Duration::from_millis(10), Duration::from_secs(1));
    let max_batch_events = worker_config.max_batch_events.max(1);
    let max_full_events_per_issue = worker_config.max_full_events_per_issue.max(1);
    let trace_sampling = worker_config.trace_sampling;

    tokio::spawn(async move {
        let mut pending: Vec<ProcessedEvent> = Vec::with_capacity(max_batch_events);
        let mut pending_txs: Vec<ProcessedTransaction> = Vec::with_capacity(max_batch_events);

        loop {
            let first = match rx.recv().await {
                Some(envelope) => envelope,
                None => break,
            };

            append_envelope_items(
                first,
                scrub_cache.as_ref(),
                sourcemap_resolver.as_ref(),
                trace_sampling,
                &mut pending,
                &mut pending_txs,
            )
            .await;

            let deadline = tokio::time::Instant::now() + flush_interval;
            while pending.len() + pending_txs.len() < max_batch_events {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(envelope)) => {
                        append_envelope_items(
                            envelope,
                            scrub_cache.as_ref(),
                            sourcemap_resolver.as_ref(),
                            trace_sampling,
                            &mut pending,
                            &mut pending_txs,
                        )
                        .await;
                    }
                    Ok(None) | Err(_) => break,
                }
            }

            if !pending.is_empty() {
                match flush_processed_batch_with_signals(&pool, &pending, max_full_events_per_issue)
                    .await
                {
                    Ok((_stats, signals)) => {
                        if let Some(observer) = &alert_observer {
                            for sig in signals {
                                observer.on_alert_signal(sig);
                            }
                        }
                    }
                    Err(err) => {
                        error!(
                            batch_events = pending.len(),
                            error = %err,
                            "failed to flush ingest event batch to PostgreSQL"
                        );
                    }
                }
                pending.clear();
            }

            if !pending_txs.is_empty() {
                if let Err(err) = flush_processed_transactions(&pool, &pending_txs).await {
                    error!(
                        batch_transactions = pending_txs.len(),
                        error = %err,
                        "failed to flush ingest transaction batch to PostgreSQL"
                    );
                }
                pending_txs.clear();
            }
        }
    })
}

async fn append_envelope_items(
    envelope: QueuedEnvelope,
    scrub_cache: Option<&ScrubRulesCache>,
    sourcemap_resolver: Option<&SourceMapResolver>,
    trace_sampling: TraceSamplingPolicy,
    out_events: &mut Vec<ProcessedEvent>,
    out_txs: &mut Vec<ProcessedTransaction>,
) {
    let custom_rules = scrub_cache
        .map(|c| {
            c.effective_rules(
                envelope.tenant_id,
                envelope.workspace_id,
                envelope.project_id,
            )
        })
        .unwrap_or_default();

    for raw_ev in envelope.events {
        match process_queued_event_with_rules_and_sourcemaps(
            raw_ev,
            &custom_rules,
            sourcemap_resolver,
        )
        .await
        {
            Ok(processed) => out_events.push(processed),
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

    for raw_tx in envelope.transactions {
        match process_queued_transaction_with_rules(raw_tx, &custom_rules, trace_sampling) {
            Ok(processed) => {
                out_events.extend(performance_problem_events(&processed));
                out_txs.push(processed);
            }
            Err(err) => {
                warn!(
                    tenant_id = envelope.tenant_id,
                    project_id = envelope.project_id,
                    envelope_id = %envelope.envelope_id,
                    error = %err,
                    "dropping malformed transaction item in worker"
                );
            }
        }
    }
}

/// Flush a batch of processed performance transactions in a single `maxi_loop_worker` transaction.
pub async fn flush_processed_transactions(
    pool: &PgPool,
    batch: &[ProcessedTransaction],
) -> Result<TransactionFlushStats, AppError> {
    if batch.is_empty() {
        return Ok(TransactionFlushStats::default());
    }
    let mut tx = begin_worker_tx(pool).await?;
    let stats = flush_processed_transactions_in_tx(&mut tx, batch).await?;
    tx.commit().await.map_err(|err| {
        AppError::DatabaseQuery(format!("commit worker transaction batch: {err}"))
    })?;
    Ok(stats)
}

/// Process and flush a slice of raw `QueuedEvent` items in a single worker batch transaction.
pub async fn flush_queued_events(
    pool: &PgPool,
    events: Vec<QueuedEvent>,
    max_full_events_per_issue: u32,
) -> Result<BatchFlushStats, AppError> {
    let mut processed = Vec::with_capacity(events.len());
    for ev in events {
        processed.push(process_queued_event_with_rules(ev, &[])?);
    }
    flush_processed_batch(pool, &processed, max_full_events_per_issue).await
}

/// Flush a batch of `ProcessedEvent` items to PostgreSQL inside a single `maxi_loop_worker` transaction.
pub async fn flush_processed_batch(
    pool: &PgPool,
    batch: &[ProcessedEvent],
    max_full_events_per_issue: u32,
) -> Result<BatchFlushStats, AppError> {
    let (stats, _signals) =
        flush_processed_batch_with_signals(pool, batch, max_full_events_per_issue).await?;
    Ok(stats)
}

/// Flush a batch of `ProcessedEvent` items to PostgreSQL inside a single `maxi_loop_worker` transaction
/// and return both flush statistics and real-time `AlertSignal`s (for new issues & regressions).
pub async fn flush_processed_batch_with_signals(
    pool: &PgPool,
    batch: &[ProcessedEvent],
    max_full_events_per_issue: u32,
) -> Result<(BatchFlushStats, Vec<AlertSignal>), AppError> {
    if batch.is_empty() {
        return Ok((BatchFlushStats::default(), Vec::new()));
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
    let mut release_agg: BTreeMap<(i64, i64, String), ReleaseAgg> = BTreeMap::new();
    let mut issue_release_agg: BTreeMap<(i64, i64, i64, String), IssueReleaseAgg> = BTreeMap::new();
    let mut alert_signals: Vec<AlertSignal> = Vec::new();
    let mut regressed_issues = 0usize;

    for ((tenant_id, project_id, fingerprint), group_events) in &grouped {
        let batch_event_count = i64::try_from(group_events.len()).unwrap_or(1);
        let first_seen = group_events
            .iter()
            .map(|e| e.received_at)
            .min()
            .unwrap_or_else(Utc::now);
        let first_release_in_batch = group_events
            .iter()
            .min_by_key(|e| e.received_at)
            .and_then(|e| e.release.clone());
        let latest_ev = group_events
            .iter()
            .max_by_key(|e| e.received_at)
            .copied()
            .unwrap_or(group_events[0]);

        let mut routed_via_alias = false;
        let mut existing: Option<ExistingIssueRow> = sqlx::query_as(
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

        // If this fingerprint was previously merged into a target issue (`F-ISS-9`), route
        // incoming events to that target issue without resurrecting a duplicate issue.
        if existing.is_none() {
            existing = sqlx::query_as(
                "SELECT i.id, i.status, i.is_regressed, i.stored_full_events, i.sample_event_id \
                 FROM issue_fingerprints f \
                 JOIN issues i \
                   ON i.tenant_id = f.tenant_id AND i.project_id = f.project_id AND i.id = f.issue_id \
                 WHERE f.tenant_id = $1 AND f.project_id = $2 AND f.fingerprint = $3 \
                 FOR UPDATE OF i",
            )
            .bind(*tenant_id)
            .bind(*project_id)
            .bind(fingerprint)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("select aliased issue for update: {err}"))
            })?;
            routed_via_alias = existing.is_some();
        }

        let is_new_issue = existing.is_none();
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

        let (resolved_issue_id, total_issue_events): (i64, i64) = match issue_id {
            Some((id, next_status, next_regressed)) => {
                let updated_count: i64 = sqlx::query_scalar(
                    "UPDATE issues \
                     SET title = CASE WHEN $19 THEN title ELSE $4 END, \
                         exception_type = CASE WHEN $19 THEN exception_type ELSE $5 END, \
                         exception_value = CASE WHEN $19 THEN exception_value ELSE $6 END, \
                         culprit = CASE WHEN $19 THEN culprit WHEN $7 <> '' THEN $7 ELSE culprit END, \
                         level = $8, \
                         status = $9, \
                         is_regressed = $10, \
                         event_count = event_count + $11, \
                         stored_full_events = $12, \
                         first_seen = LEAST(first_seen, $13), \
                         last_seen = GREATEST(last_seen, $14), \
                         last_environment = $15, \
                         first_release = COALESCE(first_release, $16), \
                         last_release = COALESCE($17, last_release), \
                         sample_event_id = COALESCE($18, sample_event_id), \
                         updated_at = NOW() \
                     WHERE tenant_id = $1 AND project_id = $2 AND id = $3 \
                     RETURNING event_count",
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
                .bind(&first_release_in_batch)
                .bind(&latest_ev.release)
                .bind(&sample_event_id)
                .bind(routed_via_alias)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("update issue {id}: {err}")))?;
                (id, updated_count)
            }
            None => {
                let new_id: i64 = sqlx::query_scalar(
                    "INSERT INTO issues ( \
                        tenant_id, project_id, fingerprint, title, exception_type, exception_value, \
                        culprit, level, status, is_regressed, event_count, stored_full_events, \
                        first_seen, last_seen, last_environment, first_release, last_release, sample_event_id, \
                        issue_type \
                     ) VALUES ( \
                        $1, $2, $3, $4, $5, $6, \
                        $7, $8, 'unresolved', FALSE, $9, $10, \
                        $11, $12, $13, $14, $15, $16, \
                        $17 \
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
                .bind(&first_release_in_batch)
                .bind(&latest_ev.release)
                .bind(&sample_event_id)
                .bind(&latest_ev.issue_type)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("insert issue: {err}")))?;
                (new_id, batch_event_count)
            }
        };

        if is_new_issue || was_resolved {
            alert_signals.push(AlertSignal {
                tenant_id: *tenant_id,
                workspace_id: latest_ev.workspace_id,
                project_id: *project_id,
                issue_id: resolved_issue_id,
                kind: if is_new_issue {
                    AlertSignalKind::NewIssue
                } else {
                    AlertSignalKind::Regression
                },
                title: latest_ev.title.clone(),
                exception_type: latest_ev.exception_type.clone(),
                exception_value: latest_ev.exception_value.clone(),
                culprit: latest_ev.culprit.clone(),
                level: latest_ev.level.clone(),
                environment: latest_ev.environment.clone(),
                release: latest_ev.release.clone(),
                event_count: total_issue_events,
            });
        }

        // Queue capped full events (up to `regular_to_store`) for multi-row batch insert.
        for (idx, ev) in group_events.iter().take(regular_to_store).enumerate() {
            let is_first_for_issue = prev_stored == 0 && idx == 0;
            let is_regression_sample = was_resolved && idx + 1 == regular_to_store;
            events_to_insert.push(EventToInsert {
                tenant_id: *tenant_id,
                project_id: *project_id,
                issue_id: resolved_issue_id,
                event_id: ev.event_id.clone(),
                fingerprint: ev.fingerprint.clone(),
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
                fingerprint: latest_ev.fingerprint.clone(),
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

        // Aggregate hourly counters AND release links for ALL events in the group.
        let mut new_issue_attributed_to_release = false;
        for ev in group_events {
            let hour_bucket = truncate_to_hour_utc(ev.received_at);
            *hourly_increments
                .entry((*tenant_id, *project_id, resolved_issue_id, hour_bucket))
                .or_insert(0) += 1;

            if let Some(version) = ev.release.as_deref() {
                let rel_entry = release_agg
                    .entry((*tenant_id, *project_id, version.to_owned()))
                    .or_insert_with(|| ReleaseAgg {
                        environment: Some(ev.environment.clone()),
                        first_seen: ev.received_at,
                        last_seen: ev.received_at,
                        event_count: 0,
                        new_issues_count: 0,
                    });
                rel_entry.first_seen = rel_entry.first_seen.min(ev.received_at);
                if ev.received_at >= rel_entry.last_seen {
                    rel_entry.last_seen = ev.received_at;
                    rel_entry.environment = Some(ev.environment.clone());
                }
                rel_entry.event_count += 1;
                if is_new_issue && !new_issue_attributed_to_release {
                    rel_entry.new_issues_count += 1;
                    new_issue_attributed_to_release = true;
                }

                let ir_entry = issue_release_agg
                    .entry((
                        *tenant_id,
                        *project_id,
                        resolved_issue_id,
                        version.to_owned(),
                    ))
                    .or_insert_with(|| IssueReleaseAgg {
                        first_seen: ev.received_at,
                        last_seen: ev.received_at,
                        event_count: 0,
                    });
                ir_entry.first_seen = ir_entry.first_seen.min(ev.received_at);
                ir_entry.last_seen = ir_entry.last_seen.max(ev.received_at);
                ir_entry.event_count += 1;
            }
        }
    }

    // 3. Multi-row batch upsert into `releases` and `issue_releases` for tracked releases.
    if !release_agg.is_empty() {
        let rel_rows: Vec<ReleaseUpsertRow> = release_agg
            .into_iter()
            .map(|((tenant_id, project_id, version), agg)| {
                let commit_sha = infer_commit_sha(&version);
                ReleaseUpsertRow {
                    tenant_id,
                    project_id,
                    version,
                    commit_sha,
                    environment: agg.environment,
                    first_seen: agg.first_seen,
                    last_seen: agg.last_seen,
                    event_count: agg.event_count,
                    new_issues_count: agg.new_issues_count,
                }
            })
            .collect();

        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO releases ( \
                tenant_id, project_id, version, commit_sha, environment, \
                first_seen, last_seen, event_count, new_issues_count \
             ) ",
        );
        qb.push_values(&rel_rows, |mut b, row| {
            b.push_bind(row.tenant_id)
                .push_bind(row.project_id)
                .push_bind(&row.version)
                .push_bind(&row.commit_sha)
                .push_bind(&row.environment)
                .push_bind(row.first_seen)
                .push_bind(row.last_seen)
                .push_bind(row.event_count)
                .push_bind(row.new_issues_count);
        });
        qb.push(
            " ON CONFLICT (tenant_id, project_id, version) DO UPDATE SET \
                commit_sha = COALESCE(releases.commit_sha, EXCLUDED.commit_sha), \
                environment = COALESCE(EXCLUDED.environment, releases.environment), \
                first_seen = LEAST(releases.first_seen, EXCLUDED.first_seen), \
                last_seen = GREATEST(releases.last_seen, EXCLUDED.last_seen), \
                event_count = releases.event_count + EXCLUDED.event_count, \
                new_issues_count = releases.new_issues_count + EXCLUDED.new_issues_count, \
                updated_at = NOW()",
        );
        qb.build()
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("batch upsert releases: {err}")))?;
    }

    if !issue_release_agg.is_empty() {
        let ir_rows: Vec<IssueReleaseUpsertRow> = issue_release_agg
            .into_iter()
            .map(
                |((tenant_id, project_id, issue_id, version), agg)| IssueReleaseUpsertRow {
                    tenant_id,
                    project_id,
                    issue_id,
                    version,
                    first_seen: agg.first_seen,
                    last_seen: agg.last_seen,
                    event_count: agg.event_count,
                },
            )
            .collect();

        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO issue_releases ( \
                tenant_id, project_id, issue_id, release_version, first_seen, last_seen, event_count \
             ) ",
        );
        qb.push_values(&ir_rows, |mut b, row| {
            b.push_bind(row.tenant_id)
                .push_bind(row.project_id)
                .push_bind(row.issue_id)
                .push_bind(&row.version)
                .push_bind(row.first_seen)
                .push_bind(row.last_seen)
                .push_bind(row.event_count);
        });
        qb.push(
            " ON CONFLICT (tenant_id, project_id, issue_id, release_version) DO UPDATE SET \
                first_seen = LEAST(issue_releases.first_seen, EXCLUDED.first_seen), \
                last_seen = GREATEST(issue_releases.last_seen, EXCLUDED.last_seen), \
                event_count = issue_releases.event_count + EXCLUDED.event_count",
        );
        qb.build().execute(&mut *tx).await.map_err(|err| {
            AppError::DatabaseQuery(format!("batch upsert issue_releases: {err}"))
        })?;
    }

    // 4. Multi-row batch insert into `events` (for events under the per-issue cap + sample).
    if !events_to_insert.is_empty() {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO events ( \
                tenant_id, project_id, issue_id, event_id, fingerprint, received_at, \
                environment, release, level, platform, exception_type, \
                exception_value, stack_frames, tags, payload, is_sample \
             ) ",
        );
        qb.push_values(&events_to_insert, |mut b, ev| {
            b.push_bind(ev.tenant_id)
                .push_bind(ev.project_id)
                .push_bind(ev.issue_id)
                .push_bind(&ev.event_id)
                .push_bind(&ev.fingerprint)
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

    Ok((
        BatchFlushStats {
            processed_events: batch.len(),
            upserted_issues: grouped.len(),
            regressed_issues,
            inserted_full_events: events_to_insert.len(),
            updated_hourly_buckets: hourly_increments.len(),
        },
        alert_signals,
    ))
}

struct ReleaseUpsertRow {
    tenant_id: i64,
    project_id: i64,
    version: String,
    commit_sha: Option<String>,
    environment: Option<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    event_count: i64,
    new_issues_count: i64,
}

struct IssueReleaseUpsertRow {
    tenant_id: i64,
    project_id: i64,
    issue_id: i64,
    version: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    event_count: i64,
}

struct ReleaseAgg {
    environment: Option<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    event_count: i64,
    new_issues_count: i64,
}

struct IssueReleaseAgg {
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    event_count: i64,
}

/// If a release version looks like a 7..=40 hex Git commit SHA (or ends with `@<sha>`),
/// infer `commit_sha` automatically so releases map directly to commits.
fn infer_commit_sha(version: &str) -> Option<String> {
    let candidate = version.rsplit('@').next().unwrap_or(version).trim();
    if (7..=40).contains(&candidate.len()) && candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(candidate.to_ascii_lowercase())
    } else {
        None
    }
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
