use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::model::AuthSession;
use maxi_loop_platform::access::AccessGuard;
use maxi_loop_platform::dsn_notify::ProjectDsnInfo;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;

use crate::model::{
    CreateProjectInput, CreateWorkspaceInput, ProjectView, TenantOverview,
    UpdateProjectSettingsInput, UpdateTenantSettingsInput, UpdateWorkspaceSettingsInput,
    WorkspaceView,
};
use crate::pii::{
    CreatePiiScrubRuleInput, PiiScrubRuleView, UpdatePiiScrubRuleInput, create_pii_scrub_rule,
    delete_pii_scrub_rule, list_pii_scrub_rules, update_pii_scrub_rule,
};
use crate::service::{
    create_project, create_workspace, get_project, get_tenant_overview, get_workspace,
    list_projects, list_workspaces, rotate_project_dsn, update_project_settings,
    update_tenant_settings, update_workspace_settings,
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
            "/tenants/current/settings",
            patch(update_current_tenant_settings_handler),
        )
        .route(
            "/workspaces",
            get(default_list_workspaces_handler).post(default_create_workspace_handler),
        )
        .route(
            "/workspaces/{workspace_slug}/settings",
            patch(default_update_workspace_settings_handler),
        )
        .route(
            "/projects",
            get(default_list_projects_handler).post(default_create_project_handler),
        )
        .route("/projects/{project_slug}", get(default_get_project_handler))
        .route(
            "/projects/{project_slug}/settings",
            patch(default_update_project_settings_handler),
        )
        .route(
            "/projects/{project_slug}/dsn/rotate",
            post(default_rotate_project_dsn_handler),
        )
        // Explicit slug-scoped hierarchy routes: /t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}
        .route("/t/{tenant_slug}", get(tenant_by_slug_handler))
        .route(
            "/t/{tenant_slug}/settings",
            patch(update_tenant_settings_handler),
        )
        .route(
            "/t/{tenant_slug}/workspaces",
            get(list_workspaces_handler).post(create_workspace_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}",
            get(get_workspace_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/settings",
            patch(update_workspace_settings_handler),
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
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/settings",
            patch(update_project_settings_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/dsn/rotate",
            post(rotate_project_dsn_handler),
        )
        // Configurable inherited PII scrubbing rules (additive to mandatory baseline)
        .route(
            "/pii-rules",
            get(list_pii_rules_handler).post(create_pii_rule_handler),
        )
        .route(
            "/pii-rules/{rule_id}",
            patch(update_pii_rule_handler).delete(delete_pii_rule_handler),
        )
}

async fn current_tenant_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<TenantOverview>, AppError> {
    session.require_user_session()?;
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
    session.require_user_session()?;
    let overview =
        get_tenant_overview(&state.db, &state.config, session.tenant_ctx, &tenant_slug).await?;
    Ok(Json(overview))
}

async fn update_current_tenant_settings_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<UpdateTenantSettingsInput>,
) -> Result<Json<TenantOverview>, AppError> {
    session.require_user_session()?;
    if !session.can_manage_tenant() {
        return Err(AppError::Forbidden(
            "only tenant owners or tenant admins can update tenant settings",
        ));
    }
    let (overview, rate_update) = update_tenant_settings(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &session.tenant.slug,
        input,
    )
    .await?;
    state.notify_tenant_rate_limits(rate_update);
    Ok(Json(overview))
}

async fn update_tenant_settings_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
    Json(input): Json<UpdateTenantSettingsInput>,
) -> Result<Json<TenantOverview>, AppError> {
    session.require_user_session()?;
    if !session.can_manage_tenant() {
        return Err(AppError::Forbidden(
            "only tenant owners or tenant admins can update tenant settings",
        ));
    }
    let (overview, rate_update) = update_tenant_settings(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &tenant_slug,
        input,
    )
    .await?;
    state.notify_tenant_rate_limits(rate_update);
    Ok(Json(overview))
}

async fn default_list_workspaces_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<WorkspaceView>>, AppError> {
    session.require_user_session()?;
    let items = list_workspaces(&state.db, session.tenant_ctx, &session.tenant.slug).await?;
    let visible: Vec<WorkspaceView> = items
        .into_iter()
        .filter(|ws| session.can_access_workspace(ws.id))
        .collect();
    Ok(Json(visible))
}

