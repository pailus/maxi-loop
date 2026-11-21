use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
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
use crate::notes::{
    CreateIssueNoteInput, IssueNoteView, NoteActor, add_issue_note, list_issue_notes,
};
use crate::releases::{
    ReleaseListFilter, ReleaseView, UpsertReleaseInput, list_release_issues, list_releases,
    upsert_release,
};
use crate::service::{
    IssueScope, get_issue_detail, get_issue_event, get_issue_stats, list_issue_events, list_issues,
    update_issue_status_with_actor,
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
        .route(
            "/issues/{issue_id}/notes",
            get(default_list_issue_notes_handler).post(default_add_issue_note_handler),
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
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/{issue_id}/notes",
            get(project_list_issue_notes_handler).post(project_add_issue_note_handler),
        )
        // Release tracking routes
        .route(
            "/releases",
            get(list_releases_handler).post(upsert_release_handler),
        )
        .route(
            "/projects/{project_slug}/releases",
            get(project_list_releases_handler),
        )
        .route(
            "/releases/{release_id}/issues",
            get(list_release_issues_handler),
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

/// Narrow an `IssueScope` and `IssueListFilter` to the API token's single project when authenticated
/// via `Authorization: Bearer mlapi_...`, rejecting requests that explicitly target another project.
fn apply_token_to_issue_filter(
    session: &AuthSession,
    filter: &mut IssueListFilter,
) -> Result<(), AppError> {
    if let Some(tok) = &session.api_token {
        if let Some(pid) = filter.project_id
            && pid != tok.project_id
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        if let Some(slug) = filter.project_slug.as_deref()
            && !slug.trim().is_empty()
            && slug.trim() != tok.project_slug
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        if let Some(ws) = filter.workspace_slug.as_deref()
            && !ws.trim().is_empty()
            && ws.trim() != tok.workspace_slug
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different workspace",
            ));
        }
        filter.project_id = Some(tok.project_id);
        filter.project_slug = Some(tok.project_slug.clone());
        filter.workspace_slug = Some(tok.workspace_slug.clone());
    }
    Ok(())
}

fn token_scoped_issue_scope<'a>(
    session: &'a AuthSession,
    tenant_slug: &'a str,
    workspace_slug: Option<&'a str>,
    project_slug: Option<&'a str>,
) -> Result<IssueScope<'a>, AppError> {
    if let Some(tok) = &session.api_token {
        if let Some(ws) = workspace_slug
            && ws != tok.workspace_slug
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different workspace",
            ));
        }
        if let Some(proj) = project_slug
            && proj != tok.project_slug
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        return Ok(IssueScope {
            tenant_slug,
            workspace_slug: Some(&tok.workspace_slug),
            project_slug: Some(&tok.project_slug),
        });
    }
    Ok(IssueScope {
        tenant_slug,
        workspace_slug,
        project_slug,
    })
}

async fn default_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(mut filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    apply_token_to_issue_filter(&session, &mut filter)?;
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let mut res = list_issues(&state.db, session.tenant_ctx, scope, filter).await?;
    if session.tenant_role.is_none() && !session.is_api_token() {
        res.items
            .retain(|item| session.can_access_workspace(item.workspace_id));
    }
    Ok(Json(res))
}

async fn default_project_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
    Query(mut filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    apply_token_to_issue_filter(&session, &mut filter)?;
    let scope = token_scoped_issue_scope(
        &session,
        &session.tenant.slug,
        Some(&session.default_workspace.slug),
        Some(&project_slug),
    )?;
    let mut res = list_issues(&state.db, session.tenant_ctx, scope, filter).await?;
    if session.tenant_role.is_none() && !session.is_api_token() {
        res.items
            .retain(|item| session.can_access_workspace(item.workspace_id));
    }
    Ok(Json(res))
}

async fn tenant_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
    Query(mut filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    apply_token_to_issue_filter(&session, &mut filter)?;
    let scope = token_scoped_issue_scope(&session, &tenant_slug, None, None)?;
    let mut res = list_issues(&state.db, session.tenant_ctx, scope, filter).await?;
    if session.tenant_role.is_none() && !session.is_api_token() {
        res.items
            .retain(|item| session.can_access_workspace(item.workspace_id));
    }
    Ok(Json(res))
}

