use std::collections::BTreeMap;
use std::time::Instant;

use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use maxi_loop_platform::bootstrap::{hash_password_argon2id, verify_password_argon2id};
use maxi_loop_platform::config::{Config, is_valid_email, is_valid_slug};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx, begin_worker_tx};
use sqlx::PgPool;
use tracing::{info, warn};

use crate::model::{
    AuthSession, AuthenticatedUser, ChangePasswordRequest, LoginRequest, Role, TenantSummary,
    WorkspaceSummary,
};
use crate::rate_limit::LoginRateLimiter;
use crate::token::{
    SESSION_COOKIE_NAME, extract_cookie_value, generate_random_token_hex, hash_session_token,
};

// Precomputed valid Argon2id PHC hash used to equalize verification timing when an email does not exist.
const DUMMY_ARGON2ID_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZTEyMzQ1Ng$9vW8X5k3Z7M2L1P4Q6R8T0Y2U4I6O8P0A2S4D6F8G0H";

pub async fn login(
    pool: &PgPool,
    config: &Config,
    limiter: &LoginRateLimiter,
    req: LoginRequest,
) -> Result<(AuthSession, String), AppError> {
    let email = req.email.trim().to_ascii_lowercase();
    if !is_valid_email(&email) {
        return Err(AppError::Validation("invalid email format".to_owned()));
    }
    if req.password.is_empty() || req.password.len() > 256 {
        return Err(AppError::Validation("invalid password length".to_owned()));
    }

    let requested_tenant_slug = match req
        .tenant_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(slug) => {
            if !is_valid_slug(slug) {
                return Err(AppError::Validation("invalid tenant_slug".to_owned()));
            }
            Some(slug.to_owned())
        }
        None => None,
    };

    if let Err(retry_after_secs) = limiter.check_and_record(&email, Instant::now()) {
        warn!("login rate limit exceeded");
        return Err(AppError::RateLimited { retry_after_secs });
    }

    // Resolve user credentials and candidate tenant membership inside worker transaction.
    let mut worker_tx = begin_worker_tx(pool).await?;

    let user_row: Option<(i64, String, String, String, bool)> = sqlx::query_as(
        "SELECT id, email, name, password_hash, is_active
         FROM users
         WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(&mut *worker_tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup login user: {err}")))?;

    let Some((user_id, user_email, user_name, password_hash, is_active)) = user_row else {
        let _ = verify_password_argon2id(&req.password, DUMMY_ARGON2ID_HASH);
        warn!("login failed: invalid credentials");
        return Err(AppError::Unauthorized("invalid credentials"));
    };

    let password_ok = verify_password_argon2id(&req.password, &password_hash)?;
    if !password_ok || !is_active {
        warn!(user_id, "login failed: invalid credentials");
        return Err(AppError::Unauthorized("invalid credentials"));
    }

    let resolved_tenant_id: Option<i64> = match &requested_tenant_slug {
        Some(slug) => sqlx::query_scalar(
            "SELECT t.id
             FROM tenants t
             JOIN memberships m ON m.tenant_id = t.id
             WHERE t.slug = $1 AND m.user_id = $2
             LIMIT 1",
        )
        .bind(slug)
        .bind(user_id)
        .fetch_optional(&mut *worker_tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("resolve login tenant by slug: {err}")))?,
        None => sqlx::query_scalar(
            "SELECT t.id
             FROM tenants t
             JOIN memberships m ON m.tenant_id = t.id
             WHERE m.user_id = $1
             ORDER BY t.is_default DESC, t.id ASC
             LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&mut *worker_tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("resolve default login tenant: {err}")))?,
    };

    worker_tx
        .commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit login lookup tx: {err}")))?;

    let Some(tenant_id) = resolved_tenant_id else {
        warn!(user_id, "login failed: no membership in target tenant");
        return Err(AppError::Unauthorized("invalid credentials"));
    };

    open_session(
        pool,
        config,
        tenant_id,
        AuthenticatedUser {
            id: user_id,
            email: user_email,
            name: user_name,
        },
    )
    .await
}

/// Create a new session for an already-authenticated `user` in `tenant_id`.
/// Returns the session and the raw cookie token (only its hash is stored).
pub(crate) async fn open_session(
    pool: &PgPool,
    config: &Config,
    tenant_id: i64,
    user: AuthenticatedUser,
) -> Result<(AuthSession, String), AppError> {
    // Execute tenant-scoped session creation inside an RLS-enforced tenant transaction.
    let tenant_ctx = TenantContext::new(tenant_id)?;
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;

    let tenant = load_tenant_summary(&mut tx, tenant_id).await?;
    let (role, tenant_role, workspace_roles) =
        load_user_memberships(&mut tx, tenant_id, user.id).await?;
    let default_workspace =
        load_default_workspace_for_user(&mut tx, tenant_id, tenant_role, &workspace_roles).await?;

    let raw_token = generate_random_token_hex();
    let token_hash = hash_session_token(&raw_token)?;
    let csrf_token = generate_random_token_hex();
    let expires_at = Utc::now() + Duration::hours(i64::from(config.session_ttl_hours));

    sqlx::query(
        "DELETE FROM sessions WHERE tenant_id = $1 AND user_id = $2 AND expires_at <= NOW()",
    )
    .bind(tenant_id)
    .bind(user.id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("prune expired sessions: {err}")))?;

    let session_id: i64 = sqlx::query_scalar(
        "INSERT INTO sessions (tenant_id, user_id, token_hash, csrf_token, expires_at)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(user.id)
    .bind(&token_hash)
    .bind(&csrf_token)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert session: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit session creation: {err}")))?;

    let session = AuthSession {
        session_id,
        tenant_ctx,
        tenant,
        default_workspace,
        user,
        role,
        tenant_role,
        workspace_roles,
        api_token: None,
        csrf_token,
    };

    Ok((session, raw_token))
}

