use chrono::{DateTime, Utc};
use maxi_loop_platform::access::ProjectAccess;
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
    /// Kept because the transaction triggered a performance issue detector (`F017`).
    Issue,
}

impl SampleReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Error => "error",
            Self::Random => "random",
            Self::Issue => "issue",
        }
    }
}

/// Span pattern detected in a single transaction that becomes a performance issue (`F017`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceProblemType {
    NPlusOneDb,
    NPlusOneApi,
    SlowDbQuery,
    ConsecutiveDb,
    ConsecutiveHttp,
}

impl PerformanceProblemType {
    /// Value stored in `issues.issue_type` (allow-listed by a CHECK constraint, migration 0011).
    pub fn issue_type(self) -> &'static str {
        match self {
            Self::NPlusOneDb => "performance_n_plus_one_db",
            Self::NPlusOneApi => "performance_n_plus_one_api",
            Self::SlowDbQuery => "performance_slow_db_query",
            Self::ConsecutiveDb => "performance_consecutive_db",
            Self::ConsecutiveHttp => "performance_consecutive_http",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::NPlusOneDb => "N+1 Query",
            Self::NPlusOneApi => "N+1 API Call",
            Self::SlowDbQuery => "Slow DB Query",
            Self::ConsecutiveDb => "Consecutive DB Queries",
            Self::ConsecutiveHttp => "Consecutive HTTP Calls",
        }
    }
}

/// One span that contributes to a detected performance problem. Only the normalized
/// (parameter-stripped) description is kept so literals never leak into issue payloads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OffendingSpan {
    pub span_id: String,
    pub op: String,
    pub description: String,
    pub start_offset_ms: f64,
    pub duration_ms: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceProblem {
    pub problem_type: PerformanceProblemType,
    /// Stable grouping key for the offending pattern (hash of normalized span description(s)).
    pub group_hash: String,
    /// Human-readable summary of the offending span description(s).
    pub description: String,
    pub offending_span_count: usize,
    pub offending_spans_total_ms: f64,
    /// Time that could be saved by batching / parallelizing (consecutive detectors only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub potential_savings_ms: Option<f64>,
    /// Bounded sample of offending spans (see `detectors::MAX_OFFENDING_SPANS`).
    pub offending_spans: Vec<OffendingSpan>,
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

/// Normalized span operation extracted from an ingested transaction for `span_minute_metrics` (`F-PRF-7`).
#[derive(Clone, Debug)]
pub struct ProcessedSpanOp {
    pub op: String,
    pub description_hash: String,
    pub normalized_description: String,
    pub duration_ms: f64,
    pub is_error: bool,
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
    pub span_ops: Vec<ProcessedSpanOp>,
    pub performance_problems: Vec<PerformanceProblem>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransactionFlushStats {
    pub processed_transactions: usize,
    pub upserted_minute_buckets: usize,
    pub upserted_span_buckets: usize,
    pub inserted_sampled_traces: usize,
}

/// Scope context for performance queries (shared by HTTP API and MCP tools).
#[derive(Clone, Debug)]
pub struct PerformanceScope<'a> {
    pub tenant_slug: &'a str,
    pub workspace_slug: Option<&'a str>,
    pub project_slug: Option<&'a str>,
    /// Caller's project visibility (`None` = whole tenant). MCP tokens are bounded by their
    /// token scope instead and pass `None`.
    pub access: Option<ProjectAccess>,
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

// --- P2: Slow DB Queries & Operations Aggregation (`F-PRF-7`, `F-MCP-14`) ---

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SlowOperationListFilter {
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub environment: Option<String>,
    pub release: Option<String>,
    pub op: Option<String>,
    pub db_only: Option<bool>,
    pub query: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub hours: Option<u32>,
    pub sort: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlowOperationSummaryView {
    pub project_id: i64,
    pub project_slug: String,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub op: String,
    pub description_hash: String,
    pub normalized_description: String,
    pub total_count: i64,
    pub error_count: i64,
    pub error_rate: f64,
    pub total_ms: f64,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub sample_trace_id: Option<String>,
    pub last_seen: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaginatedSlowOperationsResponse {
    pub items: Vec<SlowOperationSummaryView>,
    pub total: i64,
    pub page: u32,
    pub per_page: u32,
    pub total_pages: u32,
    pub window_minutes: i64,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

// --- P2: Compare Releases (`F-PRF-6`, `F-MCP-14`) ---

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CompareReleasesFilter {
    pub project_slug: String,
    pub base_release: String,
    pub target_release: String,
    pub environment: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub hours: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleasePerfMetrics {
    pub total_count: i64,
    pub error_count: i64,
    pub error_rate: f64,
    pub active_minutes: i64,
    pub throughput_rpm: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub avg_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseIssueMetrics {
    pub linked_issues_count: i64,
    pub new_issues_count: i64,
    pub regressed_issues_count: i64,
    pub unresolved_issues_count: i64,
    pub issue_events_count: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseComparisonSide {
    pub version: String,
    pub commit_sha: Option<String>,
    pub first_seen: Option<DateTime<Utc>>,
    pub last_seen: Option<DateTime<Utc>>,
    pub deployed_at: Option<DateTime<Utc>>,
    pub performance: ReleasePerfMetrics,
    pub issues: ReleaseIssueMetrics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseComparisonDelta {
    pub p50_delta_ms: f64,
    pub p50_delta_pct: Option<f64>,
    pub p95_delta_ms: f64,
    pub p95_delta_pct: Option<f64>,
    pub p99_delta_ms: f64,
    pub p99_delta_pct: Option<f64>,
    pub avg_delta_ms: f64,
    pub avg_delta_pct: Option<f64>,
    pub error_rate_delta: f64,
    pub throughput_rpm_delta: f64,
    pub throughput_rpm_delta_pct: Option<f64>,
    pub linked_issues_delta: i64,
    pub new_issues_delta: i64,
    pub regressed_issues_delta: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseNewIssueItem {
    pub id: i64,
    pub title: String,
    pub exception_type: String,
    pub level: String,
    pub status: String,
    pub is_regressed: bool,
    pub event_count: i64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionComparisonRow {
    pub transaction_name: String,
    pub op: String,
    pub status: String,
    pub base: Option<ReleasePerfMetrics>,
    pub target: Option<ReleasePerfMetrics>,
    pub p50_delta_ms: Option<f64>,
    pub p50_delta_pct: Option<f64>,
    pub p95_delta_ms: Option<f64>,
    pub p95_delta_pct: Option<f64>,
    pub p99_delta_ms: Option<f64>,
    pub p99_delta_pct: Option<f64>,
    pub error_rate_delta: Option<f64>,
    pub throughput_rpm_delta: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseComparisonView {
    pub project_id: i64,
    pub project_slug: String,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub environment: Option<String>,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub verdict: String,
    pub base: ReleaseComparisonSide,
    pub target: ReleaseComparisonSide,
    pub delta: ReleaseComparisonDelta,
    pub new_issues_in_target: Vec<ReleaseNewIssueItem>,
    pub transactions: Vec<TransactionComparisonRow>,
    pub total_transactions_compared: usize,
}
