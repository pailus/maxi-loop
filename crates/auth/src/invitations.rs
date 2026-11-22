//! F022: invite-by-link onboarding.
//!
//! An admin creates an invitation for an email + role + scope; the server returns a one-time
//! link containing a 256-bit random token (only its SHA-256 is stored). The invitee opens the
//! link, sets their own password (or proves an existing account's password) and is signed in.

use std::time::Instant;

use axum::extract::{FromRef, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use maxi_loop_platform::bootstrap::{hash_password_argon2id, verify_password_argon2id};
use maxi_loop_platform::config::is_valid_email;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{begin_tenant_tx, begin_worker_tx};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use tracing::{info, warn};

use crate::http::AuthState;
use crate::members::resolve_grant_target;
use crate::model::{AuthSession, AuthenticatedUser, Role};
use crate::service::open_session;
use crate::token::{build_session_set_cookie, generate_random_token_hex, hash_session_token};

/// Invitations expire after this many days.
pub const INVITATION_TTL_DAYS: i64 = 7;

const INVALID_INVITATION: &str = "invalid or expired invitation";

#[derive(Clone, Debug, Serialize)]
pub struct InvitationView {
    pub id: i64,
    pub email: String,
    pub role: Role,
    pub scope: &'static str,
    pub workspace_id: Option<i64>,
    pub workspace_slug: Option<String>,
    pub workspace_name: Option<String>,
    pub invited_by_email: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub is_expired: bool,
}

/// Returned once on creation: `invite_url` carries the raw token and is never retrievable again.
#[derive(Clone, Debug, Serialize)]
pub struct CreatedInvitation {
    pub invitation: InvitationView,
    pub invite_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateInvitationInput {
    pub email: String,
    pub role: String,
    pub workspace_slug: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct InvitationTokenInput {
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct InvitationPreview {
    pub tenant_name: String,
    pub email: String,
    pub role: Role,
    pub scope: &'static str,
    pub workspace_name: Option<String>,
    /// `true` when the email already has an account: the invitee must enter that password.
    pub account_exists: bool,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AcceptInvitationInput {
    pub token: String,
    /// Required only when creating a new account.
    pub name: Option<String>,
    /// New password for a new account, or the existing account's password.
    pub password: String,
}

#[derive(FromRow)]
struct InvitationRow {
    id: i64,
    email: String,
    role: String,
    workspace_id: Option<i64>,
    workspace_slug: Option<String>,
    workspace_name: Option<String>,
    invited_by_email: Option<String>,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

impl InvitationRow {
    fn into_view(self, now: DateTime<Utc>) -> Result<InvitationView, AppError> {
        Ok(InvitationView {
            id: self.id,
            email: self.email,
            role: Role::parse(&self.role)?,
            scope: scope_label(self.workspace_id),
            workspace_id: self.workspace_id,
            workspace_slug: self.workspace_slug,
            workspace_name: self.workspace_name,
            invited_by_email: self.invited_by_email,
            is_expired: self.expires_at <= now,
            expires_at: self.expires_at,
            created_at: self.created_at,
        })
    }
}

fn scope_label(workspace_id: Option<i64>) -> &'static str {
    if workspace_id.is_some() {
        "workspace"
    } else {
        "tenant"
    }
}

/// Invite tokens share the session-token format (64 hex chars) and SHA-256 storage.
fn hash_invitation_token(raw: &str) -> Result<String, AppError> {
    hash_session_token(raw).map_err(|_| AppError::Unauthorized(INVALID_INVITATION))
}

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/invitations",
            get(list_invitations_handler).post(create_invitation_handler),
        )
        .route(
            "/invitations/{invitation_id}",
            delete(revoke_invitation_handler),
        )
        .route(
            "/auth/invitations/preview",
            post(preview_invitation_handler),
        )
        .route("/auth/invitations/accept", post(accept_invitation_handler))
}

async fn list_invitations_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<InvitationView>>, AppError> {
    Ok(Json(list_invitations(&state.db, &session).await?))
}

async fn create_invitation_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateInvitationInput>,
) -> Result<(StatusCode, Json<CreatedInvitation>), AppError> {
    let created =
        create_invitation(&state.db, &session, &state.config.public_base_url, input).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn revoke_invitation_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(invitation_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    revoke_invitation(&state.db, &session, invitation_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn preview_invitation_handler(
    State(state): State<AuthState>,
    Json(input): Json<InvitationTokenInput>,
) -> Result<Json<InvitationPreview>, AppError> {
    Ok(Json(preview_invitation(&state.db, &input.token).await?))
}

async fn accept_invitation_handler(
    State(state): State<AuthState>,
    Json(input): Json<AcceptInvitationInput>,
) -> Result<Response, AppError> {
    let (tenant_id, user) = accept_invitation(&state, input).await?;
    let (session, raw_token) = open_session(&state.db, &state.config, tenant_id, user).await?;

    let max_age_secs = u64::from(state.config.session_ttl_hours) * 3600;
    let set_cookie = build_session_set_cookie(&raw_token, max_age_secs, state.config.cookie_secure);
    let view = session.to_view(state.config.single_tenant_mode);
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, set_cookie)],
        Json(view),
    )
        .into_response())
}

const INVITATION_SELECT: &str = "SELECT i.id, i.email, i.role, i.workspace_id,
        w.slug AS workspace_slug, w.name AS workspace_name,
        u.email AS invited_by_email, i.expires_at, i.created_at
     FROM invitations i
     LEFT JOIN workspaces w ON w.tenant_id = i.tenant_id AND w.id = i.workspace_id
     LEFT JOIN users u ON u.id = i.invited_by_user_id";

pub async fn list_invitations(
    pool: &PgPool,
    session: &AuthSession,
) -> Result<Vec<InvitationView>, AppError> {
    session.require_user_session()?;
    if !session.can_manage_members() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    let tenant_id = session.tenant_ctx.tenant_id();
    // Tenant managers see every open invite; workspace admins only those of their workspaces.
    let all_scopes = session.can_manage_tenant();
    let managed_ws_ids: Vec<i64> = session
        .workspace_roles
        .iter()
        .filter_map(|(&ws_id, role)| role.can_manage_projects().then_some(ws_id))
        .collect();

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let rows: Vec<InvitationRow> = sqlx::query_as(&format!(
        "{INVITATION_SELECT}
         WHERE i.tenant_id = $1
           AND i.accepted_at IS NULL AND i.revoked_at IS NULL
           AND ($2 OR i.workspace_id = ANY($3))
         ORDER BY i.created_at DESC
         LIMIT 200"
    ))
    .bind(tenant_id)
    .bind(all_scopes)
    .bind(&managed_ws_ids)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list invitations: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list invitations: {err}")))?;

    let now = Utc::now();
    rows.into_iter().map(|row| row.into_view(now)).collect()
}

pub async fn create_invitation(
    pool: &PgPool,
    session: &AuthSession,
    public_base_url: &str,
    input: CreateInvitationInput,
) -> Result<CreatedInvitation, AppError> {
    session.require_user_session()?;
    if !session.can_manage_members() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    let email = input.email.trim().to_ascii_lowercase();
    if !is_valid_email(&email) {
        return Err(AppError::Validation("invalid email format".to_owned()));
    }
    let ws_slug = input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (role, target_workspace) =
        resolve_grant_target(pool, session, input.role.trim(), ws_slug).await?;
    let ws_id = target_workspace.as_ref().map(|(id, _, _)| *id);
    let tenant_id = session.tenant_ctx.tenant_id();

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;

    // Users are visible under tenant RLS only when they already belong to this tenant,
    // which is exactly the set that could already hold this membership.
    let already_member: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM memberships m JOIN users u ON u.id = m.user_id
             WHERE m.tenant_id = $1 AND u.email = $2
               AND m.workspace_id IS NOT DISTINCT FROM $3
         )",
    )
    .bind(tenant_id)
    .bind(&email)
    .bind(ws_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("check existing membership: {err}")))?;
    if already_member {
        return Err(AppError::Conflict(
            "user already has a membership in this scope",
        ));
    }

    sqlx::query(
        "UPDATE invitations SET revoked_at = NOW()
         WHERE tenant_id = $1 AND COALESCE(workspace_id, 0) = COALESCE($2, 0) AND email = $3
           AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(tenant_id)
    .bind(ws_id)
    .bind(&email)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("revoke previous invitation: {err}")))?;

    let raw_token = generate_random_token_hex();
    let token_hash = hash_session_token(&raw_token)?;
    let expires_at = Utc::now() + Duration::days(INVITATION_TTL_DAYS);

    let (id, created_at): (i64, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO invitations
             (tenant_id, workspace_id, email, role, token_hash, invited_by_user_id, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, created_at",
    )
    .bind(tenant_id)
    .bind(ws_id)
    .bind(&email)
    .bind(role.as_str())
    .bind(&token_hash)
    .bind(session.user.id)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert invitation: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create invitation: {err}")))?;

    info!(
        tenant_id,
        actor_user_id = session.user.id,
        invitation_id = id,
        role = role.as_str(),
        workspace_id = ?ws_id,
        "invitation created"
    );

    let (workspace_slug, workspace_name) = match target_workspace {
        Some((_, slug, name)) => (Some(slug), Some(name)),
        None => (None, None),
    };
    // Token travels in the URL fragment so it never reaches server logs or Referer headers.
    let invite_url = format!(
        "{}/#invite={raw_token}",
        public_base_url.trim_end_matches('/')
    );

    Ok(CreatedInvitation {
        invitation: InvitationView {
            id,
            email,
            role,
            scope: scope_label(ws_id),
            workspace_id: ws_id,
            workspace_slug,
            workspace_name,
            invited_by_email: Some(session.user.email.clone()),
            expires_at,
            created_at,
            is_expired: false,
        },
        invite_url,
    })
}