pub async fn authenticate_cookie(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    let raw_token = extract_cookie_value(headers, SESSION_COOKIE_NAME)
        .ok_or(AppError::Unauthorized("authentication required"))?;
    let token_hash = hash_session_token(&raw_token)?;

    // Resolve tenant_id for this session token hash.
    let mut worker_tx = begin_worker_tx(pool).await?;
    let session_locator: Option<(i64, i64)> = sqlx::query_as(
        "SELECT tenant_id, id
         FROM sessions
         WHERE token_hash = $1 AND expires_at > NOW()",
    )
    .bind(&token_hash)
    .fetch_optional(&mut *worker_tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("locate session token: {err}")))?;
    worker_tx
        .commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit session locator tx: {err}")))?;

    let Some((tenant_id, session_id)) = session_locator else {
        return Err(AppError::Unauthorized("invalid or expired session"));
    };

    // Verify full session, active user, membership, tenant, and default workspace under RLS.
    let tenant_ctx = TenantContext::new(tenant_id)?;
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;

    let session_user: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT u.id, u.email, u.name, s.csrf_token
         FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.tenant_id = $1
           AND s.id = $2
           AND s.token_hash = $3
           AND s.expires_at > NOW()
           AND u.is_active = TRUE",
    )
    .bind(tenant_id)
    .bind(session_id)
    .bind(&token_hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load authenticated session user: {err}")))?;

    let Some((user_id, email, name, csrf_token)) = session_user else {
        return Err(AppError::Unauthorized("invalid or expired session"));
    };

    let tenant = load_tenant_summary(&mut tx, tenant_id).await?;
    let (role, tenant_role, workspace_roles) =
        load_user_memberships(&mut tx, tenant_id, user_id).await?;
    let default_workspace =
        load_default_workspace_for_user(&mut tx, tenant_id, tenant_role, &workspace_roles).await?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit session auth tx: {err}")))?;

    Ok(AuthSession {
        session_id,
        tenant_ctx,
        tenant,
        default_workspace,
        user: AuthenticatedUser {
            id: user_id,
            email,
            name,
        },
        role,
        tenant_role,
        workspace_roles,
        api_token: None,
        csrf_token,
    })
}

pub async fn logout(pool: &PgPool, session: &AuthSession) -> Result<(), AppError> {
    session.require_user_session()?;
    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    sqlx::query("DELETE FROM sessions WHERE tenant_id = $1 AND id = $2")
        .bind(session.tenant_ctx.tenant_id())
        .bind(session.session_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("delete session on logout: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit logout tx: {err}")))?;
    Ok(())
}

/// Change the signed-in user's password after verifying the current one. Every other
/// session of the user (in any tenant) is revoked; the current session stays signed in.
pub async fn change_password(
    pool: &PgPool,
    limiter: &LoginRateLimiter,
    session: &AuthSession,
    req: ChangePasswordRequest,
) -> Result<(), AppError> {
    session.require_user_session()?;
    if req.current_password.is_empty() || req.current_password.len() > 256 {
        return Err(AppError::Validation("invalid password length".to_owned()));
    }
    if let Err(retry_after_secs) = limiter.check_and_record(
        &format!("password-change:{}", session.user.id),
        Instant::now(),
    ) {
        warn!(
            user_id = session.user.id,
            "password change rate limit exceeded"
        );
        return Err(AppError::RateLimited { retry_after_secs });
    }
    // Validates the 12..=256 length rule before any database work.
    let new_hash = hash_password_argon2id(&req.new_password)?;

    // `users` and cross-tenant `sessions` are global, so this runs as the worker role.
    let mut tx = begin_worker_tx(pool).await?;
    let current_hash: Option<String> = sqlx::query_scalar(
        "SELECT password_hash FROM users WHERE id = $1 AND is_active = TRUE FOR UPDATE",
    )
    .bind(session.user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load password hash: {err}")))?;
    let Some(current_hash) = current_hash else {
        return Err(AppError::Unauthorized("invalid or expired session"));
    };
    if !verify_password_argon2id(&req.current_password, &current_hash)? {
        warn!(
            user_id = session.user.id,
            "password change failed: wrong current password"
        );
        return Err(AppError::Forbidden("current password is incorrect"));
    }

    sqlx::query("UPDATE users SET password_hash = $2, updated_at = NOW() WHERE id = $1")
        .bind(session.user.id)
        .bind(&new_hash)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("update password hash: {err}")))?;
    let revoked =
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND NOT (tenant_id = $2 AND id = $3)")
            .bind(session.user.id)
            .bind(session.tenant_ctx.tenant_id())
            .bind(session.session_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("revoke other sessions: {err}")))?
            .rows_affected();
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit password change: {err}")))?;

    info!(
        user_id = session.user.id,
        revoked_sessions = revoked,
        "password changed"
    );
    Ok(())
}

