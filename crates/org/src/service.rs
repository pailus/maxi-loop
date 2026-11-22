use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use maxi_loop_auth::model::{Role, TenantSummary, WorkspaceSummary};
use maxi_loop_platform::config::{Config, is_valid_slug};
use maxi_loop_platform::dsn_notify::TenantRateLimitUpdate;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub_notify::PiiScrubRuleEntry;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::dsn::{format_ingest_endpoint, format_project_dsn, generate_dsn_public_key};
use crate::model::{
    CreateProjectInput, CreateWorkspaceInput, ProjectView, TenantOverview,
    UpdateProjectSettingsInput, UpdateTenantSettingsInput, UpdateWorkspaceSettingsInput,
    WorkspaceView,
};
use crate::pii::list_pii_scrub_rules;

const MAX_PROJECT_RPM: i32 = 1_000_000;
const MAX_TENANT_AGGREGATE_RPM: i32 = 10_000_000;

/// Service facade owned by `maxi-loop-org` so upstream layers (like `maxi-loop-mcp`) can invoke
/// tenant-scoped org services without importing `sqlx` directly (`D006`).
#[derive(Clone)]
pub struct OrgService {
    db: PgPool,
    config: Arc<Config>,
}

impl OrgService {
    pub fn new(db: PgPool, config: Arc<Config>) -> Self {
        Self { db, config }
    }

    pub async fn list_projects(
        &self,
        tenant_ctx: TenantContext,
        tenant_slug: &str,
        workspace_slug: Option<&str>,
    ) -> Result<Vec<ProjectView>, AppError> {
        list_projects(
            &self.db,
            &self.config,
            tenant_ctx,
            tenant_slug,
            workspace_slug,
        )
        .await
    }

    pub async fn list_effective_pii_rules(
        &self,
        tenant_ctx: TenantContext,
        workspace_id: Option<i64>,
        project_id: Option<i64>,
    ) -> Result<Vec<PiiScrubRuleEntry>, AppError> {
        let rules = list_pii_scrub_rules(&self.db, tenant_ctx).await?;
        Ok(rules
            .into_iter()
            .filter(|r| {
                r.is_enabled
                    && match (r.workspace_id, r.project_id) {
                        (None, None) => true,
                        (Some(ws_id), None) => workspace_id == Some(ws_id),
                        (Some(ws_id), Some(p_id)) => {
                            workspace_id == Some(ws_id) && project_id == Some(p_id)
                        }
                        (None, Some(p_id)) => project_id == Some(p_id),
                    }
            })
            .filter_map(|r| r.to_cache_entry())
            .collect())
    }
}

#[derive(FromRow)]
struct WorkspaceDbRow {
    id: i64,
    slug: String,
    name: String,
    retention_days_override: Option<i16>,
    default_rate_limit_per_minute_override: Option<i32>,
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
    ws_rate_limit_override: Option<i32>,
    slug: String,
    name: String,
    platform: String,
    tags: Vec<String>,
    dsn_public_key: String,
    retention_days_override: Option<i16>,
    rate_limit_per_minute_override: Option<i32>,
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
    tags: Vec<String>,
    dsn_public_key: String,
    retention_days_override: Option<i16>,
    rate_limit_per_minute_override: Option<i32>,
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

pub async fn update_tenant_settings(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
    input: UpdateTenantSettingsInput,
) -> Result<(TenantOverview, TenantRateLimitUpdate), AppError> {
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let current = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;

    let new_name = match input.name {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 128 {
                return Err(AppError::Validation(
                    "tenant name must be between 1 and 128 characters".to_owned(),
                ));
            }
            trimmed.to_owned()
        }
        None => current.name.clone(),
    };

    let new_retention = match input.retention_days {
        Some(days) => {
            if days < 1 || days > config.retention_default_days {
                return Err(AppError::Validation(format!(
                    "tenant retention_days must be between 1 and {} days",
                    config.retention_default_days
                )));
            }
            days
        }
        None => current.retention_days,
    };