async fn project_list_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug)): Path<(String, String, String)>,
    Query(mut filter): Query<IssueListFilter>,
) -> Result<Json<PaginatedIssuesResponse>, AppError> {
    apply_token_to_issue_filter(&session, &mut filter)?;
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let mut res = list_issues(&state.db, session.tenant_ctx, scope, filter).await?;
    if session.tenant_role.is_none() && !session.is_api_token() {
        res.items
            .retain(|item| session.can_access_workspace(item.workspace_id));
    }
    Ok(Json(res))
}

async fn default_get_issue_detail_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Query(query): Query<DetailQuery>,
) -> Result<Json<IssueDetailView>, AppError> {
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let detail =
        get_issue_detail(&state.db, session.tenant_ctx, scope, issue_id, query.hours).await?;
    session.enforce_project_read(detail.issue.project_id, detail.issue.workspace_id)?;
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
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let detail =
        get_issue_detail(&state.db, session.tenant_ctx, scope, issue_id, query.hours).await?;
    session.enforce_project_read(detail.issue.project_id, detail.issue.workspace_id)?;
    Ok(Json(detail))
}

async fn default_get_issue_stats_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<IssueStatsResponse>, AppError> {
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let stats = get_issue_stats(
        &state.db,
        session.tenant_ctx,
        scope,
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
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let stats = get_issue_stats(
        &state.db,
        session.tenant_ctx,
        scope,
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
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let events =
        list_issue_events(&state.db, session.tenant_ctx, scope, issue_id, query.limit).await?;
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
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let events =
        list_issue_events(&state.db, session.tenant_ctx, scope, issue_id, query.limit).await?;
    Ok(Json(events))
}

async fn default_get_issue_event_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((issue_id, event_id)): Path<(i64, String)>,
) -> Result<Json<IssueEventView>, AppError> {
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let ev = get_issue_event(&state.db, session.tenant_ctx, scope, issue_id, &event_id).await?;
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
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let ev = get_issue_event(&state.db, session.tenant_ctx, scope, issue_id, &event_id).await?;
    Ok(Json(ev))
}

async fn default_update_issue_status_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Json(input): Json<UpdateIssueStatusInput>,
) -> Result<Json<IssueSummaryView>, AppError> {
    session.enforce_api_token_write()?;
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let detail = get_issue_detail(
        &state.db,
        session.tenant_ctx,
        scope.clone(),
        issue_id,
        Some(1),
    )
    .await?;
    session.enforce_project_write(detail.issue.project_id, detail.issue.workspace_id)?;

    let (updated, _) = update_issue_status_with_actor(
        &state.db,
        session.tenant_ctx,
        session.role,
        scope,
        issue_id,
        NoteActor {
            user_id: session.user.id,
            email: &session.user.email,
            source: if session.is_api_token() { "api" } else { "ui" },
        },
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
    session.enforce_api_token_write()?;
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let detail = get_issue_detail(
        &state.db,
        session.tenant_ctx,
        scope.clone(),
        issue_id,
        Some(1),
    )
    .await?;
    session.enforce_project_write(detail.issue.project_id, detail.issue.workspace_id)?;

    let (updated, _) = update_issue_status_with_actor(
        &state.db,
        session.tenant_ctx,
        session.role,
        scope,
        issue_id,
        NoteActor {
            user_id: session.user.id,
            email: &session.user.email,
            source: if session.is_api_token() { "api" } else { "ui" },
        },
        input,
    )
    .await?;
    Ok(Json(updated))
}

async fn default_list_issue_notes_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Query(query): Query<EventsListQuery>,
) -> Result<Json<Vec<IssueNoteView>>, AppError> {
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let notes =
        list_issue_notes(&state.db, session.tenant_ctx, scope, issue_id, query.limit).await?;
    Ok(Json(notes))
}

async fn project_list_issue_notes_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, issue_id)): Path<(
        String,
        String,
        String,
        i64,
    )>,
    Query(query): Query<EventsListQuery>,
) -> Result<Json<Vec<IssueNoteView>>, AppError> {
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let notes =
        list_issue_notes(&state.db, session.tenant_ctx, scope, issue_id, query.limit).await?;
    Ok(Json(notes))
}

