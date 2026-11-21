use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub::scrub_event_payload;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx, begin_worker_tx};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use crate::http::AuthState;
use crate::model::{AuthSession, Role};
use crate::token::generate_random_token_hex;

pub const MCP_TOKEN_PREFIX: &str = "mlmcp_";
const MAX_AUDIT_ARGS_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAccessScope {
    Tenant,
    Workspace {
        workspace_id: i64,
        workspace_slug: String,
    },
    Project {
        workspace_id: i64,
        workspace_slug: String,
        project_id: i64,
        project_slug: String,
    },
}

impl McpAccessScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tenant => "tenant",
            Self::Workspace { .. } => "workspace",
            Self::Project { .. } => "project",
        }
    }

    pub fn workspace_slug(&self) -> Option<&str> {
        match self {
            Self::Tenant => None,
            Self::Workspace { workspace_slug, .. } | Self::Project { workspace_slug, .. } => {
                Some(workspace_slug.as_str())
            }
        }
    }

    pub fn project_slug(&self) -> Option<&str> {
        match self {
            Self::Project { project_slug, .. } => Some(project_slug.as_str()),
            Self::Tenant | Self::Workspace { .. } => None,
        }
    }

    pub fn project_id(&self) -> Option<i64> {
        match self {
            Self::Project { project_id, .. } => Some(*project_id),
            Self::Tenant | Self::Workspace { .. } => None,
        }
    }

    pub fn workspace_id(&self) -> Option<i64> {
        match self {
            Self::Workspace { workspace_id, .. } | Self::Project { workspace_id, .. } => {
                Some(*workspace_id)
            }
            Self::Tenant => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct McpAuthContext {
    pub tenant_ctx: TenantContext,
    pub tenant_slug: String,
    pub user_id: i64,
    pub user_email: String,
    pub role: Role,
    pub token_id: i64,
    pub token_name: String,
    pub access_scope: McpAccessScope,
    pub can_read: bool,
    pub can_triage: bool,
    pub rate_limit_per_minute: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTokenView {
    pub id: i64,
    pub tenant_id: i64,
    pub user_id: i64,
    pub user_email: String,
    pub name: String,
    pub token_prefix: String,
    pub access_scope: String,
    pub workspace_id: Option<i64>,
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub can_read: bool,
    pub can_triage: bool,
    pub rate_limit_per_minute: i32,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedMcpTokenResponse {
    pub token: McpTokenView,
    /// Returned ONLY once at creation time; raw tokens are never stored or retrievable again.
    pub raw_token: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateMcpTokenInput {
    pub name: String,
    /// `"tenant"` (default), `"workspace"`, or `"project"`.
    pub access_scope: Option<String>,
    pub workspace_slug: Option<String>,
    pub project_slug: Option<String>,
    /// Defaults to `false` (read-only by default per F-MCP-2).
    pub can_triage: Option<bool>,
    /// Defaults to `60` requests/minute (allowed range `1..=6000`).
    pub rate_limit_per_minute: Option<i32>,
    /// Optional token lifetime in days (`1..=365`). `None` means no expiry until revoked.
    pub expires_in_days: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpAuditLogView {
    pub id: i64,
    pub tenant_id: i64,
    pub token_id: Option<i64>,
    pub user_id: i64,
    pub user_email: String,
    pub token_name: String,
    pub tool_name: String,
    pub arguments_json: Value,
    pub status: String,
    pub error_code: Option<String>,
    pub duration_ms: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpAuditStatus {
    Ok,
    Error,
    Denied,
    RateLimited,
}

impl McpAuditStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Denied => "denied",
            Self::RateLimited => "rate_limited",
        }
    }
}

/// Service facade owned by `maxi-loop-auth` so `maxi-loop-mcp` never imports `sqlx` directly (`D006`).
#[derive(Clone)]
pub struct McpAuthService {
    db: PgPool,
}

impl McpAuthService {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn authenticate_bearer(&self, raw_token: &str) -> Result<McpAuthContext, AppError> {
        authenticate_mcp_bearer_token(&self.db, raw_token).await
    }

    pub async fn record_audit(
        &self,
        auth: &McpAuthContext,
        tool_name: &str,
        arguments: &Value,
        status: McpAuditStatus,
        error_code: Option<&str>,
        duration_ms: u32,
    ) -> Result<(), AppError> {
        record_mcp_audit_log(
            &self.db,
            auth,
            tool_name,
            arguments,
            status,
            error_code,
            duration_ms,
        )
        .await
    }
}

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/mcp/tokens",
            get(list_tokens_handler).post(create_token_handler),
        )
        .route("/mcp/tokens/{token_id}", delete(revoke_token_handler))
        .route("/mcp/audit-logs", get(list_audit_logs_handler))
}

#[derive(Deserialize, Default)]
struct AuditQuery {
    limit: Option<u32>,
}

async fn list_tokens_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<McpTokenView>>, AppError> {
    session.require_user_session()?;
    let tokens = list_mcp_tokens(&state.db, &session).await?;
    Ok(Json(tokens))
}

async fn create_token_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateMcpTokenInput>,
) -> Result<(StatusCode, Json<CreatedMcpTokenResponse>), AppError> {
    session.require_user_session()?;
    let created = create_mcp_token(&state.db, &session, input).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn revoke_token_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(token_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    session.require_user_session()?;
    revoke_mcp_token(&state.db, &session, token_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_audit_logs_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<McpAuditLogView>>, AppError> {
    session.require_user_session()?;
    let logs = list_mcp_audit_logs(&state.db, &session, query.limit).await?;
    Ok(Json(logs))
}

#[derive(FromRow)]
struct McpTokenRow {
    id: i64,
    tenant_id: i64,
    user_id: i64,
    user_email: String,
    name: String,
    token_prefix: String,
    access_scope: String,
    workspace_id: Option<i64>,
    workspace_slug: Option<String>,
    project_id: Option<i64>,
    project_slug: Option<String>,
    can_read: bool,
    can_triage: bool,
    rate_limit_per_minute: i32,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl From<McpTokenRow> for McpTokenView {
    fn from(r: McpTokenRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            user_id: r.user_id,
            user_email: r.user_email,
            name: r.name,
            token_prefix: r.token_prefix,
            access_scope: r.access_scope,
            workspace_id: r.workspace_id,
            workspace_slug: r.workspace_slug,
            project_id: r.project_id,
            project_slug: r.project_slug,
            can_read: r.can_read,
            can_triage: r.can_triage,
            rate_limit_per_minute: r.rate_limit_per_minute,
            expires_at: r.expires_at,
            revoked_at: r.revoked_at,
            last_used_at: r.last_used_at,
            created_at: r.created_at,
        }
    }
}

#[derive(FromRow)]
struct McpAuditRow {
    id: i64,
    tenant_id: i64,
    token_id: Option<i64>,
    user_id: i64,
    user_email: String,
    token_name: String,
    tool_name: String,
    arguments_json: Value,
    status: String,
    error_code: Option<String>,
    duration_ms: i32,
    created_at: DateTime<Utc>,
}

impl From<McpAuditRow> for McpAuditLogView {
    fn from(r: McpAuditRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            token_id: r.token_id,
            user_id: r.user_id,
            user_email: r.user_email,
            token_name: r.token_name,
            tool_name: r.tool_name,
            arguments_json: r.arguments_json,
            status: r.status,
            error_code: r.error_code,
            duration_ms: r.duration_ms,
            created_at: r.created_at,
        }
    }
}

pub fn hash_mcp_token(raw_token: &str) -> Result<String, AppError> {
    let trimmed = raw_token.trim();
    let Some(hex_body) = trimmed.strip_prefix(MCP_TOKEN_PREFIX) else {
        return Err(AppError::Unauthorized("invalid mcp token"));
    };
    if hex_body.len() != 64 || !hex_body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::Unauthorized("invalid mcp token"));
    }
    let digest = Sha256::digest(trimmed.as_bytes());
    Ok(hex::encode(digest))
}

pub fn generate_mcp_token() -> (String, String, String) {
    let random_hex = generate_random_token_hex();
    let raw_token = format!("{MCP_TOKEN_PREFIX}{random_hex}");
    let prefix = format!("{MCP_TOKEN_PREFIX}{}", &random_hex[..8]);
    let token_hash = hex::encode(Sha256::digest(raw_token.as_bytes()));
    (raw_token, prefix, token_hash)
}

pub async fn create_mcp_token(
    pool: &PgPool,
    session: &AuthSession,
    input: CreateMcpTokenInput,
) -> Result<CreatedMcpTokenResponse, AppError> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation(
            "token name must be between 1 and 128 characters".to_owned(),
        ));
    }

    let can_triage = input.can_triage.unwrap_or(false);
    // Prevent privilege escalation: a read-only viewer cannot mint a triage-capable MCP token.
    if can_triage && !session.role.can_triage_issues() {
        return Err(AppError::Forbidden(
            "viewer role cannot create triage-scoped mcp tokens",
        ));
    }

    let rate_limit = input.rate_limit_per_minute.unwrap_or(60);
    if !(1..=6000).contains(&rate_limit) {
        return Err(AppError::Validation(
            "rate_limit_per_minute must be between 1 and 6000".to_owned(),
        ));
    }

    let expires_at = match input.expires_in_days {
        None => None,
        Some(days) if (1..=365).contains(&days) => {
            Some(Utc::now() + Duration::days(i64::from(days)))
        }
        Some(_) => {
            return Err(AppError::Validation(
                "expires_in_days must be between 1 and 365".to_owned(),
            ));
        }
    };

    let scope_raw = input
        .access_scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("tenant")
        .to_ascii_lowercase();

    let ws_slug_opt = input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let proj_slug_opt = input
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(ws) = ws_slug_opt
        && !is_valid_slug(ws)
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }
    if let Some(p) = proj_slug_opt
        && !is_valid_slug(p)
    {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let tenant_id = session.tenant_ctx.tenant_id();

    let (access_scope, workspace_id, workspace_slug, project_id, project_slug) = match scope_raw
        .as_str()
    {
        "tenant" => {
            if ws_slug_opt.is_some() || proj_slug_opt.is_some() {
                return Err(AppError::Validation(
                    "tenant-scoped token must not specify workspace_slug or project_slug"
                        .to_owned(),
                ));
            }
            ("tenant", None, None, None, None)
        }
        "workspace" => {
            if proj_slug_opt.is_some() {
                return Err(AppError::Validation(
                    "workspace-scoped token must not specify project_slug".to_owned(),
                ));
            }
            let ws_slug = ws_slug_opt.unwrap_or(session.default_workspace.slug.as_str());
            let ws_id: Option<i64> =
                sqlx::query_scalar("SELECT id FROM workspaces WHERE tenant_id = $1 AND slug = $2")
                    .bind(tenant_id)
                    .bind(ws_slug)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|err| AppError::DatabaseQuery(format!("lookup workspace: {err}")))?;
            let Some(ws_id) = ws_id else {
                return Err(AppError::NotFound);
            };
            (
                "workspace",
                Some(ws_id),
                Some(ws_slug.to_owned()),
                None,
                None,
            )
        }
        "project" => {
            let Some(p_slug) = proj_slug_opt else {
                return Err(AppError::Validation(
                    "project-scoped token requires project_slug".to_owned(),
                ));
            };
            let row: Option<(i64, String, i64, String)> = match ws_slug_opt {
                Some(ws) => sqlx::query_as(
                    "SELECT w.id, w.slug, p.id, p.slug
                         FROM projects p
                         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                         WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
                )
                .bind(tenant_id)
                .bind(ws)
                .bind(p_slug)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("lookup project: {err}")))?,
                None => sqlx::query_as(
                    "SELECT w.id, w.slug, p.id, p.slug
                         FROM projects p
                         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                         WHERE p.tenant_id = $1 AND p.slug = $2
                         ORDER BY w.is_default DESC, p.id ASC
                         LIMIT 1",
                )
                .bind(tenant_id)
                .bind(p_slug)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("lookup project: {err}")))?,
            };
            let Some((ws_id, ws_s, p_id, p_s)) = row else {
                return Err(AppError::NotFound);
            };
            ("project", Some(ws_id), Some(ws_s), Some(p_id), Some(p_s))
        }
        _ => {
            return Err(AppError::Validation(
                "access_scope must be 'tenant', 'workspace', or 'project'".to_owned(),
            ));
        }
    };

    let (raw_token, token_prefix, token_hash) = generate_mcp_token();
    let inserted: (i64, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO mcp_tokens (
             tenant_id, user_id, name, token_prefix, token_hash,
             access_scope, workspace_id, project_id,
             can_read, can_triage, rate_limit_per_minute, expires_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE, $9, $10, $11)
         RETURNING id, created_at",
    )
    .bind(tenant_id)
    .bind(session.user.id)
    .bind(name)
    .bind(&token_prefix)
    .bind(&token_hash)
    .bind(access_scope)
    .bind(workspace_id)
    .bind(project_id)
    .bind(can_triage)
    .bind(rate_limit)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert mcp_token: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create_mcp_token: {err}")))?;

    Ok(CreatedMcpTokenResponse {
        token: McpTokenView {
            id: inserted.0,
            tenant_id,
            user_id: session.user.id,
            user_email: session.user.email.clone(),
            name: name.to_owned(),
            token_prefix,
            access_scope: access_scope.to_owned(),
            workspace_id,
            workspace_slug,
            project_id,
            project_slug,
            can_read: true,
            can_triage,
            rate_limit_per_minute: rate_limit,
            expires_at,
            revoked_at: None,
            last_used_at: None,
            created_at: inserted.1,
        },
        raw_token,
    })
}

