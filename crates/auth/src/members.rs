use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use maxi_loop_platform::bootstrap::hash_password_argon2id;
use maxi_loop_platform::config::{is_valid_email, is_valid_slug};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{begin_tenant_tx, begin_worker_tx};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use tracing::info;

use crate::http::AuthState;
use crate::model::{AuthSession, Role};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantMemberView {
    pub membership_id: i64,
    pub tenant_id: i64,
    pub user_id: i64,
    pub email: String,
    pub name: String,
    pub is_active: bool,
    pub scope: String,
    pub workspace_id: Option<i64>,
    pub workspace_slug: Option<String>,
    pub workspace_name: Option<String>,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateMemberInput {
    pub email: String,
    pub name: Option<String>,
    /// Required when creating a new user account (min 12 chars, hashed with Argon2id).
    /// Ignored when attaching an already-existing user so another tenant's credentials cannot be overwritten.
    pub password: Option<String>,
    pub role: String,
    /// `None` for tenant-wide role (`tenant_owner`, `tenant_admin`, `member`, `viewer`);
    /// `Some(slug)` for workspace-scoped role (`workspace_admin`, `member`, `viewer`).
    pub workspace_slug: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateMemberRoleInput {
    pub role: String,
}

#[derive(FromRow)]
struct MemberDbRow {
    membership_id: i64,
    tenant_id: i64,
    user_id: i64,
    email: String,
    name: String,
    is_active: bool,
    workspace_id: Option<i64>,
    workspace_slug: Option<String>,
    workspace_name: Option<String>,
    role: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<MemberDbRow> for TenantMemberView {
    type Error = AppError;

    fn try_from(row: MemberDbRow) -> Result<Self, Self::Error> {
        let role = Role::parse(&row.role)?;
        let scope = if row.workspace_id.is_some() {
            "workspace".to_owned()
        } else {
            "tenant".to_owned()
        };
        Ok(Self {
            membership_id: row.membership_id,
            tenant_id: row.tenant_id,
            user_id: row.user_id,
            email: row.email,
            name: row.name,
            is_active: row.is_active,
            scope,
            workspace_id: row.workspace_id,
            workspace_slug: row.workspace_slug,
            workspace_name: row.workspace_name,
            role,
            created_at: row.created_at,
        })
    }
}

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/members",
            get(list_members_handler).post(create_member_handler),
        )
        .route(
            "/members/{membership_id}",
            patch(update_member_role_handler).delete(delete_member_handler),
        )
}

async fn list_members_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<TenantMemberView>>, AppError> {
    let members = list_members(&state.db, &session).await?;
    Ok(Json(members))
}

async fn create_member_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateMemberInput>,
) -> Result<(StatusCode, Json<TenantMemberView>), AppError> {
    let created = create_member(&state.db, &session, input).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn update_member_role_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(membership_id): Path<i64>,
    Json(input): Json<UpdateMemberRoleInput>,
) -> Result<Json<TenantMemberView>, AppError> {
    let updated = update_member_role(&state.db, &session, membership_id, input).await?;
    Ok(Json(updated))
}