async fn list_workspaces_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(tenant_slug): Path<String>,
) -> Result<Json<Vec<WorkspaceView>>, AppError> {
    session.require_user_session()?;
    let items = list_workspaces(&state.db, session.tenant_ctx, &tenant_slug).await?;
    let visible: Vec<WorkspaceView> = items
        .into_iter()
        .filter(|ws| session.can_access_workspace(ws.id))
        .collect();
    Ok(Json(visible))
}

async fn default_create_workspace_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateWorkspaceInput>,
) -> Result<(StatusCode, Json<WorkspaceView>), AppError> {
    session.require_user_session()?;
    if !session.can_manage_tenant() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
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
    session.require_user_session()?;
    if !session.can_manage_tenant() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
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
    session.require_user_session()?;
    let ws = get_workspace(&state.db, session.tenant_ctx, &tenant_slug, &workspace_slug).await?;
    if !session.can_access_workspace(ws.id) {
        return Err(AppError::Forbidden("insufficient workspace permissions"));
    }
    Ok(Json(ws))
}

async fn default_update_workspace_settings_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(workspace_slug): Path<String>,
    Json(input): Json<UpdateWorkspaceSettingsInput>,
) -> Result<Json<WorkspaceView>, AppError> {
    let tenant_slug = session.tenant.slug.clone();
    update_workspace_settings_inner(state, session, tenant_slug, workspace_slug, input).await
}

async fn update_workspace_settings_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug)): Path<(String, String)>,
    Json(input): Json<UpdateWorkspaceSettingsInput>,
) -> Result<Json<WorkspaceView>, AppError> {
    update_workspace_settings_inner(state, session, tenant_slug, workspace_slug, input).await
}

async fn update_workspace_settings_inner(
    state: AuthState,
    session: AuthSession,
    tenant_slug: String,
    workspace_slug: String,
    input: UpdateWorkspaceSettingsInput,
) -> Result<Json<WorkspaceView>, AppError> {
    session.require_user_session()?;
    let existing =
        get_workspace(&state.db, session.tenant_ctx, &tenant_slug, &workspace_slug).await?;
    if !session.can_manage_workspace(existing.id) {
        return Err(AppError::Forbidden("insufficient workspace permissions"));
    }
    let (updated, rate_update) = update_workspace_settings(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &tenant_slug,
        &workspace_slug,
        input,
    )
    .await?;
    state.notify_tenant_rate_limits(rate_update);
    Ok(Json(updated))
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
    let visible = filter_visible_projects(&session, items);
    Ok(Json(visible))
}

async fn list_workspace_projects_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug)): Path<(String, String)>,
) -> Result<Json<Vec<ProjectView>>, AppError> {
    let ws = get_workspace(&state.db, session.tenant_ctx, &tenant_slug, &workspace_slug).await?;
    if !session.can_access_workspace(ws.id) {
        return Err(AppError::Forbidden("insufficient workspace permissions"));
    }
    let items = list_projects(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &tenant_slug,
        Some(&workspace_slug),
    )
    .await?;
    let visible = filter_visible_projects(&session, items);
    Ok(Json(visible))
}

fn filter_visible_projects(session: &AuthSession, items: Vec<ProjectView>) -> Vec<ProjectView> {
    items
        .into_iter()
        .filter(|p| session.can_read_project(p.id, p.workspace_id))
        .collect()
}

async fn default_create_project_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateProjectInput>,
) -> Result<(StatusCode, Json<ProjectView>), AppError> {
    session.require_user_session()?;
    let ws_slug = input
        .workspace_slug
        .clone()
        .unwrap_or_else(|| session.default_workspace.slug.clone());
    let ws = get_workspace(
        &state.db,
        session.tenant_ctx,
        &session.tenant.slug,
        &ws_slug,
    )
    .await?;
    if !session.can_manage_workspace(ws.id) {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
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
    session.require_user_session()?;
    let ws = get_workspace(&state.db, session.tenant_ctx, &tenant_slug, &workspace_slug).await?;
    if !session.can_manage_workspace(ws.id) {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
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
    session.enforce_api_token_project_slug(&project_slug)?;
    let proj = get_project(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &session.tenant.slug,
        &session.default_workspace.slug,
        &project_slug,
    )
    .await?;
    session.enforce_project_read(proj.id, proj.workspace_id)?;
    Ok(Json(proj))
}

async fn get_project_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug)): Path<(String, String, String)>,
) -> Result<Json<ProjectView>, AppError> {
    session.enforce_api_token_project_slug(&project_slug)?;
    let proj = get_project(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &tenant_slug,
        &workspace_slug,
        &project_slug,
    )
    .await?;
    session.enforce_project_read(proj.id, proj.workspace_id)?;
    Ok(Json(proj))
}

