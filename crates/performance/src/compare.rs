use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder, Transaction};

use crate::histogram::{estimate_percentile, round_two_decimals};
use crate::ingest_proc::infer_commit_sha;
use crate::model::{
    CompareReleasesFilter, PerformanceScope, ReleaseComparisonDelta, ReleaseComparisonSide,
    ReleaseComparisonView, ReleaseIssueMetrics, ReleaseNewIssueItem, ReleasePerfMetrics,
    TransactionComparisonRow,
};

const DEFAULT_COMPARE_WINDOW_HOURS: i64 = 24 * 30;
const MAX_WINDOW_HOURS: i64 = 24 * 90;

#[derive(FromRow)]
struct ProjectIdentityRow {
    id: i64,
    slug: String,
    workspace_id: i64,
    workspace_slug: String,
}

#[derive(FromRow)]
struct ReleaseMetaRow {
    version: String,
    commit_sha: Option<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    deployed_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct ReleaseOverallPerfRow {
    release: String,
    total_count: i64,
    error_count: i64,
    active_minutes: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
}

#[derive(FromRow)]
struct ReleaseTxPerfRow {
    release: String,
    transaction_name: String,
    op: String,
    total_count: i64,
    error_count: i64,
    active_minutes: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: Vec<i64>,
}

#[derive(FromRow)]
struct ReleaseIssueCountsRow {
    release_version: String,
    linked_issues_count: i64,
    new_issues_count: i64,
    regressed_issues_count: i64,
    unresolved_issues_count: i64,
    issue_events_count: i64,
}

#[derive(FromRow)]
struct NewIssueRow {
    id: i64,
    title: String,
    exception_type: String,
    level: String,
    status: String,
    is_regressed: bool,
    event_count: i64,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
}

/// Compare two releases (`base_release` vs `target_release`) for a project across performance
/// percentiles, throughput, error rate, and issue counts (`F-PRF-6`, `F-MCP-14`).
pub async fn compare_releases(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: PerformanceScope<'_>,
    filter: CompareReleasesFilter,
) -> Result<ReleaseComparisonView, AppError> {
    let project_slug = filter.project_slug.trim();
    if !is_valid_slug(project_slug) {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }
    if let Some(scoped_proj) = scope.project_slug
        && scoped_proj.trim() != project_slug
    {
        return Err(AppError::Forbidden(
            "token is scoped to a different project",
        ));
    }

    let base_release = validate_release_version(&filter.base_release, "base_release")?;
    let target_release = validate_release_version(&filter.target_release, "target_release")?;
    if base_release == target_release {
        return Err(AppError::Validation(
            "base_release and target_release must be different".to_owned(),
        ));
    }

    let env_filter = match filter
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(v) if v.len() <= 64 => Some(v.to_owned()),
        Some(_) => {
            return Err(AppError::Validation(
                "environment exceeds maximum length of 64 characters".to_owned(),
            ));
        }
    };

    let limit = usize::try_from(filter.limit.unwrap_or(25).clamp(1, 100)).unwrap_or(25);
    let (since, until) = resolve_compare_window(filter.since, filter.until, filter.hours)?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    // 1. Resolve target project inside tenant & optional workspace scope.
    let project = resolve_project(&mut tx, tenant_ctx, scope.workspace_slug, project_slug).await?;