async fn delete_member_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(membership_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    delete_member(&state.db, &session, membership_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_members(
    pool: &PgPool,
    session: &AuthSession,
) -> Result<Vec<TenantMemberView>, AppError> {
    session.require_user_session()?;

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let tenant_id = session.tenant_ctx.tenant_id();

    let rows: Vec<MemberDbRow> = if session.can_manage_tenant() {
        sqlx::query_as(
            "SELECT m.id AS membership_id, m.tenant_id, m.user_id,
                    u.email, u.name, u.is_active,
                    m.workspace_id, w.slug AS workspace_slug, w.name AS workspace_name,
                    m.role, m.created_at
             FROM memberships m
             JOIN users u ON u.id = m.user_id
             LEFT JOIN workspaces w ON w.tenant_id = m.tenant_id AND w.id = m.workspace_id
             WHERE m.tenant_id = $1
             ORDER BY m.workspace_id NULLS FIRST, u.email ASC, m.id ASC
             LIMIT 500",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list tenant members: {err}")))?
    } else if session.can_manage_members() {
        let managed_ws_ids: Vec<i64> = session
            .workspace_roles
            .iter()
            .filter_map(|(&ws_id, role)| role.can_manage_projects().then_some(ws_id))
            .collect();
        sqlx::query_as(
            "SELECT m.id AS membership_id, m.tenant_id, m.user_id,
                    u.email, u.name, u.is_active,
                    m.workspace_id, w.slug AS workspace_slug, w.name AS workspace_name,
                    m.role, m.created_at
             FROM memberships m
             JOIN users u ON u.id = m.user_id
             LEFT JOIN workspaces w ON w.tenant_id = m.tenant_id AND w.id = m.workspace_id
             WHERE m.tenant_id = $1
               AND (m.user_id = $2 OR m.workspace_id = ANY($3))
             ORDER BY m.workspace_id NULLS FIRST, u.email ASC, m.id ASC
             LIMIT 500",
        )
        .bind(tenant_id)
        .bind(session.user.id)
        .bind(&managed_ws_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list workspace members: {err}")))?
    } else {
        sqlx::query_as(
            "SELECT m.id AS membership_id, m.tenant_id, m.user_id,
                    u.email, u.name, u.is_active,
                    m.workspace_id, w.slug AS workspace_slug, w.name AS workspace_name,
                    m.role, m.created_at
             FROM memberships m
             JOIN users u ON u.id = m.user_id
             LEFT JOIN workspaces w ON w.tenant_id = m.tenant_id AND w.id = m.workspace_id
             WHERE m.tenant_id = $1 AND m.user_id = $2
             ORDER BY m.workspace_id NULLS FIRST, m.id ASC
             LIMIT 100",
        )
        .bind(tenant_id)
        .bind(session.user.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list self memberships: {err}")))?
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_members: {err}")))?;

    rows.into_iter().map(TenantMemberView::try_from).collect()
}

pub async fn create_member(
    pool: &PgPool,
    session: &AuthSession,
    input: CreateMemberInput,
) -> Result<TenantMemberView, AppError> {
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
    let tenant_id = session.tenant_ctx.tenant_id();

    // 1. Validate scope, workspace existence, and caller privilege hierarchy BEFORE touching `users`.
    let (target_role, target_workspace) =
        resolve_grant_target(pool, session, input.role.trim(), ws_slug).await?;

    // 2. Look up or create the user record via worker tx (users table is global across tenants).
    // Never overwrite an existing user's password so cross-tenant credential takeover is impossible.
    let mut worker_tx = begin_worker_tx(pool).await?;
    let existing_user: Option<(i64, String, String, bool)> =
        sqlx::query_as("SELECT id, email, name, is_active FROM users WHERE email = $1")
            .bind(&email)
            .fetch_optional(&mut *worker_tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("lookup user by email: {err}")))?;

    let (user_id, user_email, user_name, is_active) = match existing_user {
        Some(row) => row,
        None => {
            let clean_name = input
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    AppError::Validation("name is required when creating a new user".to_owned())
                })?;
            if clean_name.len() > 128 {
                return Err(AppError::Validation(
                    "name must be between 1 and 128 characters".to_owned(),
                ));
            }
            let raw_password = input.password.as_deref().ok_or_else(|| {
                AppError::Validation(
                    "password (min 12 characters) is required when creating a new user".to_owned(),
                )
            })?;
            let password_hash = hash_password_argon2id(raw_password)?;
            let inserted: (i64, String, String, bool) = sqlx::query_as(
                "INSERT INTO users (email, name, password_hash, is_active)
                 VALUES ($1, $2, $3, TRUE)
                 RETURNING id, email, name, is_active",
            )
            .bind(&email)
            .bind(clean_name)
            .bind(&password_hash)
            .fetch_one(&mut *worker_tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("insert user: {err}")))?;
            inserted
        }
    };
    worker_tx
        .commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit user lookup/create tx: {err}")))?;

    // 3. Insert the tenant-scoped membership under Postgres RLS.
    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
    let ws_id_opt = target_workspace.as_ref().map(|(id, _, _)| *id);

    let insert_res: Result<(i64, DateTime<Utc>), sqlx::Error> = sqlx::query_as(
        "INSERT INTO memberships (tenant_id, workspace_id, user_id, role)
         VALUES ($1, $2, $3, $4)
         RETURNING id, created_at",
    )
    .bind(tenant_id)
    .bind(ws_id_opt)
    .bind(user_id)
    .bind(target_role.as_str())
    .fetch_one(&mut *tx)
    .await;

    let (membership_id, created_at) = match insert_res {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
            return Err(AppError::Conflict(
                "user already has a membership in this scope",
            ));
        }
        Err(err) => return Err(AppError::DatabaseQuery(format!("insert membership: {err}"))),
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create_member: {err}")))?;

    info!(
        tenant_id,
        actor_user_id = session.user.id,
        target_user_id = user_id,
        role = target_role.as_str(),
        workspace_id = ?ws_id_opt,
        "membership created"
    );

    let (workspace_id, workspace_slug, workspace_name, scope) = match target_workspace {
        Some((id, slug, name)) => (Some(id), Some(slug), Some(name), "workspace".to_owned()),
        None => (None, None, None, "tenant".to_owned()),
    };

    Ok(TenantMemberView {
        membership_id,
        tenant_id,
        user_id,
        email: user_email,
        name: user_name,
        is_active,
        scope,
        workspace_id,
        workspace_slug,
        workspace_name,
        role: target_role,
        created_at,
    })
}