    let new_default_rpm = match input.default_rate_limit_per_minute {
        Some(rpm) => {
            if !(1..=MAX_PROJECT_RPM).contains(&rpm) {
                return Err(AppError::Validation(format!(
                    "default_rate_limit_per_minute must be between 1 and {MAX_PROJECT_RPM}"
                )));
            }
            rpm
        }
        None => current.default_rate_limit_per_minute,
    };

    let new_agg_rpm = match input.aggregate_rate_limit_per_minute {
        Some(rpm) => {
            if !(1..=MAX_TENANT_AGGREGATE_RPM).contains(&rpm) {
                return Err(AppError::Validation(format!(
                    "aggregate_rate_limit_per_minute must be between 1 and {MAX_TENANT_AGGREGATE_RPM}"
                )));
            }
            rpm
        }
        None => current.aggregate_rate_limit_per_minute,
    };

    let retention_i16 = i16::try_from(new_retention)
        .map_err(|_| AppError::Validation("retention_days out of range".to_owned()))?;

    sqlx::query(
        "UPDATE tenants
         SET name = $1,
             retention_days = $2,
             default_rate_limit_per_minute = $3,
             aggregate_rate_limit_per_minute = $4,
             updated_at = NOW()
         WHERE id = $5",
    )
    .bind(&new_name)
    .bind(retention_i16)
    .bind(new_default_rpm)
    .bind(new_agg_rpm)
    .bind(current.id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update tenant settings: {err}")))?;

    // Collect effective rate limits for all projects in this tenant so DsnCache updates live.
    let proj_limits: Vec<(i64, i32)> = sqlx::query_as(
        "SELECT p.id,
                COALESCE(
                    p.rate_limit_per_minute_override,
                    w.default_rate_limit_per_minute_override,
                    $2
                ) AS effective_rpm
         FROM projects p
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE p.tenant_id = $1",
    )
    .bind(current.id)
    .bind(new_default_rpm)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| {
        AppError::DatabaseQuery(format!("load tenant project effective limits: {err}"))
    })?;

