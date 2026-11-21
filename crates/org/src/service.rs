use chrono::{DateTime, Utc};
use maxi_loop_auth::model::{Role, TenantSummary, WorkspaceSummary};
use maxi_loop_platform::config::{Config, is_valid_slug};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::dsn::{format_ingest_endpoint, format_project_dsn, generate_dsn_public_key};
use crate::model::{
    CreateProjectInput, CreateWorkspaceInput, ProjectView, TenantOverview, WorkspaceView,
};

#[derive(FromRow)]
struct WorkspaceDbRow {
    id: i64,
    slug: String,
    name: String,
    retention_days_override: Option<i16>,
    is_default: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct ProjectJoinedDbRow {
    id: i64,
    workspace_id: i64,
    workspace_slug: String,
    ws_retention_days_override: Option<i16>,
    slug: String,
    name: String,
    platform: String,
    dsn_public_key: String,
    retention_days_override: Option<i16>,
    rate_limit_per_minute: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct ProjectMutatedDbRow {
    id: i64,
    workspace_id: i64,
    slug: String,
    name: String,
    platform: String,
    dsn_public_key: String,
    retention_days_override: Option<i16>,
    rate_limit_per_minute: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

pub async fn get_tenant_overview(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
) -> Result<TenantOverview, AppError> {
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let default_ws = load_default_workspace_in_tx(&mut tx, tenant.id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit get_tenant_overview: {err}")))?;

    Ok(TenantOverview {
        tenant,
        default_workspace: default_ws,
        single_tenant_mode: config.single_tenant_mode,
    })
}

pub async fn list_workspaces(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
) -> Result<Vec<WorkspaceView>, AppError> {
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;

    let rows: Vec<WorkspaceDbRow> = sqlx::query_as(
        "SELECT id, slug, name, retention_days_override, is_default, created_at, updated_at
         FROM workspaces
         WHERE tenant_id = $1
         ORDER BY is_default DESC, slug ASC
         LIMIT 200",
    )
    .bind(tenant.id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list workspaces: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_workspaces: {err}")))?;

    rows.into_iter()
        .map(|row| map_workspace_row(&tenant, row))
        .collect()
}

pub async fn get_workspace(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
    workspace_slug: &str,
) -> Result<WorkspaceView, AppError> {
    if !is_valid_slug(workspace_slug) {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let ws = load_workspace_by_slug_in_tx(&mut tx, &tenant, workspace_slug).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit get_workspace: {err}")))?;
    Ok(ws)
}

pub async fn create_workspace(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    role: Role,
    tenant_slug: &str,
    input: CreateWorkspaceInput,
) -> Result<WorkspaceView, AppError> {
    if !role.can_manage_workspaces() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let slug = input.slug.trim();
    if !is_valid_slug(slug) {
        return Err(AppError::Validation(
            "workspace slug must be lowercase alphanumeric hyphen-separated (max 64 chars)"
                .to_owned(),
        ));
    }
    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation(
            "workspace name must be between 1 and 128 characters".to_owned(),
        ));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;

    let max_retention = config.retention_default_days.min(tenant.retention_days);
    let retention_override_i16 = validate_retention_override(
        input.retention_days_override,
        max_retention,
        "workspace retention_days_override",
    )?;

    let insert_res: Result<WorkspaceDbRow, sqlx::Error> = sqlx::query_as(
        "INSERT INTO workspaces (tenant_id, slug, name, retention_days_override, is_default)
         VALUES ($1, $2, $3, $4, FALSE)
         RETURNING id, slug, name, retention_days_override, is_default, created_at, updated_at",
    )
    .bind(tenant.id)
    .bind(slug)
    .bind(name)
    .bind(retention_override_i16)
    .fetch_one(&mut *tx)
    .await;

    let row = match insert_res {
        Ok(r) => r,
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
            return Err(AppError::Conflict("workspace slug already exists"));
        }
        Err(err) => return Err(AppError::DatabaseQuery(format!("insert workspace: {err}"))),
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create_workspace: {err}")))?;

    map_workspace_row(&tenant, row)
}

pub async fn list_projects(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
    workspace_slug: Option<&str>,
) -> Result<Vec<ProjectView>, AppError> {
    if let Some(ws_slug) = workspace_slug
        && !is_valid_slug(ws_slug)
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;

    let rows: Vec<ProjectJoinedDbRow> = match workspace_slug {
        Some(ws_slug) => {
            let ws = load_workspace_by_slug_in_tx(&mut tx, &tenant, ws_slug).await?;
            sqlx::query_as(
                "SELECT p.id, p.workspace_id,
                        w.slug AS workspace_slug,
                        w.retention_days_override AS ws_retention_days_override,
                        p.slug, p.name, p.platform, p.dsn_public_key,
                        p.retention_days_override, p.rate_limit_per_minute,
                        p.created_at, p.updated_at
                 FROM projects p
                 JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                 WHERE p.tenant_id = $1 AND p.workspace_id = $2
                 ORDER BY p.slug ASC
                 LIMIT 500",
            )
            .bind(tenant.id)
            .bind(ws.id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("list workspace projects: {err}")))?
        }
        None => sqlx::query_as(
            "SELECT p.id, p.workspace_id,
                    w.slug AS workspace_slug,
                    w.retention_days_override AS ws_retention_days_override,
                    p.slug, p.name, p.platform, p.dsn_public_key,
                    p.retention_days_override, p.rate_limit_per_minute,
                    p.created_at, p.updated_at
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             WHERE p.tenant_id = $1
             ORDER BY w.is_default DESC, p.slug ASC
             LIMIT 500",
        )
        .bind(tenant.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list tenant projects: {err}")))?,
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_projects: {err}")))?;

    rows.into_iter()
        .map(|row| map_project_row(config, &tenant, row))
        .collect()
}

pub async fn get_project(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
    workspace_slug: &str,
    project_slug: &str,
) -> Result<ProjectView, AppError> {
    if !is_valid_slug(workspace_slug) || !is_valid_slug(project_slug) {
        return Err(AppError::Validation(
            "invalid workspace_slug or project_slug".to_owned(),
        ));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let ws = load_workspace_by_slug_in_tx(&mut tx, &tenant, workspace_slug).await?;

    let row: Option<ProjectJoinedDbRow> = sqlx::query_as(
        "SELECT p.id, p.workspace_id,
                w.slug AS workspace_slug,
                w.retention_days_override AS ws_retention_days_override,
                p.slug, p.name, p.platform, p.dsn_public_key,
                p.retention_days_override, p.rate_limit_per_minute,
                p.created_at, p.updated_at
         FROM projects p
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE p.tenant_id = $1 AND p.workspace_id = $2 AND p.slug = $3",
    )
    .bind(tenant.id)
    .bind(ws.id)
    .bind(project_slug)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("get project: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit get_project: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::NotFound);
    };

    map_project_row(config, &tenant, row)
}

pub async fn create_project(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    role: Role,
    tenant_slug: &str,
    workspace_slug: &str,
    input: CreateProjectInput,
) -> Result<ProjectView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let slug = input.slug.trim();
    if !is_valid_slug(slug) {
        return Err(AppError::Validation(
            "project slug must be lowercase alphanumeric hyphen-separated (max 64 chars)"
                .to_owned(),
        ));
    }
    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation(
            "project name must be between 1 and 128 characters".to_owned(),
        ));
    }
    let platform = match input
        .platform
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(p) => {
            if p.len() > 64
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(AppError::Validation(
                    "invalid platform identifier".to_owned(),
                ));
            }
            p.to_ascii_lowercase()
        }
        None => "other".to_owned(),
    };

    let rate_limit_per_minute = input.rate_limit_per_minute.unwrap_or(1200);
    if !(1..=1_000_000).contains(&rate_limit_per_minute) {
        return Err(AppError::Validation(
            "rate_limit_per_minute must be between 1 and 1000000".to_owned(),
        ));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let ws = load_workspace_by_slug_in_tx(&mut tx, &tenant, workspace_slug).await?;

    let max_retention = config
        .retention_default_days
        .min(tenant.retention_days)
        .min(ws.effective_retention_days);
    let retention_override_i16 = validate_retention_override(
        input.retention_days_override,
        max_retention,
        "project retention_days_override",
    )?;

    let dsn_public_key = generate_dsn_public_key();
    let ws_retention_i16 = ws
        .retention_days_override
        .and_then(|v| i16::try_from(v).ok());

    let insert_res: Result<ProjectMutatedDbRow, sqlx::Error> = sqlx::query_as(
        "INSERT INTO projects (
             tenant_id, workspace_id, slug, name, platform,
             dsn_public_key, retention_days_override, rate_limit_per_minute
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id, workspace_id, slug, name, platform,
                   dsn_public_key, retention_days_override, rate_limit_per_minute,
                   created_at, updated_at",
    )
    .bind(tenant.id)
    .bind(ws.id)
    .bind(slug)
    .bind(name)
    .bind(&platform)
    .bind(&dsn_public_key)
    .bind(retention_override_i16)
    .bind(rate_limit_per_minute)
    .fetch_one(&mut *tx)
    .await;

    let inserted = match insert_res {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
            return Err(AppError::Conflict("project slug already exists"));
        }
        Err(err) => return Err(AppError::DatabaseQuery(format!("insert project: {err}"))),
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create_project: {err}")))?;

    map_project_row(
        config,
        &tenant,
        ProjectJoinedDbRow {
            id: inserted.id,
            workspace_id: inserted.workspace_id,
            workspace_slug: ws.slug,
            ws_retention_days_override: ws_retention_i16,
            slug: inserted.slug,
            name: inserted.name,
            platform: inserted.platform,
            dsn_public_key: inserted.dsn_public_key,
            retention_days_override: inserted.retention_days_override,
            rate_limit_per_minute: inserted.rate_limit_per_minute,
            created_at: inserted.created_at,
            updated_at: inserted.updated_at,
        },
    )
}

pub async fn rotate_project_dsn(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    role: Role,
    tenant_slug: &str,
    workspace_slug: &str,
    project_slug: &str,
) -> Result<ProjectView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if !is_valid_slug(workspace_slug) || !is_valid_slug(project_slug) {
        return Err(AppError::Validation(
            "invalid workspace_slug or project_slug".to_owned(),
        ));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let ws = load_workspace_by_slug_in_tx(&mut tx, &tenant, workspace_slug).await?;

    let new_key = generate_dsn_public_key();
    let updated: Option<ProjectMutatedDbRow> = sqlx::query_as(
        "UPDATE projects
         SET dsn_public_key = $1,
             updated_at = NOW()
         WHERE tenant_id = $2 AND workspace_id = $3 AND slug = $4
         RETURNING id, workspace_id, slug, name, platform,
                   dsn_public_key, retention_days_override, rate_limit_per_minute,
                   created_at, updated_at",
    )
    .bind(&new_key)
    .bind(tenant.id)
    .bind(ws.id)
    .bind(project_slug)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("rotate project dsn: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit rotate_project_dsn: {err}")))?;

    let Some(updated) = updated else {
        return Err(AppError::NotFound);
    };

    let ws_retention_i16 = ws
        .retention_days_override
        .and_then(|v| i16::try_from(v).ok());

    map_project_row(
        config,
        &tenant,
        ProjectJoinedDbRow {
            id: updated.id,
            workspace_id: updated.workspace_id,
            workspace_slug: ws.slug,
            ws_retention_days_override: ws_retention_i16,
            slug: updated.slug,
            name: updated.name,
            platform: updated.platform,
            dsn_public_key: updated.dsn_public_key,
            retention_days_override: updated.retention_days_override,
            rate_limit_per_minute: updated.rate_limit_per_minute,
            created_at: updated.created_at,
            updated_at: updated.updated_at,
        },
    )
}

async fn verify_tenant_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_ctx: TenantContext,
    expected_slug: &str,
) -> Result<TenantSummary, AppError> {
    if !is_valid_slug(expected_slug) {
        return Err(AppError::Validation("invalid tenant_slug".to_owned()));
    }

    let row: Option<(i64, String, String, i16, bool)> = sqlx::query_as(
        "SELECT id, slug, name, retention_days, is_default
         FROM tenants
         WHERE id = $1 AND slug = $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(expected_slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("verify tenant slug: {err}")))?;

    let Some((id, slug, name, retention_i16, is_default)) = row else {
        return Err(AppError::NotFound);
    };
    let retention_days = u16::try_from(retention_i16)
        .map_err(|_| AppError::Validation("invalid tenant retention_days".to_owned()))?;

    Ok(TenantSummary {
        id,
        slug,
        name,
        retention_days,
        is_default,
    })
}