    // 2. Load release metadata for both releases.
    let meta_rows: Vec<ReleaseMetaRow> = sqlx::query_as(
        "SELECT version, commit_sha, first_seen, last_seen, deployed_at
         FROM releases
         WHERE tenant_id = $1 AND project_id = $2 AND version IN ($3, $4)",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(project.id)
    .bind(&base_release)
    .bind(&target_release)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load release metadata: {err}")))?;
    let mut meta_map: BTreeMap<String, ReleaseMetaRow> = meta_rows
        .into_iter()
        .map(|r| (r.version.clone(), r))
        .collect();

    // 3. Aggregate overall project performance for both releases.
    let mut overall_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT m.release,
                SUM(m.total_count)::BIGINT AS total_count,
                SUM(m.error_count)::BIGINT AS error_count,
                COUNT(DISTINCT m.minute_bucket)::BIGINT AS active_minutes,
                SUM(m.duration_sum_ms)::DOUBLE PRECISION AS duration_sum_ms,
                MIN(m.duration_min_ms)::DOUBLE PRECISION AS duration_min_ms,
                MAX(m.duration_max_ms)::DOUBLE PRECISION AS duration_max_ms,
                sum_histogram_buckets(m.histogram_buckets) AS histogram_buckets
         FROM transaction_minute_metrics m
         WHERE m.tenant_id = ",
    );
    overall_qb.push_bind(tenant_ctx.tenant_id());
    overall_qb
        .push(" AND m.project_id = ")
        .push_bind(project.id);
    overall_qb
        .push(" AND m.release IN (")
        .push_bind(base_release.clone())
        .push(", ")
        .push_bind(target_release.clone())
        .push(")");
    overall_qb
        .push(" AND m.minute_bucket >= ")
        .push_bind(since)
        .push(" AND m.minute_bucket <= ")
        .push_bind(until);
    if let Some(env) = &env_filter {
        overall_qb.push(" AND m.environment = ").push_bind(env);
    }
    overall_qb.push(" GROUP BY m.release");