/// Workspace a role is granted in: `(id, slug, name)`.
pub(crate) type GrantWorkspace = (i64, String, String);

/// Validate that `session` may grant `raw_role` in the given scope (tenant-wide when
/// `ws_slug` is `None`). Shared by direct member creation and invitations so both
/// enforce the same privilege hierarchy.
pub(crate) async fn resolve_grant_target(
    pool: &PgPool,
    session: &AuthSession,
    raw_role: &str,
    ws_slug: Option<&str>,
) -> Result<(Role, Option<GrantWorkspace>), AppError> {
    let target_role = Role::parse(raw_role)?;
    let tenant_id = session.tenant_ctx.tenant_id();
    let target_workspace: Option<(i64, String, String)> = match ws_slug {
        None => {
            if !target_role.is_valid_tenant_role() {
                return Err(AppError::Validation(
                    "tenant-level role must be tenant_owner, tenant_admin, member, or viewer"
                        .to_owned(),
                ));
            }
            if !session.can_manage_tenant() {
                return Err(AppError::Forbidden(
                    "only tenant owners or tenant admins can assign tenant-level roles",
                ));
            }
            if target_role == Role::TenantOwner && session.tenant_role != Some(Role::TenantOwner) {
                return Err(AppError::Forbidden(
                    "only a tenant_owner can grant the tenant_owner role",
                ));
            }
            None
        }
        Some(slug) => {
            if !is_valid_slug(slug) {
                return Err(AppError::Validation("invalid workspace_slug".to_owned()));
            }
            if !target_role.is_valid_workspace_role() {
                return Err(AppError::Validation(
                    "workspace-level role must be workspace_admin, member, or viewer".to_owned(),
                ));
            }
            let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;
            let ws_row: Option<(i64, String, String)> = sqlx::query_as(
                "SELECT id, slug, name FROM workspaces WHERE tenant_id = $1 AND slug = $2",
            )
            .bind(tenant_id)
            .bind(slug)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("lookup workspace for member: {err}"))
            })?;
            tx.commit().await.map_err(|err| {
                AppError::DatabaseQuery(format!("commit member ws lookup: {err}"))
            })?;

            let Some((ws_id, ws_slug_val, ws_name)) = ws_row else {
                return Err(AppError::NotFound);
            };
            if !session.can_manage_workspace(ws_id) {
                return Err(AppError::Forbidden(
                    "insufficient permissions for target workspace",
                ));
            }
            Some((ws_id, ws_slug_val, ws_name))
        }
    };
    Ok((target_role, target_workspace))
}