#[derive(sqlx::FromRow)]
struct TenantSummaryRow {
    id: i64,
    slug: String,
    name: String,
    retention_days: i16,
    default_rate_limit_per_minute: i32,
    aggregate_rate_limit_per_minute: i32,
    is_default: bool,
}

#[derive(sqlx::FromRow)]
struct WorkspaceSummaryRow {
    id: i64,
    slug: String,
    name: String,
    retention_days_override: Option<i16>,
    default_rate_limit_per_minute_override: Option<i32>,
    is_default: bool,
}

pub(crate) async fn load_tenant_summary(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
) -> Result<TenantSummary, AppError> {
    let row: Option<TenantSummaryRow> = sqlx::query_as(
        "SELECT id, slug, name, retention_days,
                default_rate_limit_per_minute, aggregate_rate_limit_per_minute, is_default
         FROM tenants
         WHERE id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load tenant summary: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::Unauthorized("tenant not found"));
    };
    let retention_days = u16::try_from(row.retention_days)
        .map_err(|_| AppError::Validation("invalid tenant retention_days".to_owned()))?;

    Ok(TenantSummary {
        id: row.id,
        slug: row.slug,
        name: row.name,
        retention_days,
        default_rate_limit_per_minute: row.default_rate_limit_per_minute,
        aggregate_rate_limit_per_minute: row.aggregate_rate_limit_per_minute,
        is_default: row.is_default,
    })
}

async fn load_user_memberships(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    user_id: i64,
) -> Result<(Role, Option<Role>, BTreeMap<i64, Role>), AppError> {
    let rows: Vec<(Option<i64>, String)> = sqlx::query_as(
        "SELECT workspace_id, role
         FROM memberships
         WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load user memberships: {err}")))?;

    let mut best: Option<Role> = None;
    let mut tenant_role: Option<Role> = None;
    let mut workspace_roles: BTreeMap<i64, Role> = BTreeMap::new();

    for (ws_id_opt, raw_role) in rows {
        let parsed = Role::parse(&raw_role)?;
        if best.is_none_or(|current| parsed.privilege_rank() > current.privilege_rank()) {
            best = Some(parsed);
        }
        match ws_id_opt {
            None => {
                if tenant_role
                    .is_none_or(|current| parsed.privilege_rank() > current.privilege_rank())
                {
                    tenant_role = Some(parsed);
                }
            }
            Some(ws_id) => {
                workspace_roles
                    .entry(ws_id)
                    .and_modify(|cur| {
                        if parsed.privilege_rank() > cur.privilege_rank() {
                            *cur = parsed;
                        }
                    })
                    .or_insert(parsed);
            }
        }
    }

    let highest = best.ok_or(AppError::Unauthorized("no active tenant membership"))?;
    Ok((highest, tenant_role, workspace_roles))
}

async fn load_default_workspace_for_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    tenant_role: Option<Role>,
    workspace_roles: &BTreeMap<i64, Role>,
) -> Result<WorkspaceSummary, AppError> {
    let row: Option<WorkspaceSummaryRow> = if tenant_role.is_some() || workspace_roles.is_empty() {
        sqlx::query_as(
            "SELECT id, slug, name, retention_days_override,
                    default_rate_limit_per_minute_override, is_default
             FROM workspaces
             WHERE tenant_id = $1
             ORDER BY is_default DESC, id ASC
             LIMIT 1",
        )
        .bind(tenant_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("load default workspace: {err}")))?
    } else {
        let allowed_ids: Vec<i64> = workspace_roles.keys().copied().collect();
        sqlx::query_as(
            "SELECT id, slug, name, retention_days_override,
                    default_rate_limit_per_minute_override, is_default
             FROM workspaces
             WHERE tenant_id = $1 AND id = ANY($2)
             ORDER BY is_default DESC, id ASC
             LIMIT 1",
        )
        .bind(tenant_id)
        .bind(&allowed_ids)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("load user scoped default workspace: {err}"))
        })?
    };

    let Some(row) = row else {
        return Err(AppError::NotFound);
    };
    let retention_days_override = match row.retention_days_override {
        Some(v) => Some(
            u16::try_from(v)
                .map_err(|_| AppError::Validation("invalid workspace retention".to_owned()))?,
        ),
        None => None,
    };

    Ok(WorkspaceSummary {
        id: row.id,
        slug: row.slug,
        name: row.name,
        retention_days_override,
        default_rate_limit_per_minute_override: row.default_rate_limit_per_minute_override,
        is_default: row.is_default,
    })
}
