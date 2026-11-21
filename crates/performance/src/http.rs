use axum::extract::{FromRef, Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::model::AuthSession;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;

use crate::model::{
    PaginatedTransactionsResponse, PerformanceScope, TraceListFilter, TraceSummaryView,
    TraceWaterfallView, TransactionDetailView, TransactionListFilter,
};
use crate::service::{
    TransactionDetailQueryParams, get_trace, get_transaction_detail, list_traces, list_transactions,
};

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // Convenience routes (single-tenant & default workspace)
        .route(
            "/performance/transactions",
            get(default_list_transactions_handler),
        )
        .route(
            "/projects/{project_slug}/performance/transactions",
            get(default_project_list_transactions_handler),
        )
        .route(
            "/performance/transactions/detail",
            get(default_transaction_detail_handler),
        )
        .route("/performance/traces", get(default_list_traces_handler))
        .route(
            "/performance/traces/{trace_id}",
            get(default_get_trace_handler),
        )
        // Explicit slug-scoped hierarchy routes: /t/{tenant_slug}/...
        .route(
            "/t/{tenant_slug}/performance/transactions",
            get(tenant_list_transactions_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/performance/transactions",
            get(project_list_transactions_handler),
        )
        .route(
            "/t/{tenant_slug}/performance/traces/{trace_id}",
            get(tenant_get_trace_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/performance/traces/{trace_id}",
            get(project_get_trace_handler),
        )
}

#[derive(Deserialize)]
struct TransactionDetailQuery {
    project_slug: String,
    transaction_name: String,
    op: Option<String>,
    environment: Option<String>,
    release: Option<String>,
    hours: Option<u32>,
}

async fn default_list_transactions_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(filter): Query<TransactionListFilter>,
) -> Result<Json<PaginatedTransactionsResponse>, AppError> {
    let res = list_transactions(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn default_project_list_transactions_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
    Query(filter): Query<TransactionListFilter>,
) -> Result<Json<PaginatedTransactionsResponse>, AppError> {
    let res = list_transactions(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: Some(&session.default_workspace.slug),
            project_slug: Some(&project_slug),
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn tenant_list_transactions_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
    Query(filter): Query<TransactionListFilter>,
) -> Result<Json<PaginatedTransactionsResponse>, AppError> {
    let res = list_transactions(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &tenant_slug,
            workspace_slug: None,
            project_slug: None,
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn project_list_transactions_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug)): Path<(String, String, String)>,
    Query(filter): Query<TransactionListFilter>,
) -> Result<Json<PaginatedTransactionsResponse>, AppError> {
    let res = list_transactions(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn default_transaction_detail_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(query): Query<TransactionDetailQuery>,
) -> Result<Json<TransactionDetailView>, AppError> {
    let detail = get_transaction_detail(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        TransactionDetailQueryParams {
            project_slug: &query.project_slug,
            transaction_name: &query.transaction_name,
            op: query.op.as_deref(),
            environment: query.environment.as_deref(),
            release: query.release.as_deref(),
            hours: query.hours,
        },
    )
    .await?;
    Ok(Json(detail))
}

async fn default_list_traces_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(filter): Query<TraceListFilter>,
) -> Result<Json<Vec<TraceSummaryView>>, AppError> {
    let traces = list_traces(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        filter,
    )
    .await?;
    Ok(Json(traces))
}

async fn default_get_trace_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(trace_id): Path<String>,
) -> Result<Json<TraceWaterfallView>, AppError> {
    let wf = get_trace(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        &trace_id,
        &[],
    )
    .await?;
    Ok(Json(wf))
}

async fn tenant_get_trace_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, trace_id)): Path<(String, String)>,
) -> Result<Json<TraceWaterfallView>, AppError> {
    let wf = get_trace(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &tenant_slug,
            workspace_slug: None,
            project_slug: None,
        },
        &trace_id,
        &[],
    )
    .await?;
    Ok(Json(wf))
}

async fn project_get_trace_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, trace_id)): Path<(
        String,
        String,
        String,
        String,
    )>,
) -> Result<Json<TraceWaterfallView>, AppError> {
    let wf = get_trace(
        &state.db,
        session.tenant_ctx,
        PerformanceScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        &trace_id,
        &[],
    )
    .await?;
    Ok(Json(wf))
}
