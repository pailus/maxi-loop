use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub::scrub_event_payload_with_rules;
use maxi_loop_platform::scrub_notify::PiiScrubRuleEntry;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::codec::compress_trace_blob;
use crate::detectors::{DetectorSpan, detect_performance_problems};
use crate::histogram::{HistogramBuckets, NUM_HISTOGRAM_BUCKETS, observe_duration};
use crate::model::{
    ProcessedSpanOp, ProcessedTransaction, QueuedTransaction, SampleReason, TraceBlobPayload,
    TraceSpanRecord, TransactionFlushStats,
};
use crate::normalize::normalize_span_operation;

/// Matches the Sentry SDK default `max_spans` (1000), so every span an SDK sends is analyzed.
const MAX_SPANS_PER_TRANSACTION: usize = 1000;
const MAX_TRANSACTION_NAME_LEN: usize = 512;
/// Stored (displayed) span description length.
const MAX_SPAN_DESC_LEN: usize = 512;
/// Input bound for grouping. Larger than the stored length because ORM queries list every
/// column first and only differ in the `WHERE` clause far past 512 characters.
const MAX_SPAN_DESC_GROUPING_LEN: usize = 8192;

#[derive(Clone, Copy, Debug)]
pub struct TraceSamplingPolicy {
    pub slow_threshold_ms: f64,
    pub random_sample_rate: f64,
}

impl Default for TraceSamplingPolicy {
    fn default() -> Self {
        Self {
            slow_threshold_ms: 500.0,
            random_sample_rate: 0.05,
        }
    }
}

/// Truncate a UTC timestamp to the start of its UTC minute (`YYYY-MM-DDTHH:MM:00Z`).
pub fn truncate_to_minute_utc(ts: DateTime<Utc>) -> DateTime<Utc> {
    let secs = ts.timestamp();
    let min_secs = secs - secs.rem_euclid(60);
    DateTime::<Utc>::from_timestamp(min_secs, 0).unwrap_or(ts)
}

/// Deterministic head-consistent sampling policy (`F-PRF-4`, Design §5.5):
/// 1. Error transactions -> always sampled (`SampleReason::Error`)
/// 2. Slow transactions (`duration_ms >= slow_threshold_ms`) -> always sampled (`SampleReason::Slow`)
/// 3. Otherwise, hash `trace_id` deterministically so all distributed services sharing the same
///    `trace_id` make the same random 1–5% sampling decision (`SampleReason::Random`).
pub fn decide_sample_reason(
    trace_id: &str,
    duration_ms: f64,
    is_error: bool,
    policy: TraceSamplingPolicy,
) -> Option<SampleReason> {
    if is_error {
        return Some(SampleReason::Error);
    }
    if duration_ms >= policy.slow_threshold_ms {
        return Some(SampleReason::Slow);
    }
    let rate = policy.random_sample_rate.clamp(0.0, 1.0);
    if rate <= 0.0 {
        return None;
    }
    if rate >= 1.0 {
        return Some(SampleReason::Random);
    }

    let digest = Sha256::digest(trace_id.as_bytes());
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    let bucket = (u64::from_be_bytes(prefix) as f64) / (u64::MAX as f64);
    if bucket < rate {
        Some(SampleReason::Random)
    } else {
        None
    }
}