pub async fn revoke_invitation(
    pool: &PgPool,
    session: &AuthSession,
    invitation_id: i64,
) -> Result<(), AppError> {
    session.require_user_session()?;
    if !session.can_manage_members() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    let tenant_id = session.tenant_ctx.tenant_id();
    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;

    let row: Option<(Option<i64>, String)> = sqlx::query_as(
        "SELECT workspace_id, role FROM invitations
         WHERE tenant_id = $1 AND id = $2 AND accepted_at IS NULL AND revoked_at IS NULL
         FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(invitation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load invitation for revoke: {err}")))?;
    let Some((ws_id, raw_role)) = row else {
        return Err(AppError::NotFound);
    };

    let allowed = match ws_id {
        None => {
            session.can_manage_tenant()
                && (Role::parse(&raw_role)? != Role::TenantOwner
                    || session.tenant_role == Some(Role::TenantOwner))
        }
        Some(id) => session.can_manage_workspace(id),
    };
    if !allowed {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    sqlx::query("UPDATE invitations SET revoked_at = NOW() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(invitation_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("revoke invitation: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit revoke invitation: {err}")))?;

    info!(
        tenant_id,
        actor_user_id = session.user.id,
        invitation_id,
        "invitation revoked"
    );
    Ok(())
}

#[derive(FromRow)]
struct OpenInvitationRow {
    tenant_id: i64,
    id: i64,
    email: String,
    role: String,
    workspace_id: Option<i64>,
    expires_at: DateTime<Utc>,
    tenant_name: String,
    workspace_name: Option<String>,
}

// The invitee has no session yet, so the lookup runs as the worker role (like login).
const OPEN_INVITATION_SELECT: &str = "SELECT i.tenant_id, i.id, i.email, i.role, i.workspace_id,
        i.expires_at, t.name AS tenant_name, w.name AS workspace_name
     FROM invitations i
     JOIN tenants t ON t.id = i.tenant_id
     LEFT JOIN workspaces w ON w.tenant_id = i.tenant_id AND w.id = i.workspace_id
     WHERE i.token_hash = $1
       AND i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > NOW()";

pub async fn preview_invitation(
    pool: &PgPool,
    raw_token: &str,
) -> Result<InvitationPreview, AppError> {
    let token_hash = hash_invitation_token(raw_token)?;
    let mut tx = begin_worker_tx(pool).await?;
    let row: Option<OpenInvitationRow> = sqlx::query_as(OPEN_INVITATION_SELECT)
        .bind(&token_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("load invitation preview: {err}")))?;
    let Some(row) = row else {
        return Err(AppError::Unauthorized(INVALID_INVITATION));
    };
    let account_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)")
            .bind(&row.email)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("check invitee account: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit invitation preview: {err}")))?;

    Ok(InvitationPreview {
        tenant_name: row.tenant_name,
        email: row.email,
        role: Role::parse(&row.role)?,
        scope: scope_label(row.workspace_id),
        workspace_name: row.workspace_name,
        account_exists,
        expires_at: row.expires_at,
    })
}