async fn default_update_project_settings_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
    Query(query): Query<ProjectFilterQuery>,
    Json(input): Json<UpdateProjectSettingsInput>,
) -> Result<Json<ProjectView>, AppError> {
    let tenant_slug = session.tenant.slug.clone();
    let ws_slug = query
        .workspace_slug
        .unwrap_or_else(|| session.default_workspace.slug.clone());
    update_project_settings_inner(state, session, tenant_slug, ws_slug, project_slug, input).await
}

async fn update_project_settings_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path((tenant_slug, workspace_slug, project_slug)): Path<(String, String, String)>,
    Json(input): Json<UpdateProjectSettingsInput>,
) -> Result<Json<ProjectView>, AppError> {
    update_project_settings_inner(
        state,
        session,
        tenant_slug,
        workspace_slug,
        project_slug,
        input,
    )
    .await
}

async fn update_project_settings_inner(
    state: AuthState,
    session: AuthSession,
    tenant_slug: String,
    workspace_slug: String,
    project_slug: String,
    input: UpdateProjectSettingsInput,
) -> Result<Json<ProjectView>, AppError> {
    session.require_user_session()?;
    let ws = get_workspace(&state.db, session.tenant_ctx, &tenant_slug, &workspace_slug).await?;
    if !session.can_manage_workspace(ws.id) {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    let updated = update_project_settings(
        &state.db,
        &state.config,
        session.tenant_ctx,
        &tenant_slug,
        &workspace_slug,
        &project_slug,
        input,
    )
    .await?;
    notify_dsn_cache(&state, &updated);
    Ok(Json(updated))
}

async fn default_rotate_project_dsn_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(project_slug): Path<String>,
) -> Result<Json<ProjectView>, AppError> {
    session.require_user_session()?;
    let ws = get_workspace(
        &state.db,
        session.tenant_ctx,
        &session.tenant.slug,
        &session.default_workspace.slug,
    )
    .await?;
    if !session.can_manage_workspace(ws.id) {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
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
    session.require_user_session()?;
    let ws = get_workspace(&state.db, session.tenant_ctx, &tenant_slug, &workspace_slug).await?;
    if !session.can_manage_workspace(ws.id) {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
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
        rate_limit_per_minute: u32::try_from(proj.effective_rate_limit_per_minute.max(1))
            .unwrap_or(1200),
    });
}

async fn list_pii_rules_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<PiiScrubRuleView>>, AppError> {
    session.require_user_session()?;
    let mut rules = list_pii_scrub_rules(&state.db, session.tenant_ctx).await?;
    rules.retain(|r| session.can_read_scope(r.workspace_id, r.project_id));
    Ok(Json(rules))
}

async fn create_pii_rule_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreatePiiScrubRuleInput>,
) -> Result<(StatusCode, Json<PiiScrubRuleView>), AppError> {
    session.require_user_session()?;
    let created = create_pii_scrub_rule(&state.db, session.tenant_ctx, &session, input).await?;
    if let Some(entry) = created.to_cache_entry() {
        state.notify_pii_rule_upserted(entry);
    }
    Ok((StatusCode::CREATED, Json(created)))
}

async fn update_pii_rule_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(rule_id): Path<i64>,
    Json(input): Json<UpdatePiiScrubRuleInput>,
) -> Result<Json<PiiScrubRuleView>, AppError> {
    session.require_user_session()?;
    let updated =
        update_pii_scrub_rule(&state.db, session.tenant_ctx, &session, rule_id, input).await?;
    if let Some(entry) = updated.to_cache_entry() {
        state.notify_pii_rule_upserted(entry);
    }
    Ok(Json(updated))
}

async fn delete_pii_rule_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(rule_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    session.require_user_session()?;
    delete_pii_scrub_rule(&state.db, session.tenant_ctx, &session, rule_id).await?;
    state.notify_pii_rule_deleted(session.tenant_ctx.tenant_id(), rule_id);
    Ok(StatusCode::NO_CONTENT)
}