    let updated_tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let default_ws = load_default_workspace_in_tx(&mut tx, updated_tenant.id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit update_tenant_settings: {err}")))?;

    let cache_update = TenantRateLimitUpdate {
        tenant_id: updated_tenant.id,
        aggregate_rate_limit_per_minute: Some(u32::try_from(new_agg_rpm.max(1)).unwrap_or(6000)),
        project_effective_limits: proj_limits
            .into_iter()
            .map(|(pid, rpm)| (pid, u32::try_from(rpm.max(1)).unwrap_or(1200)))
            .collect(),
    };

    Ok((
        TenantOverview {
            tenant: updated_tenant,
            default_workspace: default_ws,
            single_tenant_mode: config.single_tenant_mode,
        },
        cache_update,
    ))
}

pub async fn list_workspaces(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
) -> Result<Vec<WorkspaceView>, AppError> {
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;

    let rows: Vec<WorkspaceDbRow> = sqlx::query_as(
        "SELECT id, slug, name, retention_days_override,
                default_rate_limit_per_minute_override, is_default, created_at, updated_at
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
    if let Some(rpm) = input.default_rate_limit_per_minute_override
        && !(1..=MAX_PROJECT_RPM).contains(&rpm)
    {
        return Err(AppError::Validation(format!(
            "default_rate_limit_per_minute_override must be between 1 and {MAX_PROJECT_RPM}"
        )));
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
        "INSERT INTO workspaces (
             tenant_id, slug, name, retention_days_override,
             default_rate_limit_per_minute_override, is_default
         )
         VALUES ($1, $2, $3, $4, $5, FALSE)
         RETURNING id, slug, name, retention_days_override,
                   default_rate_limit_per_minute_override, is_default, created_at, updated_at",
    )
    .bind(tenant.id)
    .bind(slug)
    .bind(name)
    .bind(retention_override_i16)
    .bind(input.default_rate_limit_per_minute_override)
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

pub async fn update_workspace_settings(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
    workspace_slug: &str,
    input: UpdateWorkspaceSettingsInput,
) -> Result<(WorkspaceView, TenantRateLimitUpdate), AppError> {
    if !is_valid_slug(workspace_slug) {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let current = load_workspace_by_slug_in_tx(&mut tx, &tenant, workspace_slug).await?;

    let new_name = match input.name {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 128 {
                return Err(AppError::Validation(
                    "workspace name must be between 1 and 128 characters".to_owned(),
                ));
            }
            trimmed.to_owned()
        }
        None => current.name.clone(),
    };

    let max_retention = config.retention_default_days.min(tenant.retention_days);
    let new_retention_override = match input.retention_days_override {
        Some(opt) => {
            validate_retention_override(opt, max_retention, "workspace retention_days_override")?
        }
        None => current
            .retention_days_override
            .and_then(|v| i16::try_from(v).ok()),
    };

    let new_rate_override = match input.default_rate_limit_per_minute_override {
        Some(Some(rpm)) => {
            if !(1..=MAX_PROJECT_RPM).contains(&rpm) {
                return Err(AppError::Validation(format!(
                    "default_rate_limit_per_minute_override must be between 1 and {MAX_PROJECT_RPM}"
                )));
            }
            Some(rpm)
        }
        Some(None) => None,
        None => current.default_rate_limit_per_minute_override,
    };

    let updated_row: WorkspaceDbRow = sqlx::query_as(
        "UPDATE workspaces
         SET name = $1,
             retention_days_override = $2,
             default_rate_limit_per_minute_override = $3,
             updated_at = NOW()
         WHERE tenant_id = $4 AND id = $5
         RETURNING id, slug, name, retention_days_override,
                   default_rate_limit_per_minute_override, is_default, created_at, updated_at",
    )
    .bind(&new_name)
    .bind(new_retention_override)
    .bind(new_rate_override)
    .bind(tenant.id)
    .bind(current.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update workspace settings: {err}")))?;

    let ws_effective_rpm = new_rate_override.unwrap_or(tenant.default_rate_limit_per_minute);
    let proj_limits: Vec<(i64, i32)> = sqlx::query_as(
        "SELECT id, COALESCE(rate_limit_per_minute_override, $3) AS effective_rpm
         FROM projects
         WHERE tenant_id = $1 AND workspace_id = $2",
    )
    .bind(tenant.id)
    .bind(current.id)
    .bind(ws_effective_rpm)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| {
        AppError::DatabaseQuery(format!("load workspace project effective limits: {err}"))
    })?;

    tx.commit().await.map_err(|err| {
        AppError::DatabaseQuery(format!("commit update_workspace_settings: {err}"))
    })?;

    let view = map_workspace_row(&tenant, updated_row)?;
    let cache_update = TenantRateLimitUpdate {
        tenant_id: tenant.id,
        aggregate_rate_limit_per_minute: None,
        project_effective_limits: proj_limits
            .into_iter()
            .map(|(pid, rpm)| (pid, u32::try_from(rpm.max(1)).unwrap_or(1200)))
            .collect(),
    };

    Ok((view, cache_update))
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
                        w.default_rate_limit_per_minute_override AS ws_rate_limit_override,
                        p.slug, p.name, p.platform, p.tags, p.dsn_public_key,
                        p.retention_days_override, p.rate_limit_per_minute_override,
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
                    w.default_rate_limit_per_minute_override AS ws_rate_limit_override,
                    p.slug, p.name, p.platform, p.tags, p.dsn_public_key,
                    p.retention_days_override, p.rate_limit_per_minute_override,
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
                w.default_rate_limit_per_minute_override AS ws_rate_limit_override,
                p.slug, p.name, p.platform, p.tags, p.dsn_public_key,
                p.retention_days_override, p.rate_limit_per_minute_override,
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
    let platform = parse_platform(input.platform.as_deref())?;
    let tags = normalize_project_tags(input.tags.as_deref())?;

    if let Some(rpm) = input.rate_limit_per_minute
        && !(1..=MAX_PROJECT_RPM).contains(&rpm)
    {
        return Err(AppError::Validation(format!(
            "rate_limit_per_minute must be between 1 and {MAX_PROJECT_RPM}"
        )));
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

    let effective_rpm = input
        .rate_limit_per_minute
        .unwrap_or(ws.effective_rate_limit_per_minute);

    let dsn_public_key = generate_dsn_public_key();
    let ws_retention_i16 = ws
        .retention_days_override
        .and_then(|v| i16::try_from(v).ok());

    let insert_res: Result<ProjectMutatedDbRow, sqlx::Error> = sqlx::query_as(
        "INSERT INTO projects (
             tenant_id, workspace_id, slug, name, platform, tags,
             dsn_public_key, retention_days_override,
             rate_limit_per_minute, rate_limit_per_minute_override
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         RETURNING id, workspace_id, slug, name, platform, tags,
                   dsn_public_key, retention_days_override, rate_limit_per_minute_override,
                   created_at, updated_at",
    )
    .bind(tenant.id)
    .bind(ws.id)
    .bind(slug)
    .bind(name)
    .bind(&platform)
    .bind(&tags)
    .bind(&dsn_public_key)
    .bind(retention_override_i16)
    .bind(effective_rpm)
    .bind(input.rate_limit_per_minute)
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
            ws_rate_limit_override: ws.default_rate_limit_per_minute_override,
            slug: inserted.slug,
            name: inserted.name,
            platform: inserted.platform,
            tags: inserted.tags,
            dsn_public_key: inserted.dsn_public_key,
            retention_days_override: inserted.retention_days_override,
            rate_limit_per_minute_override: inserted.rate_limit_per_minute_override,
            created_at: inserted.created_at,
            updated_at: inserted.updated_at,
        },
    )
}

pub async fn update_project_settings(
    pool: &PgPool,
    config: &Config,
    tenant_ctx: TenantContext,
    tenant_slug: &str,
    workspace_slug: &str,
    project_slug: &str,
    input: UpdateProjectSettingsInput,
) -> Result<ProjectView, AppError> {
    if !is_valid_slug(workspace_slug) || !is_valid_slug(project_slug) {
        return Err(AppError::Validation(
            "invalid workspace_slug or project_slug".to_owned(),
        ));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let tenant = verify_tenant_in_tx(&mut tx, tenant_ctx, tenant_slug).await?;
    let ws = load_workspace_by_slug_in_tx(&mut tx, &tenant, workspace_slug).await?;

    let existing: Option<ProjectMutatedDbRow> = sqlx::query_as(
        "SELECT id, workspace_id, slug, name, platform, tags,
                dsn_public_key, retention_days_override, rate_limit_per_minute_override,
                created_at, updated_at
         FROM projects
         WHERE tenant_id = $1 AND workspace_id = $2 AND slug = $3",
    )
    .bind(tenant.id)
    .bind(ws.id)
    .bind(project_slug)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup project for settings: {err}")))?;

    let Some(existing) = existing else {
        return Err(AppError::NotFound);
    };

    let new_name = match input.name {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 128 {
                return Err(AppError::Validation(
                    "project name must be between 1 and 128 characters".to_owned(),
                ));
            }
            trimmed.to_owned()
        }
        None => existing.name.clone(),
    };

    let new_platform = match input.platform {
        Some(p) => parse_platform(Some(&p))?,
        None => existing.platform.clone(),
    };

    let new_tags = match input.tags {
        Some(raw_tags) => normalize_project_tags(Some(&raw_tags))?,
        None => existing.tags.clone(),
    };

    let max_retention = config
        .retention_default_days
        .min(tenant.retention_days)
        .min(ws.effective_retention_days);
    let new_retention_override = match input.retention_days_override {
        Some(opt) => {
            validate_retention_override(opt, max_retention, "project retention_days_override")?
        }
        None => existing.retention_days_override,
    };

    let new_rate_override = match input.rate_limit_per_minute_override {
        Some(Some(rpm)) => {
            if !(1..=MAX_PROJECT_RPM).contains(&rpm) {
                return Err(AppError::Validation(format!(
                    "rate_limit_per_minute_override must be between 1 and {MAX_PROJECT_RPM}"
                )));
            }
            Some(rpm)
        }
        Some(None) => None,
        None => existing.rate_limit_per_minute_override,
    };

    let effective_rpm = new_rate_override.unwrap_or(ws.effective_rate_limit_per_minute);

    let updated: ProjectMutatedDbRow = sqlx::query_as(
        "UPDATE projects
         SET name = $1,
             platform = $2,
             tags = $3,
             retention_days_override = $4,
             rate_limit_per_minute_override = $5,
             rate_limit_per_minute = $6,
             updated_at = NOW()
         WHERE tenant_id = $7 AND id = $8
         RETURNING id, workspace_id, slug, name, platform, tags,
                   dsn_public_key, retention_days_override, rate_limit_per_minute_override,
                   created_at, updated_at",
    )
    .bind(&new_name)
    .bind(&new_platform)
    .bind(&new_tags)
    .bind(new_retention_override)
    .bind(new_rate_override)
    .bind(effective_rpm)
    .bind(tenant.id)
    .bind(existing.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update project settings: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit update_project_settings: {err}")))?;

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
            ws_rate_limit_override: ws.default_rate_limit_per_minute_override,
            slug: updated.slug,
            name: updated.name,
            platform: updated.platform,
            tags: updated.tags,
            dsn_public_key: updated.dsn_public_key,
            retention_days_override: updated.retention_days_override,
            rate_limit_per_minute_override: updated.rate_limit_per_minute_override,
            created_at: updated.created_at,
            updated_at: updated.updated_at,
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
         RETURNING id, workspace_id, slug, name, platform, tags,
                   dsn_public_key, retention_days_override, rate_limit_per_minute_override,
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
            ws_rate_limit_override: ws.default_rate_limit_per_minute_override,
            slug: updated.slug,
            name: updated.name,
            platform: updated.platform,
            tags: updated.tags,
            dsn_public_key: updated.dsn_public_key,
            retention_days_override: updated.retention_days_override,
            rate_limit_per_minute_override: updated.rate_limit_per_minute_override,
            created_at: updated.created_at,
            updated_at: updated.updated_at,
        },
    )
}

