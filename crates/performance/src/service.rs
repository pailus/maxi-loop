use chrono::{DateTime, Duration, Utc};
use maxi_loop_platform::access::push_access_sql;
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub_notify::PiiScrubRuleEntry;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder, Transaction};

use crate::compare::compare_releases as compare_releases_impl;
use crate::histogram::{estimate_percentile, round_two_decimals};
use crate::model::{
    CompareReleasesFilter, PaginatedSlowOperationsResponse, PaginatedTransactionsResponse,
    PerformanceScope, ReleaseComparisonView, SlowOperationListFilter, SlowOperationSummaryView,
    TraceListFilter, TraceSummaryView, TraceWaterfallView, TransactionDetailView,
    TransactionListFilter, TransactionSummaryView, TransactionTimeBucket,
};
use crate::waterfall::{StoredTraceSegment, assemble_trace_waterfall};

const DEFAULT_WINDOW_HOURS: i64 = 24;
const MAX_WINDOW_HOURS: i64 = 24 * 90;
const MAX_AGGREGATED_GROUPS: i64 = 2000;

/// Parameters for fetching a single transaction's detail series and sampled traces.
#[derive(Clone, Debug)]
pub struct TransactionDetailQueryParams<'a> {
    pub project_slug: &'a str,
    pub transaction_name: &'a str,
    pub op: Option<&'a str>,
    pub environment: Option<&'a str>,
    pub release: Option<&'a str>,
    pub hours: Option<u32>,
}

/// Thin service facade wrapping a `PgPool` so downstream crates like `crates/mcp` can
/// invoke RLS-enforced performance queries without importing `sqlx` (`D006`).
#[derive(Clone)]
pub struct PerformanceService {
    pool: PgPool,
}

impl PerformanceService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list_transactions(
        &self,
        tenant_ctx: TenantContext,
        scope: PerformanceScope<'_>,
        filter: TransactionListFilter,
    ) -> Result<PaginatedTransactionsResponse, AppError> {
        list_transactions(&self.pool, tenant_ctx, scope, filter).await
    }

    pub async fn get_transaction_detail(
        &self,
        tenant_ctx: TenantContext,
        scope: PerformanceScope<'_>,
        params: TransactionDetailQueryParams<'_>,
    ) -> Result<TransactionDetailView, AppError> {
        get_transaction_detail(&self.pool, tenant_ctx, scope, params).await
    }

    pub async fn list_traces(
        &self,
        tenant_ctx: TenantContext,
        scope: PerformanceScope<'_>,
        filter: TraceListFilter,
    ) -> Result<Vec<TraceSummaryView>, AppError> {
        list_traces(&self.pool, tenant_ctx, scope, filter).await
    }

    pub async fn get_trace(
        &self,
        tenant_ctx: TenantContext,
        scope: PerformanceScope<'_>,
        trace_id: &str,
        pii_rules: &[PiiScrubRuleEntry],
    ) -> Result<TraceWaterfallView, AppError> {
        get_trace(&self.pool, tenant_ctx, scope, trace_id, pii_rules).await
    }

    pub async fn list_slow_operations(
        &self,
        tenant_ctx: TenantContext,
        scope: PerformanceScope<'_>,
        filter: SlowOperationListFilter,
    ) -> Result<PaginatedSlowOperationsResponse, AppError> {
        list_slow_operations(&self.pool, tenant_ctx, scope, filter).await
    }

    pub async fn compare_releases(
        &self,
        tenant_ctx: TenantContext,
        scope: PerformanceScope<'_>,
        filter: CompareReleasesFilter,
    ) -> Result<ReleaseComparisonView, AppError> {
        compare_releases_impl(&self.pool, tenant_ctx, scope, filter).await
    }
}

#[derive(FromRow)]
struct AggregatedTxRow {
    project_id: i64,
    project_slug: String,
    workspace_id: i64,
    workspace_slug: String,
    transaction_name: String,
    op: String,
    total_count: i64,
    error_count: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
    last_seen: DateTime<Utc>,
}

#[derive(FromRow)]
struct SeriesBucketRow {
    bucket: DateTime<Utc>,
    total_count: i64,
    error_count: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
}

#[derive(FromRow)]
struct TraceSummaryRow {
    trace_id: String,
    event_id: String,
    project_id: i64,
    project_slug: String,
    workspace_slug: String,
    transaction_name: String,
    op: String,
    status: String,
    is_error: bool,
    sample_reason: String,
    environment: String,
    release: Option<String>,
    platform: String,
    duration_ms: f64,
    span_count: i32,
    start_timestamp: DateTime<Utc>,
    received_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct TraceSegmentRow {
    event_id: String,
    project_id: i64,
    project_slug: String,
    workspace_slug: String,
    trace_id: String,
    root_span_id: String,
    parent_span_id: Option<String>,
    transaction_name: String,
    op: String,
    status: String,
    is_error: bool,
    sample_reason: String,
    environment: String,
    release: Option<String>,
    platform: String,
    start_timestamp: DateTime<Utc>,
    end_timestamp: DateTime<Utc>,
    duration_ms: f64,
    tags: serde_json::Value,
    spans_zstd: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
enum TransactionSort {
    P95,
    P99,
    P50,
    Throughput,
    ErrorRate,
    Avg,
    Name,
}

impl TransactionSort {
    fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("p95") => Ok(Self::P95),
            Some("p99") => Ok(Self::P99),
            Some("p50") => Ok(Self::P50),
            Some("throughput") | Some("count") => Ok(Self::Throughput),
            Some("error_rate") | Some("errors") => Ok(Self::ErrorRate),
            Some("avg") => Ok(Self::Avg),
            Some("name") => Ok(Self::Name),
            Some(_) => Err(AppError::Validation(
                "sort must be one of: p95, p99, p50, throughput, error_rate, avg, name".to_owned(),
            )),
        }
    }
}