/// Parse, PII-scrub, normalize, sample, and (if sampled) zstd-compress a raw Sentry `transaction`.
pub fn process_queued_transaction_with_rules(
    raw: QueuedTransaction,
    custom_rules: &[PiiScrubRuleEntry],
    policy: TraceSamplingPolicy,
) -> Result<ProcessedTransaction, AppError> {
    let mut payload: Value = serde_json::from_slice(&raw.raw_payload)
        .map_err(|_| AppError::Validation("invalid transaction json payload".to_owned()))?;

    // Mandatory baseline + inherited custom PII scrubbing BEFORE any field extraction or storage.
    scrub_event_payload_with_rules(&mut payload, custom_rules);

    let trace_ctx = payload
        .get("contexts")
        .and_then(|c| c.get("trace"))
        .and_then(Value::as_object);

    let trace_id = trace_ctx
        .and_then(|t| t.get("trace_id"))
        .and_then(Value::as_str)
        .map(normalize_hex_id)
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .unwrap_or_else(|| derive_deterministic_hex(&raw.event_id, 32));

    let root_span_id = trace_ctx
        .and_then(|t| t.get("span_id"))
        .and_then(Value::as_str)
        .map(normalize_hex_id)
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .unwrap_or_else(|| derive_deterministic_hex(&format!("{}:root", raw.event_id), 16));

    let parent_span_id = trace_ctx
        .and_then(|t| t.get("parent_span_id"))
        .and_then(Value::as_str)
        .map(normalize_hex_id)
        .filter(|s| !s.is_empty() && s.len() <= 64);

    let op = trace_ctx
        .and_then(|t| t.get("op"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_chars(s, 128))
        .unwrap_or_else(|| "transaction".to_owned());

    let status = trace_ctx
        .and_then(|t| t.get("status"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .map(|s| truncate_chars(&s, 64))
        .unwrap_or_else(|| "ok".to_owned());

    let transaction_name = payload
        .get("transaction")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            trace_ctx
                .and_then(|t| t.get("description"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .map(|s| truncate_chars(s, MAX_TRANSACTION_NAME_LEN))
        .unwrap_or_else(|| "<unlabeled transaction>".to_owned());

    let end_ts = payload
        .get("timestamp")
        .and_then(parse_sentry_timestamp)
        .unwrap_or(raw.received_at);
    let start_ts = payload
        .get("start_timestamp")
        .and_then(parse_sentry_timestamp)
        .unwrap_or(end_ts);
    let (start_timestamp, end_timestamp) = if end_ts < start_ts {
        (end_ts, start_ts)
    } else {
        (start_ts, end_ts)
    };

    let duration_ms =
        ((end_timestamp.timestamp_micros() - start_timestamp.timestamp_micros()) as f64 / 1000.0)
            .max(0.0);

    let tags = normalize_tags(payload.get("tags"));
    let http_status_code = extract_http_status_code(&tags, trace_ctx);
    let is_error = is_error_status(&status, http_status_code);

    let raw_spans = payload
        .get("spans")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let span_count = i32::try_from(raw_spans.len().min(MAX_SPANS_PER_TRANSACTION)).unwrap_or(0);

    let root_start_ms = timestamp_to_ms(start_timestamp);
    let root_end_ms = timestamp_to_ms(end_timestamp);
    let bounded_span_len = raw_spans.len().min(MAX_SPANS_PER_TRANSACTION);
    // Spans are always parsed (bounded) because performance issue detection must see every
    // transaction, not only sampled ones; they are dropped below when not sampled.
    let mut parsed_spans = Vec::with_capacity(bounded_span_len);
    let mut span_norms: Vec<Option<(String, String)>> = Vec::with_capacity(bounded_span_len);
    let mut span_ops = Vec::with_capacity(bounded_span_len);

    for (idx, span_val) in raw_spans.iter().take(MAX_SPANS_PER_TRANSACTION).enumerate() {
        let Some(obj) = span_val.as_object() else {
            continue;
        };

        let span_op = obj
            .get("op")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate_chars(s, 128))
            .unwrap_or_else(|| "span".to_owned());

        let raw_desc = obj
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let span_desc = truncate_chars(raw_desc, MAX_SPAN_DESC_LEN);

        let span_status = obj
            .get("status")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .map(|s| truncate_chars(&s, 64))
            .unwrap_or_else(|| "ok".to_owned());

        let s_start_ms = obj
            .get("start_timestamp")
            .and_then(parse_sentry_timestamp)
            .map(timestamp_to_ms)
            .unwrap_or(root_start_ms);
        let s_end_ms = obj
            .get("timestamp")
            .and_then(parse_sentry_timestamp)
            .map(timestamp_to_ms)
            .unwrap_or(root_end_ms.max(s_start_ms));
        let (s_start_ms, s_end_ms) = if s_end_ms < s_start_ms {
            (s_end_ms, s_start_ms)
        } else {
            (s_start_ms, s_end_ms)
        };
        let s_dur = (s_end_ms - s_start_ms).max(0.0);

        let span_tags = normalize_tags(obj.get("tags"));
        let span_data = obj.get("data").cloned().unwrap_or_else(|| json!({}));
        let span_http_code = extract_http_status_code(&span_tags, span_data.as_object());
        let span_is_err = is_error_status(&span_status, span_http_code);

        let normalized = normalize_span_operation(
            &span_op,
            &truncate_chars(raw_desc, MAX_SPAN_DESC_GROUPING_LEN),
        );
        if let Some((normalized_description, description_hash)) = &normalized {
            span_ops.push(ProcessedSpanOp {
                op: span_op.clone(),
                description_hash: description_hash.clone(),
                normalized_description: normalized_description.clone(),
                duration_ms: s_dur,
                is_error: span_is_err,
            });
        }
        span_norms.push(normalized);

        let span_id = obj
            .get("span_id")
            .and_then(Value::as_str)
            .map(normalize_hex_id)
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .unwrap_or_else(|| {
                derive_deterministic_hex(&format!("{}:span:{idx}", raw.event_id), 16)
            });
        let span_parent = obj
            .get("parent_span_id")
            .and_then(Value::as_str)
            .map(normalize_hex_id)
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .or_else(|| Some(root_span_id.clone()));

        parsed_spans.push(TraceSpanRecord {
            span_id,
            parent_span_id: span_parent,
            op: span_op,
            description: span_desc,
            status: span_status,
            is_error: span_is_err,
            start_timestamp_ms: s_start_ms,
            end_timestamp_ms: s_end_ms,
            duration_ms: s_dur,
            tags: span_tags,
            data: span_data,
        });
    }

    let detector_spans: Vec<DetectorSpan<'_>> = parsed_spans
        .iter()
        .zip(&span_norms)
        .map(|(span, norm)| DetectorSpan {
            span_id: &span.span_id,
            parent_span_id: span.parent_span_id.as_deref(),
            op: &span.op,
            normalized: norm.as_ref().map(|(d, h)| (d.as_str(), h.as_str())),
            start_ms: span.start_timestamp_ms,
            end_ms: span.end_timestamp_ms,
            duration_ms: span.duration_ms,
        })
        .collect();
    let performance_problems = detect_performance_problems(&detector_spans, root_start_ms);

    let sample_reason = decide_sample_reason(&trace_id, duration_ms, is_error, policy).or(
        if performance_problems.is_empty() {
            None
        } else {
            Some(SampleReason::Issue)
        },
    );

    let spans_zstd = if sample_reason.is_some() {
        let blob = TraceBlobPayload {
            spans: parsed_spans,
            request: payload.get("request").cloned(),
            measurements: payload.get("measurements").cloned(),
            contexts: trace_ctx
                .and_then(|t| t.get("data"))
                .cloned()
                .map(|d| json!({ "trace_data": d })),
        };
        Some(compress_trace_blob(&blob)?)
    } else {
        None
    };

    Ok(ProcessedTransaction {
        tenant_id: raw.tenant_id,
        workspace_id: raw.workspace_id,
        project_id: raw.project_id,
        event_id: raw.event_id,
        received_at: raw.received_at,
        trace_id,
        root_span_id,
        parent_span_id,
        transaction_name,
        op,
        status,
        is_error,
        environment: raw.environment,
        release: raw.release,
        platform: raw.platform,
        start_timestamp,
        end_timestamp,
        duration_ms,
        span_count,
        tags,
        sample_reason,
        spans_zstd,
        span_ops,
        performance_problems,
    })
}

/// Flush a batch of processed transactions inside an active `maxi_loop_worker` transaction:
/// 1. Ensure daily `traces_YYYY_MM_DD` partitions exist (checking `to_regclass` first).
/// 2. Multi-row batch upsert 1-minute aggregated metrics & duration histograms into `transaction_minute_metrics`.
/// 3. Multi-row batch insert sampled zstd-compressed traces into `traces`.
/// 4. Upsert any seen `releases` versions so performance releases are tracked too.
pub async fn flush_processed_transactions_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    batch: &[ProcessedTransaction],
) -> Result<TransactionFlushStats, AppError> {
    if batch.is_empty() {
        return Ok(TransactionFlushStats::default());
    }

    // 1. Ensure daily partitions exist for any sampled trace dates in this batch.
    let sampled_dates: BTreeSet<NaiveDate> = batch
        .iter()
        .filter(|t| t.spans_zstd.is_some())
        .map(|t| t.received_at.date_naive())
        .collect();

    for target_date in sampled_dates {
        let part_regclass = format!("public.traces_{}", target_date.format("%Y_%m_%d"));
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(&part_regclass)
            .fetch_one(&mut **tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("check partition {part_regclass}: {err}"))
            })?;
        if !exists {
            sqlx::query("SELECT ensure_traces_partition($1)")
                .bind(target_date)
                .execute(&mut **tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!(
                        "ensure_traces_partition({target_date}): {err}"
                    ))
                })?;
        }
    }

    // 2. Aggregate 1-minute duration histograms in memory before single multi-row upsert.
    type BucketKey = (i64, i64, String, String, String, String, DateTime<Utc>);
    type SpanBucketKey = (i64, i64, String, String, String, String, DateTime<Utc>);
    let mut agg_buckets: BTreeMap<BucketKey, MinuteBucketAgg> = BTreeMap::new();
    let mut span_agg_buckets: BTreeMap<SpanBucketKey, SpanMinuteBucketAgg> = BTreeMap::new();
    let mut release_agg: BTreeMap<(i64, i64, String), PerfReleaseAgg> = BTreeMap::new();

    for item in batch {
        let minute_bucket = truncate_to_minute_utc(item.start_timestamp);
        let rel_key = item.release.clone().unwrap_or_default();
        let entry = agg_buckets
            .entry((
                item.tenant_id,
                item.project_id,
                item.transaction_name.clone(),
                item.op.clone(),
                item.environment.clone(),
                rel_key.clone(),
                minute_bucket,
            ))
            .or_insert_with(|| MinuteBucketAgg {
                total_count: 0,
                error_count: 0,
                duration_sum_ms: 0.0,
                duration_min_ms: item.duration_ms,
                duration_max_ms: item.duration_ms,
                histogram: [0i64; NUM_HISTOGRAM_BUCKETS],
            });

        entry.total_count += 1;
        if item.is_error {
            entry.error_count += 1;
        }
        entry.duration_sum_ms += item.duration_ms;
        entry.duration_min_ms = entry.duration_min_ms.min(item.duration_ms);
        entry.duration_max_ms = entry.duration_max_ms.max(item.duration_ms);
        observe_duration(&mut entry.histogram, item.duration_ms);

        // Aggregate normalized span operations (`F-PRF-7`) across ALL transactions.
        let sampled_trace_ref = if item.sample_reason.is_some() && item.spans_zstd.is_some() {
            Some(item.trace_id.as_str())
        } else {
            None
        };
        for span_op in &item.span_ops {
            let sp_entry = span_agg_buckets
                .entry((
                    item.tenant_id,
                    item.project_id,
                    span_op.op.clone(),
                    span_op.description_hash.clone(),
                    item.environment.clone(),
                    rel_key.clone(),
                    minute_bucket,
                ))
                .or_insert_with(|| SpanMinuteBucketAgg {
                    normalized_description: span_op.normalized_description.clone(),
                    total_count: 0,
                    error_count: 0,
                    duration_sum_ms: 0.0,
                    duration_min_ms: span_op.duration_ms,
                    duration_max_ms: span_op.duration_ms,
                    histogram: [0i64; NUM_HISTOGRAM_BUCKETS],
                    sample_trace_id: sampled_trace_ref.map(str::to_owned),
                });
            sp_entry.total_count += 1;
            if span_op.is_error {
                sp_entry.error_count += 1;
            }
            sp_entry.duration_sum_ms += span_op.duration_ms;
            sp_entry.duration_min_ms = sp_entry.duration_min_ms.min(span_op.duration_ms);
            sp_entry.duration_max_ms = sp_entry.duration_max_ms.max(span_op.duration_ms);
            observe_duration(&mut sp_entry.histogram, span_op.duration_ms);
            if let Some(tid) = sampled_trace_ref {
                sp_entry.sample_trace_id = Some(tid.to_owned());
            }
        }

        if let Some(version) = item.release.as_deref() {
            let rel = release_agg
                .entry((item.tenant_id, item.project_id, version.to_owned()))
                .or_insert_with(|| PerfReleaseAgg {
                    environment: Some(item.environment.clone()),
                    first_seen: item.received_at,
                    last_seen: item.received_at,
                    event_count: 0,
                });
            rel.first_seen = rel.first_seen.min(item.received_at);
            if item.received_at >= rel.last_seen {
                rel.last_seen = item.received_at;
                rel.environment = Some(item.environment.clone());
            }
            rel.event_count += 1;
        }
    }

    let upserted_minute_buckets = agg_buckets.len();
    if !agg_buckets.is_empty() {
        let rows: Vec<MinuteUpsertRow> = agg_buckets
            .into_iter()
            .map(
                |(
                    (
                        tenant_id,
                        project_id,
                        transaction_name,
                        op,
                        environment,
                        release,
                        minute_bucket,
                    ),
                    agg,
                )| MinuteUpsertRow {
                    tenant_id,
                    project_id,
                    transaction_name,
                    op,
                    environment,
                    release,
                    minute_bucket,
                    total_count: agg.total_count,
                    error_count: agg.error_count,
                    duration_sum_ms: agg.duration_sum_ms,
                    duration_min_ms: agg.duration_min_ms,
                    duration_max_ms: agg.duration_max_ms,
                    histogram_buckets: agg.histogram.to_vec(),
                },
            )
            .collect();

        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO transaction_minute_metrics ( \
                tenant_id, project_id, transaction_name, op, environment, release, minute_bucket, \
                total_count, error_count, duration_sum_ms, duration_min_ms, duration_max_ms, \
                histogram_buckets \
             ) ",
        );
        qb.push_values(&rows, |mut b, row| {
            b.push_bind(row.tenant_id)
                .push_bind(row.project_id)
                .push_bind(&row.transaction_name)
                .push_bind(&row.op)
                .push_bind(&row.environment)
                .push_bind(&row.release)
                .push_bind(row.minute_bucket)
                .push_bind(row.total_count)
                .push_bind(row.error_count)
                .push_bind(row.duration_sum_ms)
                .push_bind(row.duration_min_ms)
                .push_bind(row.duration_max_ms)
                .push_bind(&row.histogram_buckets);
        });
        qb.push(
            " ON CONFLICT (tenant_id, project_id, transaction_name, op, environment, release, minute_bucket) \
              DO UPDATE SET \
                total_count = transaction_minute_metrics.total_count + EXCLUDED.total_count, \
                error_count = transaction_minute_metrics.error_count + EXCLUDED.error_count, \
                duration_sum_ms = transaction_minute_metrics.duration_sum_ms + EXCLUDED.duration_sum_ms, \
                duration_min_ms = LEAST(transaction_minute_metrics.duration_min_ms, EXCLUDED.duration_min_ms), \
                duration_max_ms = GREATEST(transaction_minute_metrics.duration_max_ms, EXCLUDED.duration_max_ms), \
                histogram_buckets = merge_histogram_buckets( \
                    transaction_minute_metrics.histogram_buckets, \
                    EXCLUDED.histogram_buckets \
                )",
        );
        qb.build().execute(&mut **tx).await.map_err(|err| {
            AppError::DatabaseQuery(format!("batch upsert transaction_minute_metrics: {err}"))
        })?;
    }

    // 2b. Batch upsert 1-minute aggregated span operations (`span_minute_metrics`, `F-PRF-7`).
    let upserted_span_buckets = span_agg_buckets.len();
    if !span_agg_buckets.is_empty() {
        let rows: Vec<SpanMinuteUpsertRow> = span_agg_buckets
            .into_iter()
            .map(
                |(
                    (
                        tenant_id,
                        project_id,
                        op,
                        description_hash,
                        environment,
                        release,
                        minute_bucket,
                    ),
                    agg,
                )| SpanMinuteUpsertRow {
                    tenant_id,
                    project_id,
                    op,
                    description_hash,
                    normalized_description: agg.normalized_description,
                    environment,
                    release,
                    minute_bucket,
                    total_count: agg.total_count,
                    error_count: agg.error_count,
                    duration_sum_ms: agg.duration_sum_ms,
                    duration_min_ms: agg.duration_min_ms,
                    duration_max_ms: agg.duration_max_ms,
                    histogram_buckets: agg.histogram.to_vec(),
                    sample_trace_id: agg.sample_trace_id,
                },
            )
            .collect();

        // Chunk upserts to stay comfortably below PostgreSQL's 65535 parameter limit (15 params/row).
        for chunk in rows.chunks(2000) {
            let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
                "INSERT INTO span_minute_metrics ( \
                    tenant_id, project_id, op, description_hash, normalized_description, \
                    environment, release, minute_bucket, total_count, error_count, \
                    duration_sum_ms, duration_min_ms, duration_max_ms, histogram_buckets, \
                    sample_trace_id \
                 ) ",
            );
            qb.push_values(chunk, |mut b, row| {
                b.push_bind(row.tenant_id)
                    .push_bind(row.project_id)
                    .push_bind(&row.op)
                    .push_bind(&row.description_hash)
                    .push_bind(&row.normalized_description)
                    .push_bind(&row.environment)
                    .push_bind(&row.release)
                    .push_bind(row.minute_bucket)
                    .push_bind(row.total_count)
                    .push_bind(row.error_count)
                    .push_bind(row.duration_sum_ms)
                    .push_bind(row.duration_min_ms)
                    .push_bind(row.duration_max_ms)
                    .push_bind(&row.histogram_buckets)
                    .push_bind(&row.sample_trace_id);
            });
            qb.push(
                " ON CONFLICT (tenant_id, project_id, op, description_hash, environment, release, minute_bucket) \
                  DO UPDATE SET \
                    total_count = span_minute_metrics.total_count + EXCLUDED.total_count, \
                    error_count = span_minute_metrics.error_count + EXCLUDED.error_count, \
                    duration_sum_ms = span_minute_metrics.duration_sum_ms + EXCLUDED.duration_sum_ms, \
                    duration_min_ms = LEAST(span_minute_metrics.duration_min_ms, EXCLUDED.duration_min_ms), \
                    duration_max_ms = GREATEST(span_minute_metrics.duration_max_ms, EXCLUDED.duration_max_ms), \
                    histogram_buckets = merge_histogram_buckets( \
                        span_minute_metrics.histogram_buckets, \
                        EXCLUDED.histogram_buckets \
                    ), \
                    sample_trace_id = COALESCE(EXCLUDED.sample_trace_id, span_minute_metrics.sample_trace_id)",
            );
            qb.build().execute(&mut **tx).await.map_err(|err| {
                AppError::DatabaseQuery(format!("batch upsert span_minute_metrics: {err}"))
            })?;
        }
    }

    // 3. Batch insert sampled traces with zstd-compressed span blobs.
    let sampled: Vec<&ProcessedTransaction> = batch
        .iter()
        .filter(|t| t.sample_reason.is_some() && t.spans_zstd.is_some())
        .collect();
    let inserted_sampled_traces = sampled.len();
    if !sampled.is_empty() {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO traces ( \
                tenant_id, project_id, received_at, event_id, trace_id, root_span_id, \
                parent_span_id, transaction_name, op, status, is_error, sample_reason, \
                environment, release, platform, start_timestamp, end_timestamp, \
                duration_ms, span_count, tags, spans_zstd \
             ) ",
        );
        qb.push_values(&sampled, |mut b, item| {
            let reason = item
                .sample_reason
                .map(SampleReason::as_str)
                .unwrap_or("random");
            let blob = item.spans_zstd.as_deref().unwrap_or(&[]);
            b.push_bind(item.tenant_id)
                .push_bind(item.project_id)
                .push_bind(item.received_at)
                .push_bind(&item.event_id)
                .push_bind(&item.trace_id)
                .push_bind(&item.root_span_id)
                .push_bind(&item.parent_span_id)
                .push_bind(&item.transaction_name)
                .push_bind(&item.op)
                .push_bind(&item.status)
                .push_bind(item.is_error)
                .push_bind(reason)
                .push_bind(&item.environment)
                .push_bind(&item.release)
                .push_bind(&item.platform)
                .push_bind(item.start_timestamp)
                .push_bind(item.end_timestamp)
                .push_bind(item.duration_ms)
                .push_bind(item.span_count)
                .push_bind(&item.tags)
                .push_bind(blob);
        });
        qb.push(" ON CONFLICT (tenant_id, project_id, received_at, event_id) DO NOTHING");
        qb.build()
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("batch insert traces: {err}")))?;
    }

    // 4. Track release versions observed in transactions.
    if !release_agg.is_empty() {
        let rows: Vec<PerfReleaseUpsertRow> = release_agg
            .into_iter()
            .map(|((tenant_id, project_id, version), agg)| {
                let commit_sha = infer_commit_sha(&version);
                PerfReleaseUpsertRow {
                    tenant_id,
                    project_id,
                    version,
                    commit_sha,
                    environment: agg.environment,
                    first_seen: agg.first_seen,
                    last_seen: agg.last_seen,
                    event_count: agg.event_count,
                }
            })
            .collect();

        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO releases ( \
                tenant_id, project_id, version, commit_sha, environment, \
                first_seen, last_seen, event_count, new_issues_count \
             ) ",
        );
        qb.push_values(&rows, |mut b, row| {
            b.push_bind(row.tenant_id)
                .push_bind(row.project_id)
                .push_bind(&row.version)
                .push_bind(&row.commit_sha)
                .push_bind(&row.environment)
                .push_bind(row.first_seen)
                .push_bind(row.last_seen)
                .push_bind(row.event_count)
                .push_bind(0i64);
        });
        qb.push(
            " ON CONFLICT (tenant_id, project_id, version) DO UPDATE SET \
                commit_sha = COALESCE(releases.commit_sha, EXCLUDED.commit_sha), \
                environment = COALESCE(EXCLUDED.environment, releases.environment), \
                first_seen = LEAST(releases.first_seen, EXCLUDED.first_seen), \
                last_seen = GREATEST(releases.last_seen, EXCLUDED.last_seen), \
                event_count = releases.event_count + EXCLUDED.event_count, \
                updated_at = NOW()",
        );
        qb.build().execute(&mut **tx).await.map_err(|err| {
            AppError::DatabaseQuery(format!("batch upsert performance releases: {err}"))
        })?;
    }

    Ok(TransactionFlushStats {
        processed_transactions: batch.len(),
        upserted_minute_buckets,
        upserted_span_buckets,
        inserted_sampled_traces,
    })
}

