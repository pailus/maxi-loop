use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::model::AuthSession;
use maxi_loop_platform::config::Config;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;
use sqlx::PgPool;

use crate::model::{
    SourceMapListFilter, SourceMapView, UploadSourceMapJsonInput, UploadSourceMapRawInput,
};
use crate::resolver::SourceMapResolver;
use crate::service::{delete_source_map, list_source_maps, upload_source_map};

#[derive(Clone)]
pub struct SourceMapState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub resolver: SourceMapResolver,
}

impl SourceMapState {
    pub fn new(config: Arc<Config>, db: PgPool, resolver: SourceMapResolver) -> Self {
        Self {
            config,
            db,
            resolver,
        }
    }
}

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    SourceMapState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    // Allow up to 64 MiB body so our handler returns structured 413 JSON based on `sourcemap_max_upload_bytes`.
    Router::new()
        .route(
            "/sourcemaps",
            get(list_sourcemaps_handler).post(upload_sourcemap_json_handler),
        )
        .route(
            "/sourcemaps/{sourcemap_id}",
            delete(delete_sourcemap_handler),
        )
        .route(
            "/projects/{project_slug}/releases/{release_version}/sourcemaps",
            post(upload_sourcemap_raw_handler),
        )
        .route(
            "/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/sourcemaps",
            get(scoped_list_sourcemaps_handler),
        )
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}

#[derive(Deserialize)]
struct RawUploadQuery {
    filename: String,
    debug_id: Option<String>,
    workspace_slug: Option<String>,
}

async fn list_sourcemaps_handler(
    State(state): State<SourceMapState>,
    session: AuthSession,
    Query(mut filter): Query<SourceMapListFilter>,
) -> Result<Json<Vec<SourceMapView>>, AppError> {
    if let Some(tok) = &session.api_token {
        if let Some(slug) = filter.project_slug.as_deref()
            && !slug.trim().is_empty()
            && slug.trim() != tok.project_slug
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        filter.project_slug = Some(tok.project_slug.clone());
    }
    let mut items = list_source_maps(&state.db, session.tenant_ctx, filter).await?;
    if let Some(scoped_pid) = session.scoped_project_id() {
        items.retain(|m| m.project_id == scoped_pid);
    } else if session.tenant_role.is_none() {
        items.retain(|m| session.can_access_workspace(m.workspace_id));
    }
    Ok(Json(items))
}

async fn scoped_list_sourcemaps_handler(
    State(state): State<SourceMapState>,
    session: AuthSession,
    Path((tenant_slug, _workspace_slug, project_slug)): Path<(String, String, String)>,
    Query(mut filter): Query<SourceMapListFilter>,
) -> Result<Json<Vec<SourceMapView>>, AppError> {
    if session.tenant.slug != tenant_slug {
        return Err(AppError::NotFound);
    }
    session.enforce_api_token_project_slug(&project_slug)?;
    filter.project_slug = Some(project_slug);
    let mut items = list_source_maps(&state.db, session.tenant_ctx, filter).await?;
    if let Some(scoped_pid) = session.scoped_project_id() {
        items.retain(|m| m.project_id == scoped_pid);
    } else if session.tenant_role.is_none() {
        items.retain(|m| session.can_access_workspace(m.workspace_id));
    }
    Ok(Json(items))
}

async fn upload_sourcemap_json_handler(
    State(state): State<SourceMapState>,
    session: AuthSession,
    Json(mut input): Json<UploadSourceMapJsonInput>,
) -> Result<(StatusCode, Json<SourceMapView>), AppError> {
    session.enforce_api_token_write()?;
    if let Some(tok) = &session.api_token {
        if input.project_slug.trim() != tok.project_slug {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        input.workspace_slug = Some(tok.workspace_slug.clone());
    }
    let view = upload_source_map(
        &state.db,
        &state.resolver,
        session.tenant_ctx,
        session.role,
        session.user.id,
        state.config.sourcemap_max_upload_bytes,
        UploadSourceMapRawInput {
            project_slug: input.project_slug,
            workspace_slug: input.workspace_slug,
            release_version: input.release_version,
            filename: input.filename,
            debug_id: input.debug_id,
            raw_bytes: input.content.into_bytes(),
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn upload_sourcemap_raw_handler(
    State(state): State<SourceMapState>,
    session: AuthSession,
    Path((project_slug, release_version)): Path<(String, String)>,
    Query(mut query): Query<RawUploadQuery>,
    body: Bytes,
) -> Result<(StatusCode, Json<SourceMapView>), AppError> {
    session.enforce_api_token_write()?;
    if let Some(tok) = &session.api_token {
        if project_slug.trim() != tok.project_slug {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        query.workspace_slug = Some(tok.workspace_slug.clone());
    }
    let view = upload_source_map(
        &state.db,
        &state.resolver,
        session.tenant_ctx,
        session.role,
        session.user.id,
        state.config.sourcemap_max_upload_bytes,
        UploadSourceMapRawInput {
            project_slug,
            workspace_slug: query.workspace_slug,
            release_version,
            filename: query.filename,
            debug_id: query.debug_id,
            raw_bytes: body.to_vec(),
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn delete_sourcemap_handler(
    State(state): State<SourceMapState>,
    session: AuthSession,
    Path(sourcemap_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    session.enforce_api_token_write()?;
    if let Some(scoped_pid) = session.scoped_project_id() {
        let existing = list_source_maps(
            &state.db,
            session.tenant_ctx,
            SourceMapListFilter {
                project_slug: session.scoped_project_slug().map(ToOwned::to_owned),
                release_version: None,
                limit: Some(500),
            },
        )
        .await?;
        if !existing
            .iter()
            .any(|m| m.id == sourcemap_id && m.project_id == scoped_pid)
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
    }
    delete_source_map(
        &state.db,
        &state.resolver,
        session.tenant_ctx,
        session.role,
        sourcemap_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