/// List aggregated transactions with p50/p95/p99, throughput (rpm), and error rate (`F-PRF-3`, `F-MCP-12`).
pub async fn list_transactions(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: PerformanceScope<'_>,
    filter: TransactionListFilter,
) -> Result<PaginatedTransactionsResponse, AppError> {
    let sort = TransactionSort::parse(filter.sort.as_deref())?;
    let page = filter.page.unwrap_or(1);
    let per_page = filter.per_page.unwrap_or(25);
    if page == 0 {
        return Err(AppError::Validation("page must be >= 1".to_owned()));
    }
    if !(1..=100).contains(&per_page) {
        return Err(AppError::Validation(
            "per_page must be between 1 and 100".to_owned(),
        ));
    }

    let (since, until) = resolve_time_window(filter.since, filter.until, filter.hours)?;
    let window_minutes = ((until - since).num_seconds().max(60)) / 60;
    let window_minutes_f = ((until - since).num_seconds().max(60) as f64) / 60.0;

    let env_filter = normalize_optional_str(filter.environment.as_deref(), 64, "environment")?;
    let rel_filter = normalize_optional_str(filter.release.as_deref(), 128, "release")?;
    let op_filter = normalize_optional_str(filter.op.as_deref(), 128, "op")?;
    let search_pattern = normalize_optional_str(filter.query.as_deref(), 128, "query")?
        .map(|q| format!("%{}%", escape_ilike(&q)));

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    let effective_ws_slug = scope.workspace_slug.or(filter.workspace_slug.as_deref());
    let effective_proj_slug = scope.project_slug.or(filter.project_slug.as_deref());

    if let Some(ws) = effective_ws_slug
        && !is_valid_slug(ws.trim())
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }
    if let Some(ps) = effective_proj_slug
        && !is_valid_slug(ps.trim())
    {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT m.project_id,
                p.slug AS project_slug,
                p.workspace_id,
                w.slug AS workspace_slug,
                m.transaction_name,
                m.op,
                SUM(m.total_count)::BIGINT AS total_count,
                SUM(m.error_count)::BIGINT AS error_count,
                SUM(m.duration_sum_ms)::DOUBLE PRECISION AS duration_sum_ms,
                MIN(m.duration_min_ms)::DOUBLE PRECISION AS duration_min_ms,
                MAX(m.duration_max_ms)::DOUBLE PRECISION AS duration_max_ms,
                sum_histogram_buckets(m.histogram_buckets) AS histogram_buckets,
                MAX(m.minute_bucket) AS last_seen
         FROM transaction_minute_metrics m
         JOIN projects p ON p.tenant_id = m.tenant_id AND p.id = m.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE m.tenant_id = ",
    );
    qb.push_bind(tenant_ctx.tenant_id());
    push_access_sql(scope.access.as_ref(), &mut qb, "p.workspace_id", "p.id");
    qb.push(" AND m.minute_bucket >= ").push_bind(since);
    qb.push(" AND m.minute_bucket <= ").push_bind(until);

    if let Some(ws) = effective_ws_slug {
        qb.push(" AND w.slug = ").push_bind(ws.trim().to_owned());
    }
    if let Some(pid) = filter.project_id {
        if pid <= 0 {
            return Err(AppError::Validation("project_id must be > 0".to_owned()));
        }
        qb.push(" AND m.project_id = ").push_bind(pid);
    }
    if let Some(ps) = effective_proj_slug {
        qb.push(" AND p.slug = ").push_bind(ps.trim().to_owned());
    }
    if let Some(env) = &env_filter {
        qb.push(" AND m.environment = ").push_bind(env.clone());
    }
    if let Some(rel) = &rel_filter {
        qb.push(" AND m.release = ").push_bind(rel.clone());
    }
    if let Some(op) = &op_filter {
        qb.push(" AND m.op = ").push_bind(op.clone());
    }
    if let Some(pat) = &search_pattern {
        qb.push(" AND m.transaction_name ILIKE ")
            .push_bind(pat.clone())
            .push(" ESCAPE '\\'");
    }

    qb.push(
        " GROUP BY m.project_id, p.slug, p.workspace_id, w.slug, m.transaction_name, m.op
          ORDER BY SUM(m.total_count) DESC
          LIMIT ",
    );
    qb.push_bind(MAX_AGGREGATED_GROUPS);

    let rows: Vec<AggregatedTxRow> = qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list transactions: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list transactions: {err}")))?;

    let mut summaries: Vec<TransactionSummaryView> = rows
        .into_iter()
        .map(|row| build_tx_summary(row, window_minutes_f))
        .collect();

    summaries.sort_by(|a, b| match sort {
        TransactionSort::P95 => b
            .p95_ms
            .total_cmp(&a.p95_ms)
            .then_with(|| b.total_count.cmp(&a.total_count)),
        TransactionSort::P99 => b
            .p99_ms
            .total_cmp(&a.p99_ms)
            .then_with(|| b.total_count.cmp(&a.total_count)),
        TransactionSort::P50 => b
            .p50_ms
            .total_cmp(&a.p50_ms)
            .then_with(|| b.total_count.cmp(&a.total_count)),
        TransactionSort::Throughput => b
            .total_count
            .cmp(&a.total_count)
            .then_with(|| b.p95_ms.total_cmp(&a.p95_ms)),
        TransactionSort::ErrorRate => b
            .error_rate
            .total_cmp(&a.error_rate)
            .then_with(|| b.error_count.cmp(&a.error_count)),
        TransactionSort::Avg => b
            .avg_ms
            .total_cmp(&a.avg_ms)
            .then_with(|| b.total_count.cmp(&a.total_count)),
        TransactionSort::Name => a
            .transaction_name
            .cmp(&b.transaction_name)
            .then_with(|| a.project_slug.cmp(&b.project_slug)),
    });

    let total = i64::try_from(summaries.len()).unwrap_or(0);
    let total_pages = if total == 0 {
        1
    } else {
        u32::try_from((total + i64::from(per_page) - 1) / i64::from(per_page)).unwrap_or(1)
    };
    let offset = usize::try_from((page - 1).saturating_mul(per_page)).unwrap_or(0);
    let items: Vec<TransactionSummaryView> = summaries
        .into_iter()
        .skip(offset)
        .take(per_page as usize)
        .collect();

    Ok(PaginatedTransactionsResponse {
        items,
        total,
        page,
        per_page,
        total_pages,
        window_minutes,
        since,
        until,
    })
}