fn parse_platform(raw: Option<&str>) -> Result<String, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
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
            Ok(p.to_ascii_lowercase())
        }
        None => Ok("other".to_owned()),
    }
}

/// Validate and normalize project grouping tags (max 10 tags, each 1..=32 lowercase chars
/// from `[a-z0-9-_/.]`, deduplicated and sorted).
pub fn normalize_project_tags(raw: Option<&[String]>) -> Result<Vec<String>, AppError> {
    let Some(items) = raw else {
        return Ok(Vec::new());
    };
    let mut set = BTreeSet::new();
    for item in items {
        let trimmed = item.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.len() > 32
            || trimmed.starts_with('/')
            || trimmed.ends_with('/')
            || trimmed.contains("..")
            || !trimmed.bytes().all(|b| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'-' | b'_' | b'/' | b'.')
            })
        {
            return Err(AppError::Validation(
                "each project tag must be 1..=32 lowercase alphanumeric chars (plus '-', '_', '.', '/')"
                    .to_owned(),
            ));
        }
        set.insert(trimmed);
        if set.len() > 10 {
            return Err(AppError::Validation(
                "a project may have at most 10 tags".to_owned(),
            ));
        }
    }
    Ok(set.into_iter().collect())
}

#[derive(FromRow)]
struct TenantDbRow {
    id: i64,
    slug: String,
    name: String,
    retention_days: i16,
    default_rate_limit_per_minute: i32,
    aggregate_rate_limit_per_minute: i32,
    is_default: bool,
}