struct MinuteBucketAgg {
    total_count: i64,
    error_count: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram: HistogramBuckets,
}

struct SpanMinuteBucketAgg {
    normalized_description: String,
    total_count: i64,
    error_count: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram: HistogramBuckets,
    sample_trace_id: Option<String>,
}

struct SpanMinuteUpsertRow {
    tenant_id: i64,
    project_id: i64,
    op: String,
    description_hash: String,
    normalized_description: String,
    environment: String,
    release: String,
    minute_bucket: DateTime<Utc>,
    total_count: i64,
    error_count: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
    sample_trace_id: Option<String>,
}

struct MinuteUpsertRow {
    tenant_id: i64,
    project_id: i64,
    transaction_name: String,
    op: String,
    environment: String,
    release: String,
    minute_bucket: DateTime<Utc>,
    total_count: i64,
    error_count: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
}

struct PerfReleaseAgg {
    environment: Option<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    event_count: i64,
}

struct PerfReleaseUpsertRow {
    tenant_id: i64,
    project_id: i64,
    version: String,
    commit_sha: Option<String>,
    environment: Option<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    event_count: i64,
}

fn parse_sentry_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::Number(num) => {
            let secs_f = num.as_f64()?;
            if !secs_f.is_finite() || secs_f < 0.0 {
                return None;
            }
            let whole = secs_f.trunc() as i64;
            let nanos = ((secs_f - whole as f64) * 1_000_000_000.0).round() as u32;
            DateTime::<Utc>::from_timestamp(whole, nanos.min(999_999_999))
        }
        Value::String(s) => DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|dt| dt.with_timezone(&Utc)),
        _ => None,
    }
}