/// Fetch detailed latency time series and recent sampled traces for a single transaction.
pub async fn get_transaction_detail(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: PerformanceScope<'_>,
    params: TransactionDetailQueryParams<'_>,
) -> Result<TransactionDetailView, AppError> {
    let proj_slug = params.project_slug.trim();
    if !is_valid_slug(proj_slug) {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }
    let tx_name = params.transaction_name.trim();
    if tx_name.is_empty() || tx_name.len() > 512 {
        return Err(AppError::Validation(
            "transaction_name must be 1..=512 characters".to_owned(),
        ));
    }
    if let Some(scoped_proj) = scope.project_slug
        && scoped_proj.trim() != proj_slug
    {
        return Err(AppError::Forbidden(
            "token is scoped to a different project",
        ));
    }

    let (since, until) = resolve_time_window(None, None, params.hours)?;
    let window_secs = (until - since).num_seconds().max(60);
    let window_minutes_f = (window_secs as f64) / 60.0;
    let use_hourly = window_secs > 6 * 3600;
    let interval = if use_hourly { "1h" } else { "1m" };

    let env_filter = normalize_optional_str(params.environment, 64, "environment")?;
    let rel_filter = normalize_optional_str(params.release, 128, "release")?;
    let op_filter = normalize_optional_str(params.op, 128, "op")?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    // 1. Summary row
    let mut sum_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT m.project_id,
                p.slug AS project_slug,
                p.workspace_id,
                w.slug AS workspace_slug,
                m.transaction_name,
                m.op,
                SUM(m.total_count)::BIGINT AS total_count,
                SUM(m.error_count)::BIGINT AS error_count,
                SUM(m.duration_sum_ms)::DOUBLE PRECISION AS duration_sum_ms,
                MIN(m.duration_min_ms)::DOUBLE PRECISION AS duration_min_ms,
                MAX(m.duration_max_ms)::DOUBLE PRECISION AS duration_max_ms,
                sum_histogram_buckets(m.histogram_buckets) AS histogram_buckets,
                MAX(m.minute_bucket) AS last_seen
         FROM transaction_minute_metrics m
         JOIN projects p ON p.tenant_id = m.tenant_id AND p.id = m.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE m.tenant_id = ",
    );
    sum_qb.push_bind(tenant_ctx.tenant_id());
    push_access_sql(scope.access.as_ref(), &mut sum_qb, "p.workspace_id", "p.id");
    sum_qb
        .push(" AND p.slug = ")
        .push_bind(proj_slug.to_owned());
    sum_qb
        .push(" AND m.transaction_name = ")
        .push_bind(tx_name.to_owned());
    sum_qb.push(" AND m.minute_bucket >= ").push_bind(since);
    sum_qb.push(" AND m.minute_bucket <= ").push_bind(until);
    if let Some(ws) = scope.workspace_slug {
        sum_qb
            .push(" AND w.slug = ")
            .push_bind(ws.trim().to_owned());
    }
    if let Some(op_val) = &op_filter {
        sum_qb.push(" AND m.op = ").push_bind(op_val.clone());
    }
    if let Some(env_val) = &env_filter {
        sum_qb
            .push(" AND m.environment = ")
            .push_bind(env_val.clone());
    }
    if let Some(rel_val) = &rel_filter {
        sum_qb.push(" AND m.release = ").push_bind(rel_val.clone());
    }
    sum_qb.push(
        " GROUP BY m.project_id, p.slug, p.workspace_id, w.slug, m.transaction_name, m.op
          ORDER BY SUM(m.total_count) DESC
          LIMIT 1",
    );

    let summary_row: AggregatedTxRow = sum_qb
        .build_query_as()
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("get transaction summary: {err}")))?
        .ok_or(AppError::NotFound)?;

    let matched_op = summary_row.op.clone();
    let matched_project_id = summary_row.project_id;
    let summary = build_tx_summary(summary_row, window_minutes_f);

    // 2. Time-series buckets (`1m` or `1h`)
    let bucket_expr = if use_hourly {
        "date_trunc('hour', m.minute_bucket)"
    } else {
        "m.minute_bucket"
    };
    let mut series_qb: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "SELECT {bucket_expr} AS bucket,
                SUM(m.total_count)::BIGINT AS total_count,
                SUM(m.error_count)::BIGINT AS error_count,
                SUM(m.duration_sum_ms)::DOUBLE PRECISION AS duration_sum_ms,
                MIN(m.duration_min_ms)::DOUBLE PRECISION AS duration_min_ms,
                MAX(m.duration_max_ms)::DOUBLE PRECISION AS duration_max_ms,
                sum_histogram_buckets(m.histogram_buckets) AS histogram_buckets
         FROM transaction_minute_metrics m
         WHERE m.tenant_id = "
    ));
    series_qb.push_bind(tenant_ctx.tenant_id());
    series_qb
        .push(" AND m.project_id = ")
        .push_bind(matched_project_id);
    series_qb
        .push(" AND m.transaction_name = ")
        .push_bind(tx_name.to_owned());
    series_qb.push(" AND m.op = ").push_bind(matched_op);
    series_qb.push(" AND m.minute_bucket >= ").push_bind(since);
    series_qb.push(" AND m.minute_bucket <= ").push_bind(until);
    if let Some(env_val) = &env_filter {
        series_qb
            .push(" AND m.environment = ")
            .push_bind(env_val.clone());
    }
    if let Some(rel_val) = &rel_filter {
        series_qb
            .push(" AND m.release = ")
            .push_bind(rel_val.clone());
    }
    series_qb.push(" GROUP BY bucket ORDER BY bucket ASC LIMIT 500");

    let series_rows: Vec<SeriesBucketRow> = series_qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("get transaction series: {err}")))?;

    let series: Vec<TransactionTimeBucket> = series_rows
        .into_iter()
        .map(|row| {
            let avg_ms = if row.total_count > 0 {
                round_two_decimals(row.duration_sum_ms / row.total_count as f64)
            } else {
                0.0
            };
            TransactionTimeBucket {
                bucket: row.bucket,
                total_count: row.total_count,
                error_count: row.error_count,
                p50_ms: estimate_percentile(
                    &row.histogram_buckets,
                    row.duration_min_ms,
                    row.duration_max_ms,
                    0.50,
                ),
                p95_ms: estimate_percentile(
                    &row.histogram_buckets,
                    row.duration_min_ms,
                    row.duration_max_ms,
                    0.95,
                ),
                p99_ms: estimate_percentile(
                    &row.histogram_buckets,
                    row.duration_min_ms,
                    row.duration_max_ms,
                    0.99,
                ),
                avg_ms,
            }
        })
        .collect();

    // 3. Recent sampled traces for this transaction
    let mut tr_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT t.trace_id, t.event_id, t.project_id, p.slug AS project_slug,
                w.slug AS workspace_slug, t.transaction_name, t.op, t.status, t.is_error,
                t.sample_reason, t.environment, t.release, t.platform,
                t.duration_ms, t.span_count, t.start_timestamp, t.received_at
         FROM traces t
         JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE t.tenant_id = ",
    );
    tr_qb.push_bind(tenant_ctx.tenant_id());
    tr_qb
        .push(" AND t.project_id = ")
        .push_bind(matched_project_id);
    tr_qb
        .push(" AND t.transaction_name = ")
        .push_bind(tx_name.to_owned());
    tr_qb.push(" AND t.received_at >= ").push_bind(since);
    tr_qb.push(" AND t.received_at <= ").push_bind(until);
    if let Some(env_val) = &env_filter {
        tr_qb
            .push(" AND t.environment = ")
            .push_bind(env_val.clone());
    }
    if let Some(rel_val) = &rel_filter {
        tr_qb.push(" AND t.release = ").push_bind(rel_val.clone());
    }
    tr_qb.push(" ORDER BY t.received_at DESC LIMIT 25");

    let trace_rows: Vec<TraceSummaryRow> = tr_qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list transaction traces: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit transaction detail: {err}")))?;

    let recent_traces = trace_rows.into_iter().map(Into::into).collect();

    Ok(TransactionDetailView {
        summary,
        interval: interval.to_owned(),
        series,
        recent_traces,
    })
}