    let overall_rows: Vec<ReleaseOverallPerfRow> = overall_qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("compare release overall perf: {err}")))?;

    let mut overall_perf: BTreeMap<String, ReleasePerfMetrics> = BTreeMap::new();
    for row in overall_rows {
        overall_perf.insert(
            row.release,
            build_perf_metrics(
                row.total_count,
                row.error_count,
                row.active_minutes,
                row.duration_sum_ms,
                row.duration_min_ms,
                row.duration_max_ms,
                &row.histogram_buckets,
            ),
        );
    }

    // 4. Aggregate per-transaction performance for both releases.
    let mut tx_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT m.release,
                m.transaction_name,
                m.op,
                SUM(m.total_count)::BIGINT AS total_count,
                SUM(m.error_count)::BIGINT AS error_count,
                COUNT(DISTINCT m.minute_bucket)::BIGINT AS active_minutes,
                SUM(m.duration_sum_ms)::DOUBLE PRECISION AS duration_sum_ms,
                MIN(m.duration_min_ms)::DOUBLE PRECISION AS duration_min_ms,
                MAX(m.duration_max_ms)::DOUBLE PRECISION AS duration_max_ms,
                sum_histogram_buckets(m.histogram_buckets) AS histogram_buckets
         FROM transaction_minute_metrics m
         WHERE m.tenant_id = ",
    );
    tx_qb.push_bind(tenant_ctx.tenant_id());
    tx_qb.push(" AND m.project_id = ").push_bind(project.id);
    tx_qb
        .push(" AND m.release IN (")
        .push_bind(base_release.clone())
        .push(", ")
        .push_bind(target_release.clone())
        .push(")");
    tx_qb
        .push(" AND m.minute_bucket >= ")
        .push_bind(since)
        .push(" AND m.minute_bucket <= ")
        .push_bind(until);
    if let Some(env) = &env_filter {
        tx_qb.push(" AND m.environment = ").push_bind(env);
    }
    tx_qb.push(
        " GROUP BY m.release, m.transaction_name, m.op ORDER BY SUM(m.total_count) DESC LIMIT 2000",
    );

    let tx_rows: Vec<ReleaseTxPerfRow> = tx_qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("compare release tx perf: {err}")))?;

    // 5. Aggregate issue counts for both releases (`issue_releases` + `issues`).
    let mut iss_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT ir.release_version,
                COUNT(DISTINCT ir.issue_id)::BIGINT AS linked_issues_count,
                COUNT(DISTINCT CASE WHEN i.first_release = ir.release_version THEN ir.issue_id END)::BIGINT AS new_issues_count,
                COUNT(DISTINCT CASE WHEN i.is_regressed THEN ir.issue_id END)::BIGINT AS regressed_issues_count,
                COUNT(DISTINCT CASE WHEN i.status = 'unresolved' THEN ir.issue_id END)::BIGINT AS unresolved_issues_count,
                COALESCE(SUM(ir.event_count), 0)::BIGINT AS issue_events_count
         FROM issue_releases ir
         JOIN issues i ON i.tenant_id = ir.tenant_id AND i.project_id = ir.project_id AND i.id = ir.issue_id
         WHERE ir.tenant_id = ",
    );
    iss_qb.push_bind(tenant_ctx.tenant_id());
    iss_qb.push(" AND ir.project_id = ").push_bind(project.id);
    iss_qb
        .push(" AND ir.release_version IN (")
        .push_bind(base_release.clone())
        .push(", ")
        .push_bind(target_release.clone())
        .push(")");
    if let Some(env) = &env_filter {
        iss_qb.push(" AND i.last_environment = ").push_bind(env);
    }
    iss_qb.push(" GROUP BY ir.release_version");

    let issue_rows: Vec<ReleaseIssueCountsRow> = iss_qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("compare release issues: {err}")))?;

    let mut issue_map: BTreeMap<String, ReleaseIssueMetrics> = issue_rows
        .into_iter()
        .map(|r| {
            (
                r.release_version,
                ReleaseIssueMetrics {
                    linked_issues_count: r.linked_issues_count,
                    new_issues_count: r.new_issues_count,
                    regressed_issues_count: r.regressed_issues_count,
                    unresolved_issues_count: r.unresolved_issues_count,
                    issue_events_count: r.issue_events_count,
                },
            )
        })
        .collect();

    // 6. Top newly introduced issues in `target_release` (not present in `base_release`).
    let mut new_iss_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT i.id, i.title, i.exception_type, i.level, i.status, i.is_regressed,
                ir.event_count, i.first_seen, i.last_seen
         FROM issue_releases ir
         JOIN issues i ON i.tenant_id = ir.tenant_id AND i.project_id = ir.project_id AND i.id = ir.issue_id
         WHERE ir.tenant_id = ",
    );
    new_iss_qb.push_bind(tenant_ctx.tenant_id());
    new_iss_qb
        .push(" AND ir.project_id = ")
        .push_bind(project.id);
    new_iss_qb
        .push(" AND ir.release_version = ")
        .push_bind(target_release.clone());
    new_iss_qb
        .push(
            " AND NOT EXISTS (
                 SELECT 1 FROM issue_releases base_ir
                 WHERE base_ir.tenant_id = ir.tenant_id
                   AND base_ir.project_id = ir.project_id
                   AND base_ir.issue_id = ir.issue_id
                   AND base_ir.release_version = ",
        )
        .push_bind(base_release.clone())
        .push(")");
    if let Some(env) = &env_filter {
        new_iss_qb.push(" AND i.last_environment = ").push_bind(env);
    }
    new_iss_qb.push(" ORDER BY ir.event_count DESC, i.last_seen DESC LIMIT 10");

    let new_issue_rows: Vec<NewIssueRow> = new_iss_qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("list new issues in target release: {err}"))
        })?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit compare releases: {err}")))?;

    let base_perf = overall_perf
        .remove(&base_release)
        .unwrap_or_else(zero_perf_metrics);
    let target_perf = overall_perf
        .remove(&target_release)
        .unwrap_or_else(zero_perf_metrics);

    let base_issues = issue_map
        .remove(&base_release)
        .unwrap_or_else(zero_issue_metrics);
    let target_issues = issue_map
        .remove(&target_release)
        .unwrap_or_else(zero_issue_metrics);

    let base_side = build_side(
        &base_release,
        meta_map.remove(&base_release),
        base_perf,
        base_issues,
    );
    let target_side = build_side(
        &target_release,
        meta_map.remove(&target_release),
        target_perf,
        target_issues,
    );

    let delta = compute_overall_delta(&base_side, &target_side);
    let verdict = compute_verdict(&base_side, &target_side, &delta);

    // Pair transactions across base & target releases.
    let mut base_tx_map: BTreeMap<(String, String), ReleasePerfMetrics> = BTreeMap::new();
    let mut target_tx_map: BTreeMap<(String, String), ReleasePerfMetrics> = BTreeMap::new();
    let mut all_keys: BTreeSet<(String, String)> = BTreeSet::new();

    for row in tx_rows {
        let key = (row.transaction_name, row.op);
        let metrics = build_perf_metrics(
            row.total_count,
            row.error_count,
            row.active_minutes,
            row.duration_sum_ms,
            row.duration_min_ms,
            row.duration_max_ms,
            &row.histogram_buckets,
        );
        all_keys.insert(key.clone());
        if row.release == base_release {
            base_tx_map.insert(key, metrics);
        } else if row.release == target_release {
            target_tx_map.insert(key, metrics);
        }
    }

    let mut compared_txs: Vec<TransactionComparisonRow> = all_keys
        .into_iter()
        .map(|(tx_name, op)| {
            let b = base_tx_map.remove(&(tx_name.clone(), op.clone()));
            let t = target_tx_map.remove(&(tx_name.clone(), op.clone()));
            build_tx_comparison_row(tx_name, op, b, t)
        })
        .collect();

    compared_txs.sort_by(|a, b| {
        status_sort_rank(&a.status)
            .cmp(&status_sort_rank(&b.status))
            .then_with(|| {
                let da = a.p95_delta_ms.unwrap_or(0.0).abs();
                let db = b.p95_delta_ms.unwrap_or(0.0).abs();
                db.total_cmp(&da)
            })
            .then_with(|| a.transaction_name.cmp(&b.transaction_name))
    });

    let total_transactions_compared = compared_txs.len();
    compared_txs.truncate(limit);

    let new_issues_in_target = new_issue_rows
        .into_iter()
        .map(|r| ReleaseNewIssueItem {
            id: r.id,
            title: r.title,
            exception_type: r.exception_type,
            level: r.level,
            status: r.status,
            is_regressed: r.is_regressed,
            event_count: r.event_count,
            first_seen: r.first_seen,
            last_seen: r.last_seen,
        })
        .collect();

    Ok(ReleaseComparisonView {
        project_id: project.id,
        project_slug: project.slug,
        workspace_id: project.workspace_id,
        workspace_slug: project.workspace_slug,
        environment: env_filter,
        since,
        until,
        verdict: verdict.to_owned(),
        base: base_side,
        target: target_side,
        delta,
        new_issues_in_target,
        transactions: compared_txs,
        total_transactions_compared,
    })
}

