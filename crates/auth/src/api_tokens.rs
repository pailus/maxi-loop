use std::collections::BTreeMap;

use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx, begin_worker_tx};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use tracing::info;

use crate::http::AuthState;
use crate::model::{ApiTokenAuthScope, AuthSession, AuthenticatedUser, Role, WorkspaceSummary};
use crate::service::load_tenant_summary;
use crate::token::generate_random_token_hex;

pub const API_TOKEN_PREFIX: &str = "mlapi_";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiTokenView {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub project_id: i64,
    pub project_slug: String,
    pub created_by_user_id: i64,
    pub created_by_email: String,
    pub name: String,
    pub token_prefix: String,
    pub can_read: bool,
    pub can_write: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedApiTokenResponse {
    pub token: ApiTokenView,
    /// Returned ONCE at creation time; raw token is never persisted or retrievable again.
    pub raw_token: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateApiTokenInput {
    pub project_slug: String,
    pub workspace_slug: Option<String>,
    pub name: String,
    /// Defaults to `false` (read-only least-privilege by default).
    pub can_write: Option<bool>,
    /// Optional lifetime in days (`1..=365`). `None` means no expiry until revoked.
    pub expires_in_days: Option<u16>,
}

#[derive(Deserialize, Default)]
struct ApiTokenListQuery {
    project_slug: Option<String>,
}

#[derive(FromRow)]
struct ApiTokenDbRow {
    id: i64,
    tenant_id: i64,
    workspace_id: i64,
    workspace_slug: String,
    project_id: i64,
    project_slug: String,
    created_by_user_id: i64,
    created_by_email: String,
    name: String,
    token_prefix: String,
    can_read: bool,
    can_write: bool,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl From<ApiTokenDbRow> for ApiTokenView {
    fn from(r: ApiTokenDbRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            workspace_id: r.workspace_id,
            workspace_slug: r.workspace_slug,
            project_id: r.project_id,
            project_slug: r.project_slug,
            created_by_user_id: r.created_by_user_id,
            created_by_email: r.created_by_email,
            name: r.name,
            token_prefix: r.token_prefix,
            can_read: r.can_read,
            can_write: r.can_write,
            expires_at: r.expires_at,
            revoked_at: r.revoked_at,
            last_used_at: r.last_used_at,
            created_at: r.created_at,
        }
    }
}

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api-tokens",
            get(list_api_tokens_handler).post(create_api_token_handler),
        )
        .route("/api-tokens/{token_id}", delete(revoke_api_token_handler))
}

async fn list_api_tokens_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(query): Query<ApiTokenListQuery>,
) -> Result<Json<Vec<ApiTokenView>>, AppError> {
    let items = list_api_tokens(&state.db, &session, query.project_slug.as_deref()).await?;
    Ok(Json(items))
}