/// List recent sampled traces within the caller's tenant/workspace/project scope.
pub async fn list_traces(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: PerformanceScope<'_>,
    filter: TraceListFilter,
) -> Result<Vec<TraceSummaryView>, AppError> {
    let limit = i64::from(filter.limit.unwrap_or(50).clamp(1, 200));
    let (since, until) = resolve_time_window(filter.since, filter.until, filter.hours)?;

    let env_filter = normalize_optional_str(filter.environment.as_deref(), 64, "environment")?;
    let rel_filter = normalize_optional_str(filter.release.as_deref(), 128, "release")?;
    let tx_filter =
        normalize_optional_str(filter.transaction_name.as_deref(), 512, "transaction_name")?;
    let reason_filter = match filter
        .sample_reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some("slow" | "error" | "random" | "issue") => {
            filter.sample_reason.as_deref().map(|s| s.trim().to_owned())
        }
        Some(_) => {
            return Err(AppError::Validation(
                "sample_reason must be one of: slow, error, random, issue".to_owned(),
            ));
        }
    };

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    let effective_ws_slug = scope.workspace_slug.or(filter.workspace_slug.as_deref());
    let effective_proj_slug = scope.project_slug.or(filter.project_slug.as_deref());

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT t.trace_id, t.event_id, t.project_id, p.slug AS project_slug,
                w.slug AS workspace_slug, t.transaction_name, t.op, t.status, t.is_error,
                t.sample_reason, t.environment, t.release, t.platform,
                t.duration_ms, t.span_count, t.start_timestamp, t.received_at
         FROM traces t
         JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE t.tenant_id = ",
    );
    qb.push_bind(tenant_ctx.tenant_id());
    push_access_sql(scope.access.as_ref(), &mut qb, "p.workspace_id", "p.id");
    qb.push(" AND t.received_at >= ").push_bind(since);
    qb.push(" AND t.received_at <= ").push_bind(until);

    if let Some(ws) = effective_ws_slug {
        if !is_valid_slug(ws.trim()) {
            return Err(AppError::Validation("invalid workspace_slug".to_owned()));
        }
        qb.push(" AND w.slug = ").push_bind(ws.trim().to_owned());
    }
    if let Some(pid) = filter.project_id {
        if pid <= 0 {
            return Err(AppError::Validation("project_id must be > 0".to_owned()));
        }
        qb.push(" AND t.project_id = ").push_bind(pid);
    }
    if let Some(ps) = effective_proj_slug {
        if !is_valid_slug(ps.trim()) {
            return Err(AppError::Validation("invalid project_slug".to_owned()));
        }
        qb.push(" AND p.slug = ").push_bind(ps.trim().to_owned());
    }
    if let Some(tn) = &tx_filter {
        qb.push(" AND t.transaction_name = ").push_bind(tn.clone());
    }
    if let Some(env) = &env_filter {
        qb.push(" AND t.environment = ").push_bind(env.clone());
    }
    if let Some(rel) = &rel_filter {
        qb.push(" AND t.release = ").push_bind(rel.clone());
    }
    if let Some(reason) = &reason_filter {
        qb.push(" AND t.sample_reason = ").push_bind(reason.clone());
    }
    if let Some(is_err) = filter.is_error {
        qb.push(" AND t.is_error = ").push_bind(is_err);
    }
    if let Some(min_ms) = filter.min_duration_ms {
        if !min_ms.is_finite() || min_ms < 0.0 {
            return Err(AppError::Validation(
                "min_duration_ms must be >= 0".to_owned(),
            ));
        }
        qb.push(" AND t.duration_ms >= ").push_bind(min_ms);
    }

    qb.push(" ORDER BY t.received_at DESC LIMIT ")
        .push_bind(limit);

    let rows: Vec<TraceSummaryRow> = qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list traces: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list traces: {err}")))?;

    Ok(rows.into_iter().map(Into::into).collect())
}

