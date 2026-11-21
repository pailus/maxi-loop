use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::model::AuthSession;
use maxi_loop_platform::dsn_notify::ProjectDsnInfo;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;

use crate::model::{
    CreateProjectInput, CreateWorkspaceInput, ProjectView, TenantOverview, WorkspaceView,
};
use crate::service::{
    create_project, create_workspace, get_project, get_tenant_overview, get_workspace,
    list_projects, list_workspaces, rotate_project_dsn,
};

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // Current tenant & single-tenant convenience routes
        .route("/tenants/current", get(current_tenant_handler))
        .route(
            "/workspaces",
            get(default_list_workspaces_handler).post(default_create_workspace_handler),
        )
        .route(
            "/projects",
            get(default_list_projects_handler).post(default_create_project_handler),
        )
        .route("/projects/{project_slug}", get(default_get_project_handler))
        .route(
            "/projects/{project_slug}/dsn/rotate",
            post(default_rotate_project_dsn_handler),
        )
        // Explicit slug-scoped hierarchy routes: /t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}
        .route("/t/{tenant_slug}", get(tenant_by_slug_handler))
        .route(
            "/t/{tenant_slug}/workspaces",
            get(list_workspaces_handler).post(create_workspace_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}",
            get(get_workspace_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/projects",
            get(list_workspace_projects_handler).post(create_project_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}",
            get(get_project_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/dsn/rotate",
            post(rotate_project_dsn_handler),
        )
}

async fn current_tenant_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<TenantOverview>, AppError> {
    let overview = get_tenant_overview(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &session.tenant.slug,
    )
    .await?;
    Ok(Json(overview))
}

async fn tenant_by_slug_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
) -> Result<Json<TenantOverview>, AppError> {
    let overview =
        get_tenant_overview(&state.db, &state.config, session.tenant_ctx, &tenant_slug).await?;
    Ok(Json(overview))
}

async fn default_list_workspaces_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<WorkspaceView>>, AppError> {
    let items = list_workspaces(&state.db, session.tenant_ctx, &session.tenant.slug).await?;
    Ok(Json(items))
}

async fn list_workspaces_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
) -> Result<Json<Vec<WorkspaceView>>, AppError> {
    let items = list_workspaces(&state.db, session.tenant_ctx, &tenant_slug).await?;
    Ok(Json(items))
}

async fn default_create_workspace_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateWorkspaceInput>,
) -> Result<(StatusCode, Json<WorkspaceView>), AppError> {
    let created = create_workspace(
        &state.db,
        &state.config,
        session.tenant_ctx,
        session.role,
        &session.tenant.slug,
        input,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn create_workspace_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
    Json(input): Json<CreateWorkspaceInput>,
) -> Result<(StatusCode, Json<WorkspaceView>), AppError> {
    let created = create_workspace(
        &state.db,
        &state.config,
        session.tenant_ctx,
        session.role,
        &tenant_slug,
        input,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn get_workspace_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug)): Path<(String, String)>,
) -> Result<Json<WorkspaceView>, AppError> {
    let ws = get_workspace(&state.db, session.tenant_ctx, &tenant_slug, &workspace_slug).await?;
    Ok(Json(ws))
}

#[derive(Deserialize, Default)]
struct ProjectFilterQuery {
    workspace_slug: Option<String>,
}

async fn default_list_projects_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(query): Query<ProjectFilterQuery>,
) -> Result<Json<Vec<ProjectView>>, AppError> {
    let items = list_projects(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &session.tenant.slug,
        query.workspace_slug.as_deref(),
    )
    .await?;
    Ok(Json(items))
}

async fn list_workspace_projects_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug)): Path<(String, String)>,
) -> Result<Json<Vec<ProjectView>>, AppError> {
    let items = list_projects(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &tenant_slug,
        Some(&workspace_slug),
    )
    .await?;
    Ok(Json(items))
}

async fn default_create_project_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateProjectInput>,
) -> Result<(StatusCode, Json<ProjectView>), AppError> {
    let ws_slug = input
        .workspace_slug
        .clone()
        .unwrap_or_else(|| session.default_workspace.slug.clone());
    let created = create_project(
        &state.db,
        &state.config,
        session.tenant_ctx,
        session.role,
        &session.tenant.slug,
        &ws_slug,
        input,
    )
    .await?;
    notify_dsn_cache(&state, &created);
    Ok((StatusCode::CREATED, Json(created)))
}

async fn create_project_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug)): Path<(String, String)>,
    Json(input): Json<CreateProjectInput>,
) -> Result<(StatusCode, Json<ProjectView>), AppError> {
    let created = create_project(
        &state.db,
        &state.config,
        session.tenant_ctx,
        session.role,
        &tenant_slug,
        &workspace_slug,
        input,
    )
    .await?;
    notify_dsn_cache(&state, &created);
    Ok((StatusCode::CREATED, Json(created)))
}

async fn default_get_project_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
) -> Result<Json<ProjectView>, AppError> {
    let proj = get_project(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &session.tenant.slug,
        &session.default_workspace.slug,
        &project_slug,
    )
    .await?;
    Ok(Json(proj))
}

async fn get_project_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug)): Path<(String, String, String)>,
) -> Result<Json<ProjectView>, AppError> {
    let proj = get_project(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &tenant_slug,
        &workspace_slug,
        &project_slug,
    )
    .await?;
    Ok(Json(proj))
}

async fn default_rotate_project_dsn_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
) -> Result<Json<ProjectView>, AppError> {
    let proj = rotate_project_dsn(
        &state.db,
        &state.config,
        session.tenant_ctx,
        session.role,
        &session.tenant.slug,
        &session.default_workspace.slug,
        &project_slug,
    )
    .await?;
    notify_dsn_cache(&state, &proj);
    Ok(Json(proj))
}

async fn rotate_project_dsn_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug)): Path<(String, String, String)>,
) -> Result<Json<ProjectView>, AppError> {
    let proj = rotate_project_dsn(
        &state.db,
        &state.config,
        session.tenant_ctx,
        session.role,
        &tenant_slug,
        &workspace_slug,
        &project_slug,
    )
    .await?;
    notify_dsn_cache(&state, &proj);
    Ok(Json(proj))
}

fn notify_dsn_cache(state: &AuthState, proj: &ProjectView) {
    state.notify_project_dsn(ProjectDsnInfo {
        project_id: proj.id,
        tenant_id: proj.tenant_id,
        workspace_id: proj.workspace_id,
        dsn_public_key: proj.dsn_public_key.clone(),
        rate_limit_per_minute: u32::try_from(proj.rate_limit_per_minute.max(1)).unwrap_or(1200),
    });
}