async fn create_api_token_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateApiTokenInput>,
) -> Result<(StatusCode, Json<CreatedApiTokenResponse>), AppError> {
    let created = create_api_token(&state.db, &session, input).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn revoke_api_token_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(token_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    revoke_api_token(&state.db, &session, token_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn generate_api_token() -> (String, String, String) {
    let secret_hex = generate_random_token_hex();
    let raw = format!("{API_TOKEN_PREFIX}{secret_hex}");
    let prefix = format!("{API_TOKEN_PREFIX}{}", &secret_hex[..8]);
    let hash = sha256_hex(&raw);
    (raw, prefix, hash)
}

pub fn hash_api_token(raw_token: &str) -> Result<String, AppError> {
    let trimmed = raw_token.trim();
    let Some(hex_part) = trimmed.strip_prefix(API_TOKEN_PREFIX) else {
        return Err(AppError::Unauthorized("invalid api token format"));
    };
    if hex_part.len() != 64 || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::Unauthorized("invalid api token format"));
    }
    Ok(sha256_hex(trimmed))
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn list_api_tokens(
    pool: &PgPool,
    session: &AuthSession,
    project_slug: Option<&str>,
) -> Result<Vec<ApiTokenView>, AppError> {
    session.require_user_session()?;

    if let Some(slug) = project_slug
        && !is_valid_slug(slug)
    {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let tenant_id = session.tenant_ctx.tenant_id();

    let rows: Vec<ApiTokenDbRow> = sqlx::query_as(
        "SELECT t.id, t.tenant_id, p.workspace_id, w.slug AS workspace_slug,
                t.project_id, p.slug AS project_slug,
                t.created_by_user_id, u.email AS created_by_email,
                t.name, t.token_prefix, t.can_read, t.can_write,
                t.expires_at, t.revoked_at, t.last_used_at, t.created_at
         FROM api_tokens t
         JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         JOIN users u ON u.id = t.created_by_user_id
         WHERE t.tenant_id = $1
           AND ($2::TEXT IS NULL OR p.slug = $2)
         ORDER BY t.created_at DESC, t.id DESC
         LIMIT 200",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list api_tokens: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_api_tokens: {err}")))?;

    // Only return tokens belonging to workspaces the caller can access.
    Ok(rows
        .into_iter()
        .filter(|r| session.can_access_workspace(r.workspace_id))
        .map(ApiTokenView::from)
        .collect())
}

pub async fn create_api_token(
    pool: &PgPool,
    session: &AuthSession,
    input: CreateApiTokenInput,
) -> Result<CreatedApiTokenResponse, AppError> {
    session.require_user_session()?;

    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation(
            "token name must be between 1 and 128 characters".to_owned(),
        ));
    }

    let project_slug = input.project_slug.trim();
    if !is_valid_slug(project_slug) {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let ws_slug = input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(slug) = ws_slug
        && !is_valid_slug(slug)
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let expires_at = match input.expires_in_days {
        None => None,
        Some(days) => {
            if !(1..=365).contains(&days) {
                return Err(AppError::Validation(
                    "expires_in_days must be between 1 and 365".to_owned(),
                ));
            }
            Some(Utc::now() + Duration::days(i64::from(days)))
        }
    };

    let can_write = input.can_write.unwrap_or(false);
    let tenant_id = session.tenant_ctx.tenant_id();

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;

    let project_row: Option<(i64, i64, String, String)> = match ws_slug {
        Some(slug) => sqlx::query_as(
            "SELECT p.id, p.workspace_id, w.slug, p.slug
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
        )
        .bind(tenant_id)
        .bind(slug)
        .bind(project_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for api token: {err}")))?,
        None => sqlx::query_as(
            "SELECT p.id, p.workspace_id, w.slug, p.slug
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             WHERE p.tenant_id = $1 AND p.slug = $2
             ORDER BY w.is_default DESC, p.id ASC
             LIMIT 1",
        )
        .bind(tenant_id)
        .bind(project_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for api token: {err}")))?,
    };

    let Some((project_id, workspace_id, workspace_slug, resolved_project_slug)) = project_row
    else {
        return Err(AppError::NotFound);
    };

    // Strict least-privilege: only workspace/tenant admins may mint project API tokens.
    if !session.can_manage_workspace(workspace_id) {
        return Err(AppError::Forbidden(
            "insufficient permissions to create project api tokens",
        ));
    }

    let (raw_token, token_prefix, token_hash) = generate_api_token();

    let inserted: (i64, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO api_tokens (
             tenant_id, project_id, created_by_user_id,
             name, token_prefix, token_hash, can_read, can_write, expires_at
         ) VALUES ($1, $2, $3, $4, $5, $6, TRUE, $7, $8)
         RETURNING id, created_at",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(session.user.id)
    .bind(name)
    .bind(&token_prefix)
    .bind(&token_hash)
    .bind(can_write)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert api_token: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create_api_token: {err}")))?;

    info!(
        tenant_id,
        project_id,
        token_id = inserted.0,
        can_write,
        actor_user_id = session.user.id,
        "project api token created"
    );

    Ok(CreatedApiTokenResponse {
        token: ApiTokenView {
            id: inserted.0,
            tenant_id,
            workspace_id,
            workspace_slug,
            project_id,
            project_slug: resolved_project_slug,
            created_by_user_id: session.user.id,
            created_by_email: session.user.email.clone(),
            name: name.to_owned(),
            token_prefix,
            can_read: true,
            can_write,
            expires_at,
            revoked_at: None,
            last_used_at: None,
            created_at: inserted.1,
        },
        raw_token,
    })
}

pub async fn revoke_api_token(
    pool: &PgPool,
    session: &AuthSession,
    token_id: i64,
) -> Result<(), AppError> {
    session.require_user_session()?;
    if token_id <= 0 {
        return Err(AppError::Validation("invalid token_id".to_owned()));
    }

    let tenant_id = session.tenant_ctx.tenant_id();
    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;

    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT t.project_id, p.workspace_id
         FROM api_tokens t
         JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
         WHERE t.tenant_id = $1 AND t.id = $2",
    )
    .bind(tenant_id)
    .bind(token_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup api_token for revoke: {err}")))?;

    let Some((_project_id, workspace_id)) = row else {
        return Err(AppError::NotFound);
    };

    if !session.can_manage_workspace(workspace_id) {
        return Err(AppError::Forbidden(
            "insufficient permissions to revoke api token",
        ));
    }

    sqlx::query(
        "UPDATE api_tokens
         SET revoked_at = COALESCE(revoked_at, NOW())
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(token_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("revoke api_token: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit revoke_api_token: {err}")))?;

    info!(
        tenant_id,
        token_id,
        actor_user_id = session.user.id,
        "project api token revoked"
    );

    Ok(())
}

/// Authenticate a per-project `Authorization: Bearer mlapi_...` token into a project-scoped `AuthSession`.
pub async fn authenticate_api_token(
    pool: &PgPool,
    raw_token: &str,
) -> Result<AuthSession, AppError> {
    let token_hash = hash_api_token(raw_token)?;

    // Step 1: locate tenant_id + token_id via worker transaction.
    let mut worker_tx = begin_worker_tx(pool).await?;
    let locator: Option<(i64, i64, i64)> = sqlx::query_as(
        "SELECT tenant_id, project_id, id
         FROM api_tokens
         WHERE token_hash = $1
           AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at > NOW())",
    )
    .bind(&token_hash)
    .fetch_optional(&mut *worker_tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("locate api_token: {err}")))?;
    worker_tx
        .commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit api_token locator tx: {err}")))?;

    let Some((tenant_id, project_id, token_id)) = locator else {
        return Err(AppError::Unauthorized("invalid or revoked api token"));
    };

    // Step 2: verify full token + project + workspace + active creator under RLS.
    let tenant_ctx = TenantContext::new(tenant_id)?;
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;

    #[derive(FromRow)]
    struct TokenAuthRow {
        id: i64,
        name: String,
        project_id: i64,
        project_slug: String,
        workspace_id: i64,
        workspace_slug: String,
        workspace_name: String,
        ws_retention_override: Option<i16>,
        ws_rate_override: Option<i32>,
        ws_is_default: bool,
        can_read: bool,
        can_write: bool,
        user_id: i64,
        user_email: String,
        user_name: String,
    }

    let row: Option<TokenAuthRow> = sqlx::query_as(
        "SELECT t.id, t.name, t.project_id, p.slug AS project_slug,
                p.workspace_id, w.slug AS workspace_slug, w.name AS workspace_name,
                w.retention_days_override AS ws_retention_override,
                w.default_rate_limit_per_minute_override AS ws_rate_override,
                w.is_default AS ws_is_default,
                t.can_read, t.can_write,
                u.id AS user_id, u.email AS user_email, u.name AS user_name
         FROM api_tokens t
         JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         JOIN users u ON u.id = t.created_by_user_id
         WHERE t.tenant_id = $1
           AND t.project_id = $2
           AND t.id = $3
           AND t.token_hash = $4
           AND t.revoked_at IS NULL
           AND (t.expires_at IS NULL OR t.expires_at > NOW())
           AND u.is_active = TRUE",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(token_id)
    .bind(&token_hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("verify api_token under rls: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::Unauthorized("invalid or revoked api token"));
    };

    let tenant = load_tenant_summary(&mut tx, tenant_id).await?;

    sqlx::query(
        "UPDATE api_tokens SET last_used_at = NOW() WHERE tenant_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(token_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("touch api_token last_used_at: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit api_token auth tx: {err}")))?;

    let retention_days_override = match row.ws_retention_override {
        Some(v) => Some(
            u16::try_from(v)
                .map_err(|_| AppError::Validation("invalid workspace retention".to_owned()))?,
        ),
        None => None,
    };

    let effective_role = if row.can_write {
        Role::Member
    } else {
        Role::Viewer
    };

    let mut workspace_roles = BTreeMap::new();
    workspace_roles.insert(row.workspace_id, effective_role);

    Ok(AuthSession {
        session_id: 0,
        tenant_ctx,
        tenant,
        default_workspace: WorkspaceSummary {
            id: row.workspace_id,
            slug: row.workspace_slug.clone(),
            name: row.workspace_name,
            retention_days_override,
            default_rate_limit_per_minute_override: row.ws_rate_override,
            is_default: row.ws_is_default,
        },
        user: AuthenticatedUser {
            id: row.user_id,
            email: row.user_email,
            name: row.user_name,
        },
        role: effective_role,
        tenant_role: None,
        workspace_roles,
        api_token: Some(ApiTokenAuthScope {
            token_id: row.id,
            token_name: row.name,
            project_id: row.project_id,
            project_slug: row.project_slug,
            workspace_id: row.workspace_id,
            workspace_slug: row.workspace_slug,
            can_read: row.can_read,
            can_write: row.can_write,
        }),
        csrf_token: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_validates_prefixed_api_tokens() {
        let (raw, prefix, hash) = generate_api_token();
        assert!(raw.starts_with(API_TOKEN_PREFIX));
        assert_eq!(raw.len(), API_TOKEN_PREFIX.len() + 64);
        assert!(prefix.starts_with(API_TOKEN_PREFIX));
        assert_eq!(hash.len(), 64);
        assert_eq!(hash_api_token(&raw).unwrap(), hash);
        assert!(
            hash_api_token(
                "mlmcp_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            )
            .is_err()
        );
        assert!(hash_api_token("mlapi_short").is_err());
    }
}