/// Fetch and assemble a distributed trace waterfall by `trace_id` (`F-PRF-4`, `F-PRF-5`, `F-MCP-13`).
pub async fn get_trace(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: PerformanceScope<'_>,
    trace_id: &str,
    pii_rules: &[PiiScrubRuleEntry],
) -> Result<TraceWaterfallView, AppError> {
    let normalized_trace_id = trace_id.trim().replace('-', "").to_ascii_lowercase();
    if normalized_trace_id.is_empty()
        || normalized_trace_id.len() > 64
        || !normalized_trace_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppError::Validation(
            "trace_id must be a non-empty hex string (max 64 chars)".to_owned(),
        ));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT t.event_id, t.project_id, p.slug AS project_slug, w.slug AS workspace_slug,
                t.trace_id, t.root_span_id, t.parent_span_id, t.transaction_name, t.op,
                t.status, t.is_error, t.sample_reason, t.environment, t.release, t.platform,
                t.start_timestamp, t.end_timestamp, t.duration_ms, t.tags, t.spans_zstd
         FROM traces t
         JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE t.tenant_id = ",
    );
    qb.push_bind(tenant_ctx.tenant_id());
    push_access_sql(scope.access.as_ref(), &mut qb, "p.workspace_id", "p.id");
    qb.push(" AND t.trace_id = ")
        .push_bind(normalized_trace_id.clone());

    if let Some(ws) = scope.workspace_slug {
        if !is_valid_slug(ws.trim()) {
            return Err(AppError::Validation("invalid workspace_slug".to_owned()));
        }
        qb.push(" AND w.slug = ").push_bind(ws.trim().to_owned());
    }
    if let Some(ps) = scope.project_slug {
        if !is_valid_slug(ps.trim()) {
            return Err(AppError::Validation("invalid project_slug".to_owned()));
        }
        qb.push(" AND p.slug = ").push_bind(ps.trim().to_owned());
    }
    qb.push(" ORDER BY t.start_timestamp ASC LIMIT 100");

    let rows: Vec<TraceSegmentRow> = qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("get trace segments: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit get trace: {err}")))?;

    if rows.is_empty() {
        return Err(AppError::NotFound);
    }

    let segments: Vec<StoredTraceSegment> = rows
        .into_iter()
        .map(|r| StoredTraceSegment {
            event_id: r.event_id,
            project_id: r.project_id,
            project_slug: r.project_slug,
            workspace_slug: r.workspace_slug,
            trace_id: r.trace_id,
            root_span_id: r.root_span_id,
            parent_span_id: r.parent_span_id,
            transaction_name: r.transaction_name,
            op: r.op,
            status: r.status,
            is_error: r.is_error,
            sample_reason: r.sample_reason,
            environment: r.environment,
            release: r.release,
            platform: r.platform,
            start_timestamp: r.start_timestamp,
            end_timestamp: r.end_timestamp,
            duration_ms: r.duration_ms,
            tags: r.tags,
            spans_zstd: r.spans_zstd,
        })
        .collect();

    assemble_trace_waterfall(normalized_trace_id, segments, pii_rules)
}