pub async fn list_mcp_tokens(
    pool: &PgPool,
    session: &AuthSession,
) -> Result<Vec<McpTokenView>, AppError> {
    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let tenant_id = session.tenant_ctx.tenant_id();

    let rows: Vec<McpTokenRow> = if session.role.can_manage_projects() {
        sqlx::query_as(
            "SELECT t.id, t.tenant_id, t.user_id, u.email AS user_email,
                    t.name, t.token_prefix, t.access_scope,
                    t.workspace_id, w.slug AS workspace_slug,
                    t.project_id, p.slug AS project_slug,
                    t.can_read, t.can_triage, t.rate_limit_per_minute,
                    t.expires_at, t.revoked_at, t.last_used_at, t.created_at
             FROM mcp_tokens t
             JOIN users u ON u.id = t.user_id
             LEFT JOIN workspaces w ON w.tenant_id = t.tenant_id AND w.id = t.workspace_id
             LEFT JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
             WHERE t.tenant_id = $1
             ORDER BY t.created_at DESC, t.id DESC
             LIMIT 200",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list mcp_tokens: {err}")))?
    } else {
        sqlx::query_as(
            "SELECT t.id, t.tenant_id, t.user_id, u.email AS user_email,
                    t.name, t.token_prefix, t.access_scope,
                    t.workspace_id, w.slug AS workspace_slug,
                    t.project_id, p.slug AS project_slug,
                    t.can_read, t.can_triage, t.rate_limit_per_minute,
                    t.expires_at, t.revoked_at, t.last_used_at, t.created_at
             FROM mcp_tokens t
             JOIN users u ON u.id = t.user_id
             LEFT JOIN workspaces w ON w.tenant_id = t.tenant_id AND w.id = t.workspace_id
             LEFT JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
             WHERE t.tenant_id = $1 AND t.user_id = $2
             ORDER BY t.created_at DESC, t.id DESC
             LIMIT 200",
        )
        .bind(tenant_id)
        .bind(session.user.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list user mcp_tokens: {err}")))?
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_mcp_tokens: {err}")))?;

    Ok(rows.into_iter().map(McpTokenView::from).collect())
}

pub async fn revoke_mcp_token(
    pool: &PgPool,
    session: &AuthSession,
    token_id: i64,
) -> Result<(), AppError> {
    if token_id <= 0 {
        return Err(AppError::Validation("invalid token_id".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let tenant_id = session.tenant_ctx.tenant_id();

    let res = if session.role.can_manage_projects() {
        sqlx::query(
            "UPDATE mcp_tokens
             SET revoked_at = COALESCE(revoked_at, NOW())
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(token_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("revoke mcp_token: {err}")))?
    } else {
        sqlx::query(
            "UPDATE mcp_tokens
             SET revoked_at = COALESCE(revoked_at, NOW())
             WHERE tenant_id = $1 AND id = $2 AND user_id = $3",
        )
        .bind(tenant_id)
        .bind(token_id)
        .bind(session.user.id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("revoke user mcp_token: {err}")))?
    };

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit revoke_mcp_token: {err}")))?;

    Ok(())
}

pub async fn authenticate_mcp_bearer_token(
    pool: &PgPool,
    raw_token: &str,
) -> Result<McpAuthContext, AppError> {
    let token_hash = hash_mcp_token(raw_token)?;

    // Step 1: Locate (tenant_id, token_id) via worker role using SHA-256 hash only.
    let mut worker_tx = begin_worker_tx(pool).await?;
    let locator: Option<(i64, i64)> = sqlx::query_as(
        "SELECT tenant_id, id
         FROM mcp_tokens
         WHERE token_hash = $1
           AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at > NOW())",
    )
    .bind(&token_hash)
    .fetch_optional(&mut *worker_tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("locate mcp_token: {err}")))?;
    worker_tx
        .commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit mcp locator: {err}")))?;

    let Some((tenant_id, token_id)) = locator else {
        return Err(AppError::Unauthorized("invalid or revoked mcp token"));
    };

    // Step 2: Verify full token, active user, tenant, membership, and scope inside RLS tenant tx.
    let tenant_ctx = TenantContext::new(tenant_id)?;
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;

    #[derive(FromRow)]
    struct ResolvedTokenRow {
        id: i64,
        user_id: i64,
        user_email: String,
        tenant_slug: String,
        name: String,
        access_scope: String,
        workspace_id: Option<i64>,
        workspace_slug: Option<String>,
        project_id: Option<i64>,
        project_slug: Option<String>,
        can_read: bool,
        can_triage: bool,
        rate_limit_per_minute: i32,
    }

    let row: Option<ResolvedTokenRow> = sqlx::query_as(
        "SELECT t.id, t.user_id, u.email AS user_email, tn.slug AS tenant_slug,
                t.name, t.access_scope,
                t.workspace_id, w.slug AS workspace_slug,
                t.project_id, p.slug AS project_slug,
                t.can_read, t.can_triage, t.rate_limit_per_minute
         FROM mcp_tokens t
         JOIN users u ON u.id = t.user_id
         JOIN tenants tn ON tn.id = t.tenant_id
         LEFT JOIN workspaces w ON w.tenant_id = t.tenant_id AND w.id = t.workspace_id
         LEFT JOIN projects p ON p.tenant_id = t.tenant_id AND p.id = t.project_id
         WHERE t.tenant_id = $1
           AND t.id = $2
           AND t.token_hash = $3
           AND t.revoked_at IS NULL
           AND (t.expires_at IS NULL OR t.expires_at > NOW())
           AND u.is_active = TRUE",
    )
    .bind(tenant_id)
    .bind(token_id)
    .bind(&token_hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load mcp_token: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::Unauthorized("invalid or revoked mcp token"));
    };

    // Verify user still holds an active membership in this tenant.
    let roles: Vec<String> =
        sqlx::query_scalar("SELECT role FROM memberships WHERE tenant_id = $1 AND user_id = $2")
            .bind(tenant_id)
            .bind(row.user_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("load mcp token memberships: {err}")))?;

    let mut best_role: Option<Role> = None;
    for r in roles {
        let parsed = Role::parse(&r)?;
        if best_role.is_none_or(|cur| parsed.privilege_rank() > cur.privilege_rank()) {
            best_role = Some(parsed);
        }
    }
    let Some(role) = best_role else {
        return Err(AppError::Unauthorized("no active tenant membership"));
    };

    let access_scope = match row.access_scope.as_str() {
        "tenant" => McpAccessScope::Tenant,
        "workspace" => {
            let (Some(ws_id), Some(ws_slug)) = (row.workspace_id, row.workspace_slug) else {
                return Err(AppError::Unauthorized("mcp token workspace scope missing"));
            };
            McpAccessScope::Workspace {
                workspace_id: ws_id,
                workspace_slug: ws_slug,
            }
        }
        "project" => {
            let (Some(ws_id), Some(ws_slug), Some(p_id), Some(p_slug)) = (
                row.workspace_id,
                row.workspace_slug,
                row.project_id,
                row.project_slug,
            ) else {
                return Err(AppError::Unauthorized("mcp token project scope missing"));
            };
            McpAccessScope::Project {
                workspace_id: ws_id,
                workspace_slug: ws_slug,
                project_id: p_id,
                project_slug: p_slug,
            }
        }
        _ => return Err(AppError::Unauthorized("invalid mcp token scope")),
    };

    // Effective triage permission requires BOTH token `can_triage` AND user role capability.
    let effective_can_triage = row.can_triage && role.can_triage_issues();

    // Update last_used_at best-effort within the same transaction.
    sqlx::query("UPDATE mcp_tokens SET last_used_at = NOW() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(row.id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("touch mcp_token last_used_at: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit mcp auth tx: {err}")))?;

    Ok(McpAuthContext {
        tenant_ctx,
        tenant_slug: row.tenant_slug,
        user_id: row.user_id,
        user_email: row.user_email,
        role,
        token_id: row.id,
        token_name: row.name,
        access_scope,
        can_read: row.can_read,
        can_triage: effective_can_triage,
        rate_limit_per_minute: u32::try_from(row.rate_limit_per_minute.max(1)).unwrap_or(60),
    })
}