pub async fn update_member_role(
    pool: &PgPool,
    session: &AuthSession,
    membership_id: i64,
    input: UpdateMemberRoleInput,
) -> Result<TenantMemberView, AppError> {
    session.require_user_session()?;
    if membership_id <= 0 {
        return Err(AppError::Validation("invalid membership_id".to_owned()));
    }
    if !session.can_manage_members() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let new_role = Role::parse(input.role.trim())?;
    let tenant_id = session.tenant_ctx.tenant_id();

    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;

    let existing: Option<MemberDbRow> = sqlx::query_as(
        "SELECT m.id AS membership_id, m.tenant_id, m.user_id,
                u.email, u.name, u.is_active,
                m.workspace_id, w.slug AS workspace_slug, w.name AS workspace_name,
                m.role, m.created_at
         FROM memberships m
         JOIN users u ON u.id = m.user_id
         LEFT JOIN workspaces w ON w.tenant_id = m.tenant_id AND w.id = m.workspace_id
         WHERE m.tenant_id = $1 AND m.id = $2",
    )
    .bind(tenant_id)
    .bind(membership_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup membership to update: {err}")))?;

    let Some(mut row) = existing else {
        return Err(AppError::NotFound);
    };

    let current_role = Role::parse(&row.role)?;

    match row.workspace_id {
        None => {
            if !new_role.is_valid_tenant_role() {
                return Err(AppError::Validation(
                    "tenant-level role must be tenant_owner, tenant_admin, member, or viewer"
                        .to_owned(),
                ));
            }
            if !session.can_manage_tenant() {
                return Err(AppError::Forbidden(
                    "only tenant owners or tenant admins can modify tenant-level memberships",
                ));
            }
            if (current_role == Role::TenantOwner || new_role == Role::TenantOwner)
                && session.tenant_role != Some(Role::TenantOwner)
            {
                return Err(AppError::Forbidden(
                    "only a tenant_owner can modify or grant tenant_owner",
                ));
            }
            if current_role == Role::TenantOwner && new_role != Role::TenantOwner {
                let owner_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM memberships
                     WHERE tenant_id = $1 AND workspace_id IS NULL AND role = 'tenant_owner'",
                )
                .bind(tenant_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("count tenant owners: {err}")))?;
                if owner_count <= 1 {
                    return Err(AppError::Validation(
                        "cannot demote the last tenant_owner".to_owned(),
                    ));
                }
            }
        }
        Some(ws_id) => {
            if !new_role.is_valid_workspace_role() {
                return Err(AppError::Validation(
                    "workspace-level role must be workspace_admin, member, or viewer".to_owned(),
                ));
            }
            if !session.can_manage_workspace(ws_id) {
                return Err(AppError::Forbidden(
                    "insufficient permissions for target workspace",
                ));
            }
        }
    }

    sqlx::query("UPDATE memberships SET role = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(new_role.as_str())
        .bind(tenant_id)
        .bind(membership_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("update membership role: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit update_member_role: {err}")))?;

    info!(
        tenant_id,
        actor_user_id = session.user.id,
        membership_id,
        old_role = current_role.as_str(),
        new_role = new_role.as_str(),
        "membership role updated"
    );

    row.role = new_role.as_str().to_owned();
    TenantMemberView::try_from(row)
}

pub async fn delete_member(
    pool: &PgPool,
    session: &AuthSession,
    membership_id: i64,
) -> Result<(), AppError> {
    session.require_user_session()?;
    if membership_id <= 0 {
        return Err(AppError::Validation("invalid membership_id".to_owned()));
    }
    if !session.can_manage_members() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let tenant_id = session.tenant_ctx.tenant_id();
    let mut tx = begin_tenant_tx(pool, session.tenant_ctx).await?;

    let existing: Option<(i64, Option<i64>, String)> = sqlx::query_as(
        "SELECT user_id, workspace_id, role
         FROM memberships
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(membership_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup membership to delete: {err}")))?;

    let Some((target_user_id, workspace_id, raw_role)) = existing else {
        return Err(AppError::NotFound);
    };

    let current_role = Role::parse(&raw_role)?;

    match workspace_id {
        None => {
            if !session.can_manage_tenant() {
                return Err(AppError::Forbidden(
                    "only tenant owners or tenant admins can remove tenant-level memberships",
                ));
            }
            if current_role == Role::TenantOwner {
                if session.tenant_role != Some(Role::TenantOwner) {
                    return Err(AppError::Forbidden(
                        "only a tenant_owner can remove a tenant_owner",
                    ));
                }
                let owner_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM memberships
                     WHERE tenant_id = $1 AND workspace_id IS NULL AND role = 'tenant_owner'",
                )
                .bind(tenant_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("count tenant owners: {err}")))?;
                if owner_count <= 1 {
                    return Err(AppError::Validation(
                        "cannot remove the last tenant_owner".to_owned(),
                    ));
                }
            }
        }
        Some(ws_id) => {
            if !session.can_manage_workspace(ws_id) {
                return Err(AppError::Forbidden(
                    "insufficient permissions for target workspace",
                ));
            }
        }
    }

    sqlx::query("DELETE FROM memberships WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(membership_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("delete membership: {err}")))?;

    // If the user no longer has any membership in this tenant, revoke all active tenant sessions immediately.
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memberships WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(tenant_id)
    .bind(target_user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("count remaining memberships: {err}")))?;

    if remaining == 0 {
        sqlx::query("DELETE FROM sessions WHERE tenant_id = $1 AND user_id = $2")
            .bind(tenant_id)
            .bind(target_user_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("revoke removed user sessions: {err}"))
            })?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit delete_member: {err}")))?;

    info!(
        tenant_id,
        actor_user_id = session.user.id,
        membership_id,
        target_user_id,
        "membership removed"
    );

    Ok(())
}