#[derive(FromRow)]
struct AggregatedSpanOpRow {
    project_id: i64,
    project_slug: String,
    workspace_id: i64,
    workspace_slug: String,
    op: String,
    description_hash: String,
    normalized_description: String,
    total_count: i64,
    error_count: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
    sample_trace_id: Option<String>,
    last_seen: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
enum SlowOpSort {
    P95,
    P99,
    P50,
    TotalMs,
    Avg,
    Count,
    ErrorRate,
}

impl SlowOpSort {
    fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("p95") => Ok(Self::P95),
            Some("p99") => Ok(Self::P99),
            Some("p50") => Ok(Self::P50),
            Some("total_ms") | Some("total") => Ok(Self::TotalMs),
            Some("avg") => Ok(Self::Avg),
            Some("count") => Ok(Self::Count),
            Some("error_rate") | Some("errors") => Ok(Self::ErrorRate),
            Some(_) => Err(AppError::Validation(
                "sort must be one of: p95, p99, p50, total_ms, avg, count, error_rate".to_owned(),
            )),
        }
    }
}

/// List aggregated slow DB queries and operations per normalized `(op, description)` (`F-PRF-7`, `F-MCP-14`).
pub async fn list_slow_operations(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: PerformanceScope<'_>,
    filter: SlowOperationListFilter,
) -> Result<PaginatedSlowOperationsResponse, AppError> {
    let sort = SlowOpSort::parse(filter.sort.as_deref())?;
    let page = filter.page.unwrap_or(1);
    let per_page = filter.per_page.unwrap_or(25);
    if page == 0 {
        return Err(AppError::Validation("page must be >= 1".to_owned()));
    }
    if !(1..=100).contains(&per_page) {
        return Err(AppError::Validation(
            "per_page must be between 1 and 100".to_owned(),
        ));
    }

    let (since, until) = resolve_time_window(filter.since, filter.until, filter.hours)?;
    let window_minutes = ((until - since).num_seconds().max(60)) / 60;

    let env_filter = normalize_optional_str(filter.environment.as_deref(), 64, "environment")?;
    let rel_filter = normalize_optional_str(filter.release.as_deref(), 128, "release")?;
    let op_filter = normalize_optional_str(filter.op.as_deref(), 128, "op")?;
    let search_pattern = normalize_optional_str(filter.query.as_deref(), 128, "query")?
        .map(|q| format!("%{}%", escape_ilike(&q)));
    let db_only = filter.db_only.unwrap_or(false);

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    let effective_ws_slug = scope.workspace_slug.or(filter.workspace_slug.as_deref());
    let effective_proj_slug = scope.project_slug.or(filter.project_slug.as_deref());

    if let Some(ws) = effective_ws_slug
        && !is_valid_slug(ws.trim())
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }
    if let Some(ps) = effective_proj_slug
        && !is_valid_slug(ps.trim())
    {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT m.project_id,
                p.slug AS project_slug,
                p.workspace_id,
                w.slug AS workspace_slug,
                m.op,
                m.description_hash,
                MAX(m.normalized_description) AS normalized_description,
                SUM(m.total_count)::BIGINT AS total_count,
                SUM(m.error_count)::BIGINT AS error_count,
                SUM(m.duration_sum_ms)::DOUBLE PRECISION AS duration_sum_ms,
                MIN(m.duration_min_ms)::DOUBLE PRECISION AS duration_min_ms,
                MAX(m.duration_max_ms)::DOUBLE PRECISION AS duration_max_ms,
                sum_histogram_buckets(m.histogram_buckets) AS histogram_buckets,
                MAX(m.sample_trace_id) AS sample_trace_id,
                MAX(m.minute_bucket) AS last_seen
         FROM span_minute_metrics m
         JOIN projects p ON p.tenant_id = m.tenant_id AND p.id = m.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE m.tenant_id = ",
    );
    qb.push_bind(tenant_ctx.tenant_id());
    push_access_sql(scope.access.as_ref(), &mut qb, "p.workspace_id", "p.id");
    qb.push(" AND m.minute_bucket >= ").push_bind(since);
    qb.push(" AND m.minute_bucket <= ").push_bind(until);

    if let Some(ws) = effective_ws_slug {
        qb.push(" AND w.slug = ").push_bind(ws.trim().to_owned());
    }
    if let Some(pid) = filter.project_id {
        if pid <= 0 {
            return Err(AppError::Validation("project_id must be > 0".to_owned()));
        }
        qb.push(" AND m.project_id = ").push_bind(pid);
    }
    if let Some(ps) = effective_proj_slug {
        qb.push(" AND p.slug = ").push_bind(ps.trim().to_owned());
    }
    if let Some(env) = &env_filter {
        qb.push(" AND m.environment = ").push_bind(env.clone());
    }
    if let Some(rel) = &rel_filter {
        qb.push(" AND m.release = ").push_bind(rel.clone());
    }
    if let Some(op) = &op_filter {
        let trimmed = op.trim();
        if let Some(prefix) = trimmed.strip_suffix('*') {
            let pat = format!("{}%", escape_ilike(prefix));
            qb.push(" AND m.op ILIKE ")
                .push_bind(pat)
                .push(" ESCAPE '\\'");
        } else if trimmed.ends_with('.') {
            let pat = format!("{}%", escape_ilike(trimmed));
            qb.push(" AND m.op ILIKE ")
                .push_bind(pat)
                .push(" ESCAPE '\\'");
        } else if trimmed.eq_ignore_ascii_case("db") {
            qb.push(" AND (LOWER(m.op) = 'db' OR LOWER(m.op) LIKE 'db.%')");
        } else {
            qb.push(" AND m.op = ").push_bind(trimmed.to_owned());
        }
    } else if db_only {
        qb.push(" AND (LOWER(m.op) = 'db' OR LOWER(m.op) LIKE 'db.%')");
    }
    if let Some(pat) = &search_pattern {
        qb.push(" AND m.normalized_description ILIKE ")
            .push_bind(pat.clone())
            .push(" ESCAPE '\\'");
    }

    qb.push(
        " GROUP BY m.project_id, p.slug, p.workspace_id, w.slug, m.op, m.description_hash
          ORDER BY SUM(m.duration_sum_ms) DESC
          LIMIT ",
    );
    qb.push_bind(MAX_AGGREGATED_GROUPS);

    let rows: Vec<AggregatedSpanOpRow> = qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list slow operations: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list slow operations: {err}")))?;

    let mut summaries: Vec<SlowOperationSummaryView> = rows
        .into_iter()
        .map(|row| {
            let total = row.total_count.max(0);
            let errors = row.error_count.max(0);
            let error_rate = if total > 0 {
                round_two_decimals((errors as f64) / (total as f64) * 10000.0) / 10000.0
            } else {
                0.0
            };
            let avg_ms = if total > 0 {
                round_two_decimals(row.duration_sum_ms / (total as f64))
            } else {
                0.0
            };
            SlowOperationSummaryView {
                project_id: row.project_id,
                project_slug: row.project_slug,
                workspace_id: row.workspace_id,
                workspace_slug: row.workspace_slug,
                op: row.op,
                description_hash: row.description_hash,
                normalized_description: row.normalized_description,
                total_count: total,
                error_count: errors,
                error_rate,
                total_ms: round_two_decimals(row.duration_sum_ms),
                avg_ms,
                p50_ms: estimate_percentile(
                    &row.histogram_buckets,
                    row.duration_min_ms,
                    row.duration_max_ms,
                    0.50,
                ),
                p95_ms: estimate_percentile(
                    &row.histogram_buckets,
                    row.duration_min_ms,
                    row.duration_max_ms,
                    0.95,
                ),
                p99_ms: estimate_percentile(
                    &row.histogram_buckets,
                    row.duration_min_ms,
                    row.duration_max_ms,
                    0.99,
                ),
                min_ms: round_two_decimals(row.duration_min_ms),
                max_ms: round_two_decimals(row.duration_max_ms),
                sample_trace_id: row.sample_trace_id,
                last_seen: row.last_seen,
            }
        })
        .collect();

    summaries.sort_by(|a, b| match sort {
        SlowOpSort::P95 => b
            .p95_ms
            .total_cmp(&a.p95_ms)
            .then_with(|| b.total_ms.total_cmp(&a.total_ms)),
        SlowOpSort::P99 => b
            .p99_ms
            .total_cmp(&a.p99_ms)
            .then_with(|| b.total_ms.total_cmp(&a.total_ms)),
        SlowOpSort::P50 => b
            .p50_ms
            .total_cmp(&a.p50_ms)
            .then_with(|| b.total_ms.total_cmp(&a.total_ms)),
        SlowOpSort::TotalMs => b
            .total_ms
            .total_cmp(&a.total_ms)
            .then_with(|| b.p95_ms.total_cmp(&a.p95_ms)),
        SlowOpSort::Avg => b
            .avg_ms
            .total_cmp(&a.avg_ms)
            .then_with(|| b.total_count.cmp(&a.total_count)),
        SlowOpSort::Count => b
            .total_count
            .cmp(&a.total_count)
            .then_with(|| b.p95_ms.total_cmp(&a.p95_ms)),
        SlowOpSort::ErrorRate => b
            .error_rate
            .total_cmp(&a.error_rate)
            .then_with(|| b.error_count.cmp(&a.error_count)),
    });

    let total = i64::try_from(summaries.len()).unwrap_or(0);
    let total_pages = if total == 0 {
        1
    } else {
        u32::try_from((total + i64::from(per_page) - 1) / i64::from(per_page)).unwrap_or(1)
    };
    let offset = usize::try_from((page - 1).saturating_mul(per_page)).unwrap_or(0);
    let items = summaries
        .into_iter()
        .skip(offset)
        .take(per_page as usize)
        .collect();

    Ok(PaginatedSlowOperationsResponse {
        items,
        total,
        page,
        per_page,
        total_pages,
        window_minutes,
        since,
        until,
    })
}