#[derive(FromRow)]
struct WorkspaceSummaryDbRow {
    id: i64,
    slug: String,
    name: String,
    retention_days_override: Option<i16>,
    default_rate_limit_per_minute_override: Option<i32>,
    is_default: bool,
}

async fn verify_tenant_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_ctx: TenantContext,
    expected_slug: &str,
) -> Result<TenantSummary, AppError> {
    if !is_valid_slug(expected_slug) {
        return Err(AppError::Validation("invalid tenant_slug".to_owned()));
    }

    let row: Option<TenantDbRow> = sqlx::query_as(
        "SELECT id, slug, name, retention_days,
                default_rate_limit_per_minute, aggregate_rate_limit_per_minute, is_default
         FROM tenants
         WHERE id = $1 AND slug = $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(expected_slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("verify tenant slug: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::NotFound);
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

async fn load_default_workspace_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
) -> Result<WorkspaceSummary, AppError> {
    let row: Option<WorkspaceSummaryDbRow> = sqlx::query_as(
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
    .map_err(|err| AppError::DatabaseQuery(format!("load default workspace: {err}")))?;

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

async fn load_workspace_by_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &TenantSummary,
    workspace_slug: &str,
) -> Result<WorkspaceView, AppError> {
    if !is_valid_slug(workspace_slug) {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let row: Option<WorkspaceDbRow> = sqlx::query_as(
        "SELECT id, slug, name, retention_days_override,
                default_rate_limit_per_minute_override, is_default, created_at, updated_at
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
    let effective_rate_limit_per_minute = row
        .default_rate_limit_per_minute_override
        .unwrap_or(tenant.default_rate_limit_per_minute);

    Ok(WorkspaceView {
        id: row.id,
        tenant_id: tenant.id,
        tenant_slug: tenant.slug.clone(),
        slug: row.slug,
        name: row.name,
        retention_days_override,
        effective_retention_days,
        default_rate_limit_per_minute_override: row.default_rate_limit_per_minute_override,
        effective_rate_limit_per_minute,
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

    let ws_effective_rpm = row
        .ws_rate_limit_override
        .unwrap_or(tenant.default_rate_limit_per_minute);
    let effective_rate_limit_per_minute = row
        .rate_limit_per_minute_override
        .unwrap_or(ws_effective_rpm);

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
        tags: row.tags,
        dsn_public_key: row.dsn_public_key,
        dsn,
        ingest_endpoint,
        retention_days_override,
        effective_retention_days,
        rate_limit_per_minute_override: row.rate_limit_per_minute_override,
        rate_limit_per_minute: effective_rate_limit_per_minute,
        effective_rate_limit_per_minute,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_dedupes_and_validates_project_tags() {
        let tags = normalize_project_tags(Some(&[
            " Backend ".to_owned(),
            "e2e".to_owned(),
            "backend".to_owned(),
            "team/core".to_owned(),
            "".to_owned(),
        ]))
        .expect("valid tags");
        assert_eq!(tags, vec!["backend", "e2e", "team/core"]);

        assert!(normalize_project_tags(Some(&["bad tag!".to_owned()])).is_err());
        assert!(normalize_project_tags(Some(&["../escape".to_owned()])).is_err());
        let too_many: Vec<String> = (0..11).map(|i| format!("tag-{i}")).collect();
        assert!(normalize_project_tags(Some(&too_many)).is_err());
    }
}