pub fn timestamp_to_ms(ts: DateTime<Utc>) -> f64 {
    (ts.timestamp_micros() as f64) / 1000.0
}

fn normalize_hex_id(raw: &str) -> String {
    raw.trim().replace('-', "").to_ascii_lowercase()
}

fn derive_deterministic_hex(seed: &str, len: usize) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let encoded = hex::encode(digest);
    encoded.chars().take(len).collect()
}

fn truncate_chars(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        raw.to_owned()
    } else {
        raw.chars().take(max_chars).collect()
    }
}

fn normalize_tags(raw: Option<&Value>) -> Value {
    let mut out = Map::new();
    match raw {
        Some(Value::Object(map)) => {
            for (k, v) in map.iter().take(64) {
                let key = k.trim();
                if key.is_empty() || key.len() > 64 {
                    continue;
                }
                let val_str = match v {
                    Value::String(s) => truncate_chars(s.trim(), 200),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                out.insert(key.to_owned(), Value::String(val_str));
            }
        }
        Some(Value::Array(items)) => {
            for item in items.iter().take(64) {
                if let Some(pair) = item.as_array()
                    && pair.len() == 2
                    && let Some(k) = pair[0].as_str()
                {
                    let key = k.trim();
                    if key.is_empty() || key.len() > 64 {
                        continue;
                    }
                    if let Some(v) = pair[1].as_str() {
                        out.insert(key.to_owned(), Value::String(truncate_chars(v.trim(), 200)));
                    }
                }
            }
        }
        _ => {}
    }
    Value::Object(out)
}

fn extract_http_status_code(tags: &Value, ctx: Option<&Map<String, Value>>) -> Option<u16> {
    if let Some(code_str) = tags
        .get("http.status_code")
        .or_else(|| tags.get("http.response.status_code"))
        .and_then(Value::as_str)
        && let Ok(code) = code_str.parse::<u16>()
    {
        return Some(code);
    }
    if let Some(map) = ctx {
        for candidate in [Some(map), map.get("data").and_then(Value::as_object)]
            .into_iter()
            .flatten()
        {
            for key in ["http.status_code", "http.response.status_code"] {
                if let Some(v) = candidate.get(key) {
                    if let Some(n) = v.as_u64() {
                        return u16::try_from(n).ok();
                    }
                    if let Some(s) = v.as_str()
                        && let Ok(code) = s.parse::<u16>()
                    {
                        return Some(code);
                    }
                }
            }
        }
    }
    None
}

fn is_error_status(status: &str, http_status_code: Option<u16>) -> bool {
    if let Some(code) = http_status_code
        && code >= 500
    {
        return true;
    }
    !matches!(status, "" | "ok" | "unknown" | "cancelled")
}

pub(crate) fn infer_commit_sha(version: &str) -> Option<String> {
    let candidate = version.rsplit('@').next().unwrap_or(version).trim();
    if (7..=40).contains(&candidate.len()) && candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(candidate.to_ascii_lowercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_policy_prioritizes_error_then_slow_then_deterministic_random() {
        let policy = TraceSamplingPolicy {
            slow_threshold_ms: 500.0,
            random_sample_rate: 0.05,
        };

        assert_eq!(
            decide_sample_reason("aaaa", 20.0, true, policy),
            Some(SampleReason::Error)
        );
        assert_eq!(
            decide_sample_reason("aaaa", 750.0, false, policy),
            Some(SampleReason::Slow)
        );
        assert_eq!(
            decide_sample_reason("aaaa", 750.0, true, policy),
            Some(SampleReason::Error)
        );

        // Deterministic head-consistent random sampling across 1000 distinct trace IDs (~5%).
        let mut sampled = 0usize;
        for i in 0..1000 {
            let tid = format!("{i:032x}");
            let first = decide_sample_reason(&tid, 25.0, false, policy);
            let second = decide_sample_reason(&tid, 25.0, false, policy);
            assert_eq!(first, second, "same trace_id must make identical decision");
            if first == Some(SampleReason::Random) {
                sampled += 1;
            }
        }
        assert!(
            (25..=80).contains(&sampled),
            "expected ~5% random sample rate out of 1000, got {sampled}"
        );
    }

    #[test]
    fn parses_transaction_scrubs_pii_and_compresses_spans_when_sampled() {
        let payload = json!({
            "event_id": "tx111111111111111111111111111111",
            "type": "transaction",
            "transaction": "GET /api/orders/{id}",
            "start_timestamp": 1_760_000_000.0,
            "timestamp": 1_760_000_000.85,
            "environment": "production",
            "release": "abc1234",
            "contexts": {
                "trace": {
                    "trace_id": "0123456789abcdef0123456789abcdef",
                    "span_id": "1111111111111111",
                    "op": "http.server",
                    "status": "ok"
                }
            },
            "request": {
                "url": "https://api.example.com/orders?api_key=secret_query_val",
                "headers": {
                    "Authorization": "Bearer super_secret_token"
                }
            },
            "spans": [
                {
                    "span_id": "2222222222222222",
                    "parent_span_id": "1111111111111111",
                    "op": "db.sql.query",
                    "description": "SELECT * FROM orders WHERE id = $1",
                    "start_timestamp": 1_760_000_000.1,
                    "timestamp": 1_760_000_000.7,
                    "status": "ok",
                    "data": { "password": "db_password_raw" }
                }
            ]
        });

        let raw = QueuedTransaction {
            tenant_id: 1,
            workspace_id: 1,
            project_id: 1,
            event_id: "tx111111111111111111111111111111".to_owned(),
            received_at: Utc::now(),
            environment: "production".to_owned(),
            release: Some("abc1234".to_owned()),
            platform: "python".to_owned(),
            raw_payload: serde_json::to_vec(&payload).unwrap(),
        };

        let processed =
            process_queued_transaction_with_rules(raw, &[], TraceSamplingPolicy::default())
                .expect("process ok");

        assert_eq!(processed.transaction_name, "GET /api/orders/{id}");
        assert_eq!(processed.op, "http.server");
        assert!((processed.duration_ms - 850.0).abs() < 1.0);
        assert_eq!(processed.sample_reason, Some(SampleReason::Slow));
        assert_eq!(processed.span_count, 1);

        let blob = crate::codec::decompress_trace_blob(processed.spans_zstd.as_ref().unwrap())
            .expect("decompress ok");
        let serialized = serde_json::to_string(&blob).unwrap();
        assert!(!serialized.contains("super_secret_token"));
        assert!(!serialized.contains("secret_query_val"));
        assert!(!serialized.contains("db_password_raw"));
        assert!(serialized.contains("[Filtered]"));
    }

    #[test]
    fn detects_n_plus_one_in_spans_beyond_the_first_five_hundred() {
        // Sentry SDKs send up to 1000 spans per transaction; an N+1 at the end of a large
        // request (e.g. serializing a long list) must still be detected.
        let base = 1_760_000_000.0_f64;
        let mut spans: Vec<Value> = (0..600)
            .map(|i| {
                json!({
                    "span_id": format!("{:016x}", i + 1),
                    "op": "template.render",
                    "description": "render row",
                    "start_timestamp": base + 0.0001 * f64::from(i),
                    "timestamp": base + 0.0001 * f64::from(i) + 0.00005,
                })
            })
            .collect();
        for i in 0..6 {
            let start = base + 0.1 + 0.02 * f64::from(i);
            spans.push(json!({
                "span_id": format!("{:016x}", 1000 + i),
                "op": "db",
                "description": format!("SELECT * FROM master_jabatan WHERE id = {}", i + 1),
                "start_timestamp": start,
                "timestamp": start + 0.015,
            }));
        }
        let payload = json!({
            "event_id": "tx333333333333333333333333333333",
            "type": "transaction",
            "transaction": "/karir/api/pegawai_mutasi/",
            "start_timestamp": base,
            "timestamp": base + 0.3,
            "contexts": {"trace": {
                "trace_id": "0123456789abcdef0123456789abcdee",
                "span_id": "aaaaaaaaaaaaaaaa",
                "op": "http.server"
            }},
            "spans": spans
        });
        let raw = QueuedTransaction {
            tenant_id: 1,
            workspace_id: 1,
            project_id: 1,
            event_id: "tx333333333333333333333333333333".to_owned(),
            received_at: Utc::now(),
            environment: "production".to_owned(),
            release: None,
            platform: "python".to_owned(),
            raw_payload: serde_json::to_vec(&payload).unwrap(),
        };

        let processed =
            process_queued_transaction_with_rules(raw, &[], TraceSamplingPolicy::default())
                .expect("process ok");

        assert_eq!(processed.span_count, 606);
        assert!(
            processed
                .performance_problems
                .iter()
                .any(|p| p.problem_type == crate::model::PerformanceProblemType::NPlusOneDb),
            "expected an N+1 DB problem, got {:?}",
            processed.performance_problems
        );
    }

    #[test]
    fn detects_5xx_error_from_nested_otel_trace_data_status_code() {
        let payload = json!({
            "event_id": "tx222222222222222222222222222222",
            "type": "transaction",
            "transaction": "/api/fastapi/fail",
            "start_timestamp": "2026-11-21T18:57:02.000000Z",
            "timestamp": "2026-11-21T18:57:02.040000Z",
            "environment": "production",
            "contexts": {
                "trace": {
                    "trace_id": "fedcba9876543210fedcba9876543210",
                    "span_id": "aaaa1111bbbb2222",
                    "op": "http.server",
                    "status": "ok",
                    "data": {
                        "http.response.status_code": 503
                    }
                }
            },
            "spans": []
        });

        let raw = QueuedTransaction {
            tenant_id: 1,
            workspace_id: 1,
            project_id: 1,
            event_id: "tx222222222222222222222222222222".to_owned(),
            received_at: Utc::now(),
            environment: "production".to_owned(),
            release: None,
            platform: "python".to_owned(),
            raw_payload: serde_json::to_vec(&payload).unwrap(),
        };

        let processed =
            process_queued_transaction_with_rules(raw, &[], TraceSamplingPolicy::default())
                .expect("process ok");

        assert!(processed.is_error);
        assert_eq!(processed.sample_reason, Some(SampleReason::Error));
    }
}