fn build_tx_summary(row: AggregatedTxRow, window_minutes_f: f64) -> TransactionSummaryView {
    let total = row.total_count.max(0);
    let errors = row.error_count.max(0);
    let error_rate = if total > 0 {
        round_two_decimals((errors as f64) / (total as f64) * 10000.0) / 10000.0
    } else {
        0.0
    };
    let throughput_rpm = round_two_decimals((total as f64) / window_minutes_f.max(1.0));
    let avg_ms = if total > 0 {
        round_two_decimals(row.duration_sum_ms / (total as f64))
    } else {
        0.0
    };

    TransactionSummaryView {
        project_id: row.project_id,
        project_slug: row.project_slug,
        workspace_id: row.workspace_id,
        workspace_slug: row.workspace_slug,
        transaction_name: row.transaction_name,
        op: row.op,
        total_count: total,
        error_count: errors,
        error_rate,
        throughput_rpm,
        p50_ms: estimate_percentile(
            &row.histogram_buckets,
            row.duration_min_ms,
            row.duration_max_ms,
            0.50,
        ),
        p95_ms: estimate_percentile(
            &row.histogram_buckets,
            row.duration_min_ms,
            row.duration_max_ms,
            0.95,
        ),
        p99_ms: estimate_percentile(
            &row.histogram_buckets,
            row.duration_min_ms,
            row.duration_max_ms,
            0.99,
        ),
        avg_ms,
        min_ms: round_two_decimals(row.duration_min_ms),
        max_ms: round_two_decimals(row.duration_max_ms),
        last_seen: row.last_seen,
    }
}

