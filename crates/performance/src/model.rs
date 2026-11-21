use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Raw Sentry `transaction` envelope item handed from ingest HTTP layer to the async worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedTransaction {
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub project_id: i64,
    pub event_id: String,
    pub received_at: DateTime<Utc>,
    pub environment: String,
    pub release: Option<String>,
    pub platform: String,
    pub raw_payload: Vec<u8>,
}

/// Sampling reason persisted on sampled traces (`F-PRF-4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleReason {
    Slow,
    Error,
    Random,
}

impl SampleReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Error => "error",
            Self::Random => "random",
        }
    }
}

/// Individual child span stored inside a zstd-compressed `TraceBlobPayload`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceSpanRecord {
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub op: String,
    pub description: String,
    pub status: String,
    pub is_error: bool,
    pub start_timestamp_ms: f64,
    pub end_timestamp_ms: f64,
    pub duration_ms: f64,
    pub tags: Value,
    pub data: Value,
}

/// JSON payload compressed with zstd and persisted in `traces.spans_zstd`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TraceBlobPayload {
    pub spans: Vec<TraceSpanRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurements: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
}

/// Normalized, PII-scrubbed transaction ready for batched metric aggregation & optional zstd trace storage.
#[derive(Clone, Debug)]
pub struct ProcessedTransaction {
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub project_id: i64,
    pub event_id: String,
    pub received_at: DateTime<Utc>,
    pub trace_id: String,
    pub root_span_id: String,
    pub parent_span_id: Option<String>,
    pub transaction_name: String,
    pub op: String,
    pub status: String,
    pub is_error: bool,
    pub environment: String,
    pub release: Option<String>,
    pub platform: String,
    pub start_timestamp: DateTime<Utc>,
    pub end_timestamp: DateTime<Utc>,
    pub duration_ms: f64,
    pub span_count: i32,
    pub tags: Value,
    pub sample_reason: Option<SampleReason>,
    pub spans_zstd: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransactionFlushStats {
    pub processed_transactions: usize,
    pub upserted_minute_buckets: usize,
    pub inserted_sampled_traces: usize,
}

/// Scope context for performance queries (shared by HTTP API and MCP tools).
#[derive(Clone, Debug)]
pub struct PerformanceScope<'a> {
    pub tenant_slug: &'a str,
    pub workspace_slug: Option<&'a str>,
    pub project_slug: Option<&'a str>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TransactionListFilter {
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub environment: Option<String>,
    pub release: Option<String>,
    pub op: Option<String>,
    pub query: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub hours: Option<u32>,
    pub sort: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionSummaryView {
    pub project_id: i64,
    pub project_slug: string_alias::Str,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub transaction_name: String,
    pub op: String,
    pub total_count: i64,
    pub error_count: i64,
    pub error_rate: f64,
    pub throughput_rpm: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub avg_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub last_seen: DateTime<Utc>,
}

mod string_alias {
    pub type Str = String;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaginatedTransactionsResponse {
    pub items: Vec<TransactionSummaryView>,
    pub total: i64,
    pub page: u32,
    pub per_page: u32,
    pub total_pages: u32,
    pub window_minutes: i64,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionTimeBucket {
    pub bucket: DateTime<Utc>,
    pub total_count: i64,
    pub error_count: i64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub avg_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionDetailView {
    pub summary: TransactionSummaryView,
    pub interval: String,
    pub series: Vec<TransactionTimeBucket>,
    pub recent_traces: Vec<TraceSummaryView>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TraceListFilter {
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub transaction_name: Option<String>,
    pub environment: Option<String>,
    pub release: Option<String>,
    pub sample_reason: Option<String>,
    pub is_error: Option<bool>,
    pub min_duration_ms: Option<f64>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub hours: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceSummaryView {
    pub trace_id: String,
    pub event_id: String,
    pub project_id: i64,
    pub project_slug: String,
    pub workspace_slug: String,
    pub transaction_name: String,
    pub op: String,
    pub status: String,
    pub is_error: bool,
    pub sample_reason: String,
    pub environment: String,
    pub release: Option<String>,
    pub platform: String,
    pub duration_ms: f64,
    pub span_count: i32,
    pub start_timestamp: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
}

/// Flattened span node ordered for hierarchical waterfall rendering (DFS tree order).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WaterfallSpanNode {
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub project_id: i64,
    pub project_slug: String,
    pub platform: String,
    pub environment: String,
    pub release: Option<String>,
    pub op: String,
    pub description: String,
    pub status: String,
    pub is_error: bool,
    pub is_transaction_root: bool,
    pub depth: u32,
    pub start_timestamp_ms: f64,
    pub end_timestamp_ms: f64,
    pub offset_ms: f64,
    pub duration_ms: f64,
    pub self_duration_ms: f64,
    pub pct_of_trace: f64,
    pub is_slow: bool,
    pub tags: Value,
    pub data: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlowSpanHighlight {
    pub span_id: String,
    pub project_slug: String,
    pub op: String,
    pub description: String,
    pub duration_ms: f64,
    pub self_duration_ms: f64,
    pub pct_of_trace: f64,
    pub is_error: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceWaterfallView {
    pub trace_id: String,
    pub root_transaction: String,
    pub status: String,
    pub is_error: bool,
    pub projects: Vec<String>,
    pub services_count: usize,
    pub sample_reasons: Vec<String>,
    pub start_timestamp: DateTime<Utc>,
    pub end_timestamp: DateTime<Utc>,
    pub total_duration_ms: f64,
    pub span_count: usize,
    pub slowest_spans: Vec<SlowSpanHighlight>,
    pub spans: Vec<WaterfallSpanNode>,
}