fn build_side(
    version: &str,
    meta: Option<ReleaseMetaRow>,
    performance: ReleasePerfMetrics,
    issues: ReleaseIssueMetrics,
) -> ReleaseComparisonSide {
    let commit_sha = meta
        .as_ref()
        .and_then(|m| m.commit_sha.clone())
        .or_else(|| infer_commit_sha(version));
    ReleaseComparisonSide {
        version: version.to_owned(),
        commit_sha,
        first_seen: meta.as_ref().map(|m| m.first_seen),
        last_seen: meta.as_ref().map(|m| m.last_seen),
        deployed_at: meta.and_then(|m| m.deployed_at),
        performance,
        issues,
    }
}

fn build_perf_metrics(
    total_count: i64,
    error_count: i64,
    active_minutes: i64,
    duration_sum_ms: f64,
    duration_min_ms: f64,
    duration_max_ms: f64,
    histogram_buckets: &[i64],
) -> ReleasePerfMetrics {
    let total = total_count.max(0);
    let errors = error_count.max(0);
    let active = active_minutes.max(0);
    let error_rate = if total > 0 {
        round_two_decimals((errors as f64) / (total as f64) * 10000.0) / 10000.0
    } else {
        0.0
    };
    let throughput_rpm = if total > 0 {
        round_two_decimals((total as f64) / (active.max(1) as f64))
    } else {
        0.0
    };
    let avg_ms = if total > 0 {
        round_two_decimals(duration_sum_ms / (total as f64))
    } else {
        0.0
    };

    ReleasePerfMetrics {
        total_count: total,
        error_count: errors,
        error_rate,
        active_minutes: active,
        throughput_rpm,
        p50_ms: estimate_percentile(histogram_buckets, duration_min_ms, duration_max_ms, 0.50),
        p95_ms: estimate_percentile(histogram_buckets, duration_min_ms, duration_max_ms, 0.95),
        p99_ms: estimate_percentile(histogram_buckets, duration_min_ms, duration_max_ms, 0.99),
        avg_ms,
        min_ms: round_two_decimals(duration_min_ms),
        max_ms: round_two_decimals(duration_max_ms),
    }
}