async fn default_add_issue_note_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(issue_id): Path<i64>,
    Json(input): Json<CreateIssueNoteInput>,
) -> Result<(StatusCode, Json<IssueNoteView>), AppError> {
    session.enforce_api_token_write()?;
    let scope = token_scoped_issue_scope(&session, &session.tenant.slug, None, None)?;
    let detail = get_issue_detail(
        &state.db,
        session.tenant_ctx,
        scope.clone(),
        issue_id,
        Some(1),
    )
    .await?;
    session.enforce_project_write(detail.issue.project_id, detail.issue.workspace_id)?;

    let note = add_issue_note(
        &state.db,
        session.tenant_ctx,
        session.role,
        scope,
        issue_id,
        NoteActor {
            user_id: session.user.id,
            email: &session.user.email,
            source: if session.is_api_token() { "api" } else { "ui" },
        },
        input,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(note)))
}

async fn project_add_issue_note_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug, issue_id)): Path<(
        String,
        String,
        String,
        i64,
    )>,
    Json(input): Json<CreateIssueNoteInput>,
) -> Result<(StatusCode, Json<IssueNoteView>), AppError> {
    session.enforce_api_token_write()?;
    let scope = token_scoped_issue_scope(
        &session,
        &tenant_slug,
        Some(&workspace_slug),
        Some(&project_slug),
    )?;
    let detail = get_issue_detail(
        &state.db,
        session.tenant_ctx,
        scope.clone(),
        issue_id,
        Some(1),
    )
    .await?;
    session.enforce_project_write(detail.issue.project_id, detail.issue.workspace_id)?;

    let note = add_issue_note(
        &state.db,
        session.tenant_ctx,
        session.role,
        scope,
        issue_id,
        NoteActor {
            user_id: session.user.id,
            email: &session.user.email,
            source: if session.is_api_token() { "api" } else { "ui" },
        },
        input,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(note)))
}

async fn list_releases_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(mut filter): Query<ReleaseListFilter>,
) -> Result<Json<Vec<ReleaseView>>, AppError> {
    if let Some(tok) = &session.api_token {
        if let Some(slug) = filter.project_slug.as_deref()
            && !slug.trim().is_empty()
            && slug.trim() != tok.project_slug
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        if let Some(pid) = filter.project_id
            && pid != tok.project_id
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        filter.project_id = Some(tok.project_id);
        filter.project_slug = Some(tok.project_slug.clone());
    }
    let mut items = list_releases(&state.db, session.tenant_ctx, filter).await?;
    if session.tenant_role.is_none() && !session.is_api_token() {
        items.retain(|r| session.can_access_workspace(r.workspace_id));
    }
    Ok(Json(items))
}

async fn project_list_releases_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
    Query(mut filter): Query<ReleaseListFilter>,
) -> Result<Json<Vec<ReleaseView>>, AppError> {
    session.enforce_api_token_project_slug(&project_slug)?;
    filter.project_slug = Some(project_slug);
    if let Some(pid) = session.scoped_project_id() {
        filter.project_id = Some(pid);
    }
    let mut items = list_releases(&state.db, session.tenant_ctx, filter).await?;
    if session.tenant_role.is_none() && !session.is_api_token() {
        items.retain(|r| session.can_access_workspace(r.workspace_id));
    }
    Ok(Json(items))
}

async fn upsert_release_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(mut input): Json<UpsertReleaseInput>,
) -> Result<(StatusCode, Json<ReleaseView>), AppError> {
    session.enforce_api_token_write()?;
    if let Some(tok) = &session.api_token {
        if input.project_slug.trim() != tok.project_slug {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        input.workspace_slug = Some(tok.workspace_slug.clone());
    }
    let rel = upsert_release(&state.db, session.tenant_ctx, session.role, input).await?;
    Ok((StatusCode::CREATED, Json(rel)))
}

async fn list_release_issues_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(release_id): Path<i64>,
    Query(query): Query<EventsListQuery>,
) -> Result<Json<Vec<IssueSummaryView>>, AppError> {
    let mut items =
        list_release_issues(&state.db, session.tenant_ctx, release_id, query.limit).await?;
    if let Some(scoped_pid) = session.scoped_project_id() {
        items.retain(|i| i.project_id == scoped_pid);
    } else if session.tenant_role.is_none() {
        items.retain(|i| session.can_access_workspace(i.workspace_id));
    }
    Ok(Json(items))
}