pub async fn record_mcp_audit_log(
    pool: &PgPool,
    auth: &McpAuthContext,
    tool_name: &str,
    arguments: &Value,
    status: McpAuditStatus,
    error_code: Option<&str>,
    duration_ms: u32,
) -> Result<(), AppError> {
    let mut sanitized_args = arguments.clone();
    scrub_event_payload(&mut sanitized_args);

    // Cap serialized arguments size so huge inputs cannot bloat audit logs.
    let serialized = serde_json::to_string(&sanitized_args).unwrap_or_else(|_| "{}".to_owned());
    let bounded_args = if serialized.len() > MAX_AUDIT_ARGS_BYTES {
        serde_json::json!({
            "_truncated": true,
            "bytes": serialized.len()
        })
    } else {
        sanitized_args
    };

    let clean_tool = tool_name.trim();
    let bounded_tool = if clean_tool.is_empty() {
        "unknown"
    } else if clean_tool.len() > 128 {
        &clean_tool[..128]
    } else {
        clean_tool
    };

    let bounded_err = error_code.map(|e| if e.len() > 128 { &e[..128] } else { e });

    let duration_i32 = i32::try_from(duration_ms).unwrap_or(i32::MAX);

    let mut tx = begin_tenant_tx(pool, auth.tenant_ctx).await?;
    sqlx::query(
        "INSERT INTO mcp_audit_logs (
             tenant_id, token_id, user_id, user_email, token_name,
             tool_name, arguments_json, status, error_code, duration_ms
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(auth.tenant_ctx.tenant_id())
    .bind(auth.token_id)
    .bind(auth.user_id)
    .bind(&auth.user_email)
    .bind(&auth.token_name)
    .bind(bounded_tool)
    .bind(bounded_args)
    .bind(status.as_str())
    .bind(bounded_err)
    .bind(duration_i32)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert mcp_audit_log: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit mcp_audit_log: {err}")))?;

    Ok(())
}

pub async fn list_mcp_audit_logs(
    pool: &PgPool,
    session: &AuthSession,
    limit: Option<u32>,
) -> Result<Vec<McpAuditLogView>, AppError> {
    let max_rows = i64::from(limit.unwrap_or(100).clamp(1, 500));
    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let tenant_id = session.tenant_ctx.tenant_id();

    let rows: Vec<McpAuditRow> = if session.role.can_manage_projects() {
        sqlx::query_as(
            "SELECT id, tenant_id, token_id, user_id, user_email, token_name,
                    tool_name, arguments_json, status, error_code, duration_ms, created_at
             FROM mcp_audit_logs
             WHERE tenant_id = $1
             ORDER BY created_at DESC, id DESC
             LIMIT $2",
        )
        .bind(tenant_id)
        .bind(max_rows)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list mcp_audit_logs: {err}")))?
    } else {
        sqlx::query_as(
            "SELECT id, tenant_id, token_id, user_id, user_email, token_name,
                    tool_name, arguments_json, status, error_code, duration_ms, created_at
             FROM mcp_audit_logs
             WHERE tenant_id = $1 AND user_id = $2
             ORDER BY created_at DESC, id DESC
             LIMIT $3",
        )
        .bind(tenant_id)
        .bind(session.user.id)
        .bind(max_rows)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list user mcp_audit_logs: {err}")))?
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_mcp_audit_logs: {err}")))?;

    Ok(rows.into_iter().map(McpAuditLogView::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_validates_prefixed_mcp_tokens() {
        let (raw, prefix, hash) = generate_mcp_token();
        assert!(raw.starts_with(MCP_TOKEN_PREFIX));
        assert_eq!(raw.len(), MCP_TOKEN_PREFIX.len() + 64);
        assert!(prefix.starts_with(MCP_TOKEN_PREFIX));
        assert_eq!(hash.len(), 64);
        assert_eq!(hash_mcp_token(&raw).unwrap(), hash);
        assert!(hash_mcp_token("invalid_prefix_token").is_err());
        assert!(hash_mcp_token("mlmcp_short").is_err());
    }
}