fn zero_perf_metrics() -> ReleasePerfMetrics {
    ReleasePerfMetrics {
        total_count: 0,
        error_count: 0,
        error_rate: 0.0,
        active_minutes: 0,
        throughput_rpm: 0.0,
        p50_ms: 0.0,
        p95_ms: 0.0,
        p99_ms: 0.0,
        avg_ms: 0.0,
        min_ms: 0.0,
        max_ms: 0.0,
    }
}

fn zero_issue_metrics() -> ReleaseIssueMetrics {
    ReleaseIssueMetrics {
        linked_issues_count: 0,
        new_issues_count: 0,
        regressed_issues_count: 0,
        unresolved_issues_count: 0,
        issue_events_count: 0,
    }
}

fn pct_change(base: f64, target: f64) -> Option<f64> {
    if base.abs() < f64::EPSILON {
        None
    } else {
        Some(round_two_decimals(((target - base) / base) * 100.0))
    }
}

fn compute_overall_delta(
    base: &ReleaseComparisonSide,
    target: &ReleaseComparisonSide,
) -> ReleaseComparisonDelta {
    let bp = &base.performance;
    let tp = &target.performance;
    ReleaseComparisonDelta {
        p50_delta_ms: round_two_decimals(tp.p50_ms - bp.p50_ms),
        p50_delta_pct: pct_change(bp.p50_ms, tp.p50_ms),
        p95_delta_ms: round_two_decimals(tp.p95_ms - bp.p95_ms),
        p95_delta_pct: pct_change(bp.p95_ms, tp.p95_ms),
        p99_delta_ms: round_two_decimals(tp.p99_ms - bp.p99_ms),
        p99_delta_pct: pct_change(bp.p99_ms, tp.p99_ms),
        avg_delta_ms: round_two_decimals(tp.avg_ms - bp.avg_ms),
        avg_delta_pct: pct_change(bp.avg_ms, tp.avg_ms),
        error_rate_delta: round_two_decimals((tp.error_rate - bp.error_rate) * 10000.0) / 10000.0,
        throughput_rpm_delta: round_two_decimals(tp.throughput_rpm - bp.throughput_rpm),
        throughput_rpm_delta_pct: pct_change(bp.throughput_rpm, tp.throughput_rpm),
        linked_issues_delta: target.issues.linked_issues_count - base.issues.linked_issues_count,
        new_issues_delta: target.issues.new_issues_count - base.issues.new_issues_count,
        regressed_issues_delta: target.issues.regressed_issues_count
            - base.issues.regressed_issues_count,
    }
}