async fn load_default_workspace_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
) -> Result<WorkspaceSummary, AppError> {
    let row: Option<(i64, String, String, Option<i16>, bool)> = sqlx::query_as(
        "SELECT id, slug, name, retention_days_override, is_default
         FROM workspaces
         WHERE tenant_id = $1
         ORDER BY is_default DESC, id ASC
         LIMIT 1",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load default workspace: {err}")))?;

    let Some((id, slug, name, retention_override_i16, is_default)) = row else {
        return Err(AppError::NotFound);
    };
    let retention_days_override = match retention_override_i16 {
        Some(v) => Some(
            u16::try_from(v)
                .map_err(|_| AppError::Validation("invalid workspace retention".to_owned()))?,
        ),
        None => None,
    };

    Ok(WorkspaceSummary {
        id,
        slug,
        name,
        retention_days_override,
        is_default,
    })
}

async fn load_workspace_by_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &TenantSummary,
    workspace_slug: &str,
) -> Result<WorkspaceView, AppError> {
    if !is_valid_slug(workspace_slug) {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let row: Option<WorkspaceDbRow> = sqlx::query_as(
        "SELECT id, slug, name, retention_days_override, is_default, created_at, updated_at
         FROM workspaces
         WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant.id)
    .bind(workspace_slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load workspace by slug: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::NotFound);
    };

    map_workspace_row(tenant, row)
}