impl From<TraceSummaryRow> for TraceSummaryView {
    fn from(r: TraceSummaryRow) -> Self {
        Self {
            trace_id: r.trace_id,
            event_id: r.event_id,
            project_id: r.project_id,
            project_slug: r.project_slug,
            workspace_slug: r.workspace_slug,
            transaction_name: r.transaction_name,
            op: r.op,
            status: r.status,
            is_error: r.is_error,
            sample_reason: r.sample_reason,
            environment: r.environment,
            release: r.release,
            platform: r.platform,
            duration_ms: round_two_decimals(r.duration_ms),
            span_count: r.span_count,
            start_timestamp: r.start_timestamp,
            received_at: r.received_at,
        }
    }
}

fn resolve_time_window(
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    hours: Option<u32>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), AppError> {
    let now = Utc::now();
    let end = until.unwrap_or(now);
    let start = match since {
        Some(s) => s,
        None => {
            let h = i64::from(hours.unwrap_or(DEFAULT_WINDOW_HOURS as u32));
            if !(1..=MAX_WINDOW_HOURS).contains(&h) {
                return Err(AppError::Validation(format!(
                    "hours must be between 1 and {MAX_WINDOW_HOURS}"
                )));
            }
            end - Duration::hours(h)
        }
    };
    if start >= end {
        return Err(AppError::Validation(
            "since must be before until".to_owned(),
        ));
    }
    if (end - start) > Duration::hours(MAX_WINDOW_HOURS) {
        return Err(AppError::Validation(
            "requested time range exceeds maximum of 90 days".to_owned(),
        ));
    }
    Ok((start, end))
}

async fn verify_tenant_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
) -> Result<(), AppError> {
    let slug = tenant_slug.trim();
    if !is_valid_slug(slug) {
        return Err(AppError::Validation("invalid tenant_slug".to_owned()));
    }
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM tenants WHERE id = $1 AND slug = $2")
            .bind(tenant_ctx.tenant_id())
            .bind(slug)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("verify tenant slug: {err}")))?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(())
}

fn normalize_optional_str(
    raw: Option<&str>,
    max_len: usize,
    field: &'static str,
) -> Result<Option<String>, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) if v.len() <= max_len => Ok(Some(v.to_owned())),
        Some(_) => Err(AppError::Validation(format!(
            "{field} exceeds maximum length of {max_len} characters"
        ))),
    }
}

fn escape_ilike(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            other => out.push(other),
        }
    }
    out
}