fn compute_verdict(
    base: &ReleaseComparisonSide,
    target: &ReleaseComparisonSide,
    delta: &ReleaseComparisonDelta,
) -> &'static str {
    let has_perf = base.performance.total_count > 0 && target.performance.total_count > 0;
    let has_issues = base.issues.linked_issues_count > 0 || target.issues.linked_issues_count > 0;
    if !has_perf && !has_issues {
        return "insufficient_data";
    }

    let perf_regressed = has_perf
        && ((delta.p95_delta_ms >= 15.0 && delta.p95_delta_pct.unwrap_or(0.0) >= 20.0)
            || delta.error_rate_delta >= 0.02);
    let issues_regressed = target.issues.new_issues_count > base.issues.new_issues_count
        && target.issues.new_issues_count > 0;

    if perf_regressed || issues_regressed {
        return "regressed";
    }

    let perf_improved = has_perf
        && ((delta.p95_delta_ms <= -15.0 && delta.p95_delta_pct.unwrap_or(0.0) <= -15.0)
            || delta.error_rate_delta <= -0.02);
    if perf_improved {
        return "improved";
    }

    "stable"
}

fn build_tx_comparison_row(
    transaction_name: String,
    op: String,
    base: Option<ReleasePerfMetrics>,
    target: Option<ReleasePerfMetrics>,
) -> TransactionComparisonRow {
    match (&base, &target) {
        (Some(b), Some(t)) => {
            let p50_d = round_two_decimals(t.p50_ms - b.p50_ms);
            let p50_pct = pct_change(b.p50_ms, t.p50_ms);
            let p95_d = round_two_decimals(t.p95_ms - b.p95_ms);
            let p95_pct = pct_change(b.p95_ms, t.p95_ms);
            let p99_d = round_two_decimals(t.p99_ms - b.p99_ms);
            let p99_pct = pct_change(b.p99_ms, t.p99_ms);
            let err_d = round_two_decimals((t.error_rate - b.error_rate) * 10000.0) / 10000.0;
            let rpm_d = round_two_decimals(t.throughput_rpm - b.throughput_rpm);

            let status = if (p95_d >= 15.0 && p95_pct.unwrap_or(0.0) >= 20.0) || err_d >= 0.02 {
                "regressed"
            } else if (p95_d <= -15.0 && p95_pct.unwrap_or(0.0) <= -15.0) || err_d <= -0.02 {
                "improved"
            } else {
                "stable"
            };

            TransactionComparisonRow {
                transaction_name,
                op,
                status: status.to_owned(),
                base,
                target,
                p50_delta_ms: Some(p50_d),
                p50_delta_pct: p50_pct,
                p95_delta_ms: Some(p95_d),
                p95_delta_pct: p95_pct,
                p99_delta_ms: Some(p99_d),
                p99_delta_pct: p99_pct,
                error_rate_delta: Some(err_d),
                throughput_rpm_delta: Some(rpm_d),
            }
        }
        (None, Some(_)) => TransactionComparisonRow {
            transaction_name,
            op,
            status: "new".to_owned(),
            base,
            target,
            p50_delta_ms: None,
            p50_delta_pct: None,
            p95_delta_ms: None,
            p95_delta_pct: None,
            p99_delta_ms: None,
            p99_delta_pct: None,
            error_rate_delta: None,
            throughput_rpm_delta: None,
        },
        (Some(_), None) => TransactionComparisonRow {
            transaction_name,
            op,
            status: "removed".to_owned(),
            base,
            target,
            p50_delta_ms: None,
            p50_delta_pct: None,
            p95_delta_ms: None,
            p95_delta_pct: None,
            p99_delta_ms: None,
            p99_delta_pct: None,
            error_rate_delta: None,
            throughput_rpm_delta: None,
        },
        (None, None) => TransactionComparisonRow {
            transaction_name,
            op,
            status: "stable".to_owned(),
            base,
            target,
            p50_delta_ms: None,
            p50_delta_pct: None,
            p95_delta_ms: None,
            p95_delta_pct: None,
            p99_delta_ms: None,
            p99_delta_pct: None,
            error_rate_delta: None,
            throughput_rpm_delta: None,
        },
    }
}