/// Accept an invitation: create the account (or verify the existing account's password),
/// grant the membership and consume the invite, all in one transaction.
/// Returns the tenant and user to open a session for.
pub async fn accept_invitation(
    state: &AuthState,
    input: AcceptInvitationInput,
) -> Result<(i64, AuthenticatedUser), AppError> {
    let token_hash = hash_invitation_token(&input.token)?;
    if input.password.is_empty() || input.password.len() > 256 {
        return Err(AppError::Validation("invalid password length".to_owned()));
    }

    let mut tx = begin_worker_tx(&state.db).await?;
    // Row lock makes a second concurrent accept of the same token wait, then see it consumed.
    let row: Option<OpenInvitationRow> =
        sqlx::query_as(&format!("{OPEN_INVITATION_SELECT} FOR UPDATE OF i"))
            .bind(&token_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("load invitation to accept: {err}")))?;
    let Some(invite) = row else {
        warn!("invitation accept failed: invalid or expired token");
        return Err(AppError::Unauthorized(INVALID_INVITATION));
    };

    let existing_user: Option<(i64, String, String, String, bool)> = sqlx::query_as(
        "SELECT id, email, name, password_hash, is_active FROM users WHERE email = $1 FOR UPDATE",
    )
    .bind(&invite.email)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup invitee account: {err}")))?;

    let user = match existing_user {
        Some((id, email, name, password_hash, is_active)) => {
            // Proving the existing password shares the login rate-limit bucket for this email,
            // so the invite flow cannot be used to bypass login throttling.
            if let Err(retry_after_secs) =
                state.login_limiter.check_and_record(&email, Instant::now())
            {
                return Err(AppError::RateLimited { retry_after_secs });
            }
            if !verify_password_argon2id(&input.password, &password_hash)? || !is_active {
                warn!(
                    user_id = id,
                    "invitation accept failed: invalid credentials"
                );
                return Err(AppError::Unauthorized("invalid credentials"));
            }
            AuthenticatedUser { id, email, name }
        }
        None => {
            let name = input
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.chars().count() <= 128)
                .ok_or_else(|| {
                    AppError::Validation("name must be between 1 and 128 characters".to_owned())
                })?;
            let password_hash = hash_password_argon2id(&input.password)?;
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO users (email, name, password_hash, is_active)
                 VALUES ($1, $2, $3, TRUE) RETURNING id",
            )
            .bind(&invite.email)
            .bind(name)
            .bind(&password_hash)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("insert invited user: {err}")))?;
            AuthenticatedUser {
                id,
                email: invite.email.clone(),
                name: name.to_owned(),
            }
        }
    };

    // Someone may have granted this scope directly since the invite was sent; then the
    // invite is simply consumed.
    let already_member: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM memberships
             WHERE tenant_id = $1 AND user_id = $2 AND workspace_id IS NOT DISTINCT FROM $3
         )",
    )
    .bind(invite.tenant_id)
    .bind(user.id)
    .bind(invite.workspace_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("check invitee membership: {err}")))?;
    if !already_member {
        sqlx::query(
            "INSERT INTO memberships (tenant_id, workspace_id, user_id, role)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(invite.tenant_id)
        .bind(invite.workspace_id)
        .bind(user.id)
        .bind(&invite.role)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("insert invited membership: {err}")))?;
    }

    sqlx::query(
        "UPDATE invitations SET accepted_at = NOW(), accepted_user_id = $3
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(invite.tenant_id)
    .bind(invite.id)
    .bind(user.id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("mark invitation accepted: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit accept invitation: {err}")))?;

    info!(
        tenant_id = invite.tenant_id,
        invitation_id = invite.id,
        user_id = user.id,
        role = invite.role.as_str(),
        workspace_id = ?invite.workspace_id,
        expires_at = %invite.expires_at,
        "invitation accepted"
    );
    Ok((invite.tenant_id, user))
}
