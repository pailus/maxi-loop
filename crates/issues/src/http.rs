use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{get, patch};
use axum::{Json, Router};
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::model::AuthSession;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;

use crate::model::{
    IssueDetailView, IssueEventView, IssueListFilter, IssueStatsResponse, IssueSummaryView,
    PaginatedIssuesResponse, UpdateIssueStatusInput,
};
use crate::service::{
    IssueScope, get_issue_detail, get_issue_event, get_issue_stats, list_issue_events, list_issues,
    update_issue_status,
};

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // Single-tenant & default convenience routes
        .route("/issues", get(default_list_issues_handler))
        .route(
            "/projects/{project_slug}/issues",
            get(default_project_list_issues_handler),
        )
        .route(
            "/issues/{issue_id}",
            get(default_get_issue_detail_handler).patch(default_update_issue_status_handler),
        )
        .route(
            "/issues/{issue_id}/status",
            patch(default_update_issue_status_handler).post(default_update_issue_status_handler),
        )
        .route(
            "/issues/{issue_id}/stats",
            get(default_get_issue_stats_handler),
        )
        .route(
            "/issues/{issue_id}/events",
            get(default_list_issue_events_handler),
        )
        .route(
            "/issues/{issue_id}/events/{event_id}",
            get(default_get_issue_event_handler),
        )
        // Explicit slug-scoped hierarchy routes: /t/{tenant_slug}/...
        .route("/t/{tenant_slug}/issues", get(tenant_list_issues_handler))
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues",
            get(project_list_issues_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/{issue_id}",
            get(project_get_issue_detail_handler).patch(project_update_issue_status_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/{issue_id}/status",
            patch(project_update_issue_status_handler).post(project_update_issue_status_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/{issue_id}/stats",
            get(project_get_issue_stats_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/{issue_id}/events",
            get(project_list_issue_events_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/{issue_id}/events/{event_id}",
            get(project_get_issue_event_handler),
        )
}

#[derive(Deserialize, Default)]
struct DetailQuery {
    hours: Option<u32>,
}

#[derive(Deserialize, Default)]
struct StatsQuery {
    interval: Option<String>,
    hours: Option<u32>,
}

#[derive(Deserialize, Default)]
struct EventsListQuery {
    limit: Option<u32>,
}

async fn default_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    let res = list_issues(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn default_project_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
    Query(filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    let res = list_issues(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: Some(&session.default_workspace.slug),
            project_slug: Some(&project_slug),
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn tenant_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
    Query(filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    let res = list_issues(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &tenant_slug,
            workspace_slug: None,
            project_slug: None,
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn project_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug)): Path<(String, String, String)>,
    Query(filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    let res = list_issues(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        filter,
    )
    .await?;
    Ok(Json(res))
}

async fn default_get_issue_detail_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Query(query): Query<DetailQuery>,
) -> Result<Json<IssueDetailView>, AppError> {
    let detail = get_issue_detail(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        issue_id,
        query.hours,
    )
    .await?;
    Ok(Json(detail))
}

async fn project_get_issue_detail_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, issue_id)): Path<(
        String,
        String,
        String,
        i64,
    )>,
    Query(query): Query<DetailQuery>,
) -> Result<Json<IssueDetailView>, AppError> {
    let detail = get_issue_detail(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        issue_id,
        query.hours,
    )
    .await?;
    Ok(Json(detail))
}

async fn default_get_issue_stats_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<IssueStatsResponse>, AppError> {
    let stats = get_issue_stats(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        issue_id,
        query.interval.as_deref(),
        query.hours,
    )
    .await?;
    Ok(Json(stats))
}

async fn project_get_issue_stats_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, issue_id)): Path<(
        String,
        String,
        String,
        i64,
    )>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<IssueStatsResponse>, AppError> {
    let stats = get_issue_stats(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        issue_id,
        query.interval.as_deref(),
        query.hours,
    )
    .await?;
    Ok(Json(stats))
}

async fn default_list_issue_events_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Query(query): Query<EventsListQuery>,
) -> Result<Json<Vec<IssueEventView>>, AppError> {
    let events = list_issue_events(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        issue_id,
        query.limit,
    )
    .await?;
    Ok(Json(events))
}

async fn project_list_issue_events_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, issue_id)): Path<(
        String,
        String,
        String,
        i64,
    )>,
    Query(query): Query<EventsListQuery>,
) -> Result<Json<Vec<IssueEventView>>, AppError> {
    let events = list_issue_events(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        issue_id,
        query.limit,
    )
    .await?;
    Ok(Json(events))
}

async fn default_get_issue_event_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((issue_id, event_id)): Path<(i64, String)>,
) -> Result<Json<IssueEventView>, AppError> {
    let ev = get_issue_event(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        issue_id,
        &event_id,
    )
    .await?;
    Ok(Json(ev))
}

async fn project_get_issue_event_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, issue_id, event_id)): Path<(
        String,
        String,
        String,
        i64,
        String,
    )>,
) -> Result<Json<IssueEventView>, AppError> {
    let ev = get_issue_event(
        &state.db,
        session.tenant_ctx,
        IssueScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        issue_id,
        &event_id,
    )
    .await?;
    Ok(Json(ev))
}

async fn default_update_issue_status_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Json(input): Json<UpdateIssueStatusInput>,
) -> Result<Json<IssueSummaryView>, AppError> {
    let updated = update_issue_status(
        &state.db,
        session.tenant_ctx,
        session.role,
        IssueScope {
            tenant_slug: &session.tenant.slug,
            workspace_slug: None,
            project_slug: None,
        },
        issue_id,
        input,
    )
    .await?;
    Ok(Json(updated))
}

async fn project_update_issue_status_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, issue_id)): Path<(
        String,
        String,
        String,
        i64,
    )>,
    Json(input): Json<UpdateIssueStatusInput>,
) -> Result<Json<IssueSummaryView>, AppError> {
    let updated = update_issue_status(
        &state.db,
        session.tenant_ctx,
        session.role,
        IssueScope {
            tenant_slug: &tenant_slug,
            workspace_slug: Some(&workspace_slug),
            project_slug: Some(&project_slug),
        },
        issue_id,
        input,
    )
    .await?;
    Ok(Json(updated))
}