fn status_sort_rank(status: &str) -> u8 {
    match status {
        "regressed" => 0,
        "new" => 1,
        "improved" => 2,
        "stable" => 3,
        "removed" => 4,
        _ => 5,
    }
}

fn validate_release_version(raw: &str, field: &'static str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.chars().any(char::is_whitespace) {
        return Err(AppError::Validation(format!(
            "{field} must be 1..=128 non-whitespace characters"
        )));
    }
    Ok(trimmed.to_owned())
}

fn resolve_compare_window(
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    hours: Option<u32>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), AppError> {
    let now = Utc::now();
    let end = until.unwrap_or(now);
    let start = match since {
        Some(s) => s,
        None => {
            let h = i64::from(hours.unwrap_or(DEFAULT_COMPARE_WINDOW_HOURS as u32));
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

async fn verify_tenant_slug(
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

async fn resolve_project(
    tx: &mut Transaction<'_, Postgres>,
    tenant_ctx: TenantContext,
    workspace_slug: Option<&str>,
    project_slug: &str,
) -> Result<ProjectIdentityRow, AppError> {
    if let Some(ws) = workspace_slug {
        let ws_trimmed = ws.trim();
        if !is_valid_slug(ws_trimmed) {
            return Err(AppError::Validation("invalid workspace_slug".to_owned()));
        }
        sqlx::query_as(
            "SELECT p.id, p.slug, p.workspace_id, w.slug AS workspace_slug
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
        )
        .bind(tenant_ctx.tenant_id())
        .bind(ws_trimmed)
        .bind(project_slug)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for compare: {err}")))?
        .ok_or(AppError::NotFound)
    } else {
        sqlx::query_as(
            "SELECT p.id, p.slug, p.workspace_id, w.slug AS workspace_slug
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             WHERE p.tenant_id = $1 AND p.slug = $2
             ORDER BY w.is_default DESC, p.id ASC
             LIMIT 1",
        )
        .bind(tenant_ctx.tenant_id())
        .bind(project_slug)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for compare: {err}")))?
        .ok_or(AppError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_regression_and_improvement_verdicts_and_deltas() {
        let base = ReleaseComparisonSide {
            version: "v1.0.0".to_owned(),
            commit_sha: None,
            first_seen: None,
            last_seen: None,
            deployed_at: None,
            performance: ReleasePerfMetrics {
                total_count: 100,
                error_count: 1,
                error_rate: 0.01,
                active_minutes: 10,
                throughput_rpm: 10.0,
                p50_ms: 40.0,
                p95_ms: 100.0,
                p99_ms: 180.0,
                avg_ms: 55.0,
                min_ms: 10.0,
                max_ms: 250.0,
            },
            issues: zero_issue_metrics(),
        };

        let slower_target = ReleaseComparisonSide {
            version: "v2.0.0".to_owned(),
            performance: ReleasePerfMetrics {
                p50_ms: 90.0,
                p95_ms: 250.0,
                p99_ms: 400.0,
                avg_ms: 110.0,
                ..base.performance.clone()
            },
            ..base.clone()
        };

        let delta = compute_overall_delta(&base, &slower_target);
        assert_eq!(delta.p95_delta_ms, 150.0);
        assert_eq!(delta.p95_delta_pct, Some(150.0));
        assert_eq!(compute_verdict(&base, &slower_target, &delta), "regressed");

        let faster_target = ReleaseComparisonSide {
            version: "v2.1.0".to_owned(),
            performance: ReleasePerfMetrics {
                p50_ms: 25.0,
                p95_ms: 60.0,
                p99_ms: 90.0,
                avg_ms: 35.0,
                ..base.performance.clone()
            },
            ..base.clone()
        };
        let faster_delta = compute_overall_delta(&base, &faster_target);
        assert_eq!(faster_delta.p95_delta_ms, -40.0);
        assert_eq!(
            compute_verdict(&base, &faster_target, &faster_delta),
            "improved"
        );
    }
}