fn validate_retention_override(
    override_days: Option<u16>,
    max_allowed: u16,
    field_name: &str,
) -> Result<Option<i16>, AppError> {
    match override_days {
        None => Ok(None),
        Some(days) => {
            if days < 1 || days > max_allowed {
                return Err(AppError::Validation(format!(
                    "{field_name} must be between 1 and {max_allowed} days"
                )));
            }
            let val = i16::try_from(days)
                .map_err(|_| AppError::Validation(format!("{field_name} out of range")))?;
            Ok(Some(val))
        }
    }
}

fn map_workspace_row(
    tenant: &TenantSummary,
    row: WorkspaceDbRow,
) -> Result<WorkspaceView, AppError> {
    let retention_days_override = match row.retention_days_override {
        Some(v) => Some(
            u16::try_from(v)
                .map_err(|_| AppError::Validation("invalid workspace retention".to_owned()))?,
        ),
        None => None,
    };
    let effective_retention_days = retention_days_override
        .unwrap_or(tenant.retention_days)
        .min(tenant.retention_days);

    Ok(WorkspaceView {
        id: row.id,
        tenant_id: tenant.id,
        tenant_slug: tenant.slug.clone(),
        slug: row.slug,
        name: row.name,
        retention_days_override,
        effective_retention_days,
        is_default: row.is_default,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn map_project_row(
    config: &Config,
    tenant: &TenantSummary,
    row: ProjectJoinedDbRow,
) -> Result<ProjectView, AppError> {
    let ws_retention = match row.ws_retention_days_override {
        Some(v) => Some(
            u16::try_from(v)
                .map_err(|_| AppError::Validation("invalid workspace retention".to_owned()))?,
        ),
        None => None,
    };
    let retention_days_override = match row.retention_days_override {
        Some(v) => Some(
            u16::try_from(v)
                .map_err(|_| AppError::Validation("invalid project retention".to_owned()))?,
        ),
        None => None,
    };

    let ws_effective = ws_retention
        .unwrap_or(tenant.retention_days)
        .min(tenant.retention_days);
    let effective_retention_days = retention_days_override
        .unwrap_or(ws_effective)
        .min(ws_effective)
        .min(config.retention_default_days);

    let dsn = format_project_dsn(&config.public_base_url, &row.dsn_public_key, row.id);
    let ingest_endpoint = format_ingest_endpoint(&config.public_base_url, row.id);

    Ok(ProjectView {
        id: row.id,
        tenant_id: tenant.id,
        tenant_slug: tenant.slug.clone(),
        workspace_id: row.workspace_id,
        workspace_slug: row.workspace_slug,
        slug: row.slug,
        name: row.name,
        platform: row.platform,
        dsn_public_key: row.dsn_public_key,
        dsn,
        ingest_endpoint,
        retention_days_override,
        effective_retention_days,
        rate_limit_per_minute: row.rate_limit_per_minute,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}
