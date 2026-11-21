use chrono::{DateTime, Utc};
use maxi_loop_auth::model::Role;
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub_notify::{PiiScrubRuleEntry, PiiScrubRuleType};
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiiScrubRuleView {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: Option<i64>,
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub scope: String,
    pub name: String,
    pub rule_type: String,
    pub pattern: String,
    pub replacement: String,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PiiScrubRuleView {
    pub fn to_cache_entry(&self) -> Option<PiiScrubRuleEntry> {
        let rule_type = PiiScrubRuleType::parse(&self.rule_type)?;
        Some(PiiScrubRuleEntry {
            id: self.id,
            tenant_id: self.tenant_id,
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            rule_type,
            pattern: self.pattern.clone(),
            replacement: self.replacement.clone(),
            is_enabled: self.is_enabled,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreatePiiScrubRuleInput {
    pub name: String,
    pub rule_type: String,
    pub pattern: String,
    pub replacement: Option<String>,
    pub workspace_slug: Option<String>,
    pub project_slug: Option<String>,
    pub is_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdatePiiScrubRuleInput {
    pub name: Option<String>,
    pub replacement: Option<String>,
    pub is_enabled: Option<bool>,
}

#[derive(FromRow)]
struct PiiRuleJoinedRow {
    id: i64,
    tenant_id: i64,
    workspace_id: Option<i64>,
    workspace_slug: Option<String>,
    project_id: Option<i64>,
    project_slug: Option<String>,
    name: String,
    rule_type: String,
    pattern: String,
    replacement: String,
    is_enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn map_row(row: PiiRuleJoinedRow) -> PiiScrubRuleView {
    let scope = match (row.workspace_id, row.project_id) {
        (None, None) => "tenant",
        (Some(_), None) => "workspace",
        (_, Some(_)) => "project",
    }
    .to_owned();

    PiiScrubRuleView {
        id: row.id,
        tenant_id: row.tenant_id,
        workspace_id: row.workspace_id,
        workspace_slug: row.workspace_slug,
        project_id: row.project_id,
        project_slug: row.project_slug,
        scope,
        name: row.name,
        rule_type: row.rule_type,
        pattern: row.pattern,
        replacement: row.replacement,
        is_enabled: row.is_enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

pub async fn list_pii_scrub_rules(
    pool: &PgPool,
    tenant_ctx: TenantContext,
) -> Result<Vec<PiiScrubRuleView>, AppError> {
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let rows: Vec<PiiRuleJoinedRow> = sqlx::query_as(
        "SELECT r.id, r.tenant_id, r.workspace_id, w.slug AS workspace_slug,
                r.project_id, p.slug AS project_slug,
                r.name, r.rule_type, r.pattern, r.replacement, r.is_enabled,
                r.created_at, r.updated_at
         FROM pii_scrub_rules r
         LEFT JOIN workspaces w ON w.tenant_id = r.tenant_id AND w.id = r.workspace_id
         LEFT JOIN projects p ON p.tenant_id = r.tenant_id AND p.id = r.project_id
         WHERE r.tenant_id = $1
         ORDER BY r.workspace_id NULLS FIRST, r.project_id NULLS FIRST, r.id ASC
         LIMIT 500",
    )
    .bind(tenant_ctx.tenant_id())
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list pii_scrub_rules: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list pii_scrub_rules: {err}")))?;

    Ok(rows.into_iter().map(map_row).collect())
}

pub async fn create_pii_scrub_rule(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    input: CreatePiiScrubRuleInput,
) -> Result<PiiScrubRuleView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation(
            "rule name must be between 1 and 128 characters".to_owned(),
        ));
    }

    let rule_type = PiiScrubRuleType::parse(&input.rule_type).ok_or_else(|| {
        AppError::Validation(
            "rule_type must be 'field_name', 'header_name', or 'value_substring'".to_owned(),
        )
    })?;

    let pattern = input.pattern.trim();
    if pattern.len() < 2 || pattern.len() > 128 {
        return Err(AppError::Validation(
            "pattern must be between 2 and 128 characters".to_owned(),
        ));
    }

    let replacement = input
        .replacement
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("[Filtered]");
    if replacement.len() > 64 {
        return Err(AppError::Validation(
            "replacement must be between 1 and 64 characters".to_owned(),
        ));
    }

    let is_enabled = input.is_enabled.unwrap_or(true);
    let ws_slug = input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let proj_slug = input
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(ws) = ws_slug
        && !is_valid_slug(ws)
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }
    if let Some(p) = proj_slug
        && !is_valid_slug(p)
    {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;

    let (workspace_id, workspace_slug_resolved, project_id, project_slug_resolved) = match (
        ws_slug, proj_slug,
    ) {
        (None, None) => (None, None, None, None),
        (Some(ws), None) => {
            let ws_id: Option<i64> =
                sqlx::query_scalar("SELECT id FROM workspaces WHERE tenant_id = $1 AND slug = $2")
                    .bind(tenant_ctx.tenant_id())
                    .bind(ws)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|err| AppError::DatabaseQuery(format!("lookup workspace: {err}")))?;
            let Some(ws_id) = ws_id else {
                return Err(AppError::NotFound);
            };
            (Some(ws_id), Some(ws.to_owned()), None, None)
        }
        (ws_opt, Some(p_slug)) => {
            let row: Option<(i64, String, i64, String)> = match ws_opt {
                    Some(ws) => {
                        sqlx::query_as(
                            "SELECT w.id, w.slug, p.id, p.slug
                             FROM projects p
                             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                             WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
                        )
                        .bind(tenant_ctx.tenant_id())
                        .bind(ws)
                        .bind(p_slug)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|err| AppError::DatabaseQuery(format!("lookup project: {err}")))?
                    }
                    None => {
                        sqlx::query_as(
                            "SELECT w.id, w.slug, p.id, p.slug
                             FROM projects p
                             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                             WHERE p.tenant_id = $1 AND p.slug = $2
                             ORDER BY w.is_default DESC, p.id ASC
                             LIMIT 1",
                        )
                        .bind(tenant_ctx.tenant_id())
                        .bind(p_slug)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|err| AppError::DatabaseQuery(format!("lookup project: {err}")))?
                    }
                };
            let Some((ws_id, ws_s, p_id, p_s)) = row else {
                return Err(AppError::NotFound);
            };
            (Some(ws_id), Some(ws_s), Some(p_id), Some(p_s))
        }
    };

    let inserted: (i64, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO pii_scrub_rules (
             tenant_id, workspace_id, project_id, name, rule_type, pattern, replacement, is_enabled
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id, created_at, updated_at",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(workspace_id)
    .bind(project_id)
    .bind(name)
    .bind(rule_type.as_str())
    .bind(pattern)
    .bind(replacement)
    .bind(is_enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert pii_scrub_rule: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create pii_scrub_rule: {err}")))?;

    Ok(map_row(PiiRuleJoinedRow {
        id: inserted.0,
        tenant_id: tenant_ctx.tenant_id(),
        workspace_id,
        workspace_slug: workspace_slug_resolved,
        project_id,
        project_slug: project_slug_resolved,
        name: name.to_owned(),
        rule_type: rule_type.as_str().to_owned(),
        pattern: pattern.to_owned(),
        replacement: replacement.to_owned(),
        is_enabled,
        created_at: inserted.1,
        updated_at: inserted.2,
    }))
}

pub async fn update_pii_scrub_rule(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    rule_id: i64,
    input: UpdatePiiScrubRuleInput,
) -> Result<PiiScrubRuleView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if rule_id <= 0 {
        return Err(AppError::Validation("invalid rule_id".to_owned()));
    }

    let name_clean = match input.name.as_deref() {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 128 {
                return Err(AppError::Validation(
                    "rule name must be between 1 and 128 characters".to_owned(),
                ));
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    let replacement_clean = match input.replacement.as_deref() {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 64 {
                return Err(AppError::Validation(
                    "replacement must be between 1 and 64 characters".to_owned(),
                ));
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let updated: Option<i64> = sqlx::query_scalar(
        "UPDATE pii_scrub_rules
         SET name = COALESCE($3, name),
             replacement = COALESCE($4, replacement),
             is_enabled = COALESCE($5, is_enabled),
             updated_at = NOW()
         WHERE tenant_id = $1 AND id = $2
         RETURNING id",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(rule_id)
    .bind(name_clean)
    .bind(replacement_clean)
    .bind(input.is_enabled)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update pii_scrub_rule: {err}")))?;

    if updated.is_none() {
        return Err(AppError::NotFound);
    }

    let row: PiiRuleJoinedRow = sqlx::query_as(
        "SELECT r.id, r.tenant_id, r.workspace_id, w.slug AS workspace_slug,
                r.project_id, p.slug AS project_slug,
                r.name, r.rule_type, r.pattern, r.replacement, r.is_enabled,
                r.created_at, r.updated_at
         FROM pii_scrub_rules r
         LEFT JOIN workspaces w ON w.tenant_id = r.tenant_id AND w.id = r.workspace_id
         LEFT JOIN projects p ON p.tenant_id = r.tenant_id AND p.id = r.project_id
         WHERE r.tenant_id = $1 AND r.id = $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(rule_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load updated pii_scrub_rule: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit update pii_scrub_rule: {err}")))?;

    Ok(map_row(row))
}

pub async fn delete_pii_scrub_rule(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    rule_id: i64,
) -> Result<(), AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if rule_id <= 0 {
        return Err(AppError::Validation("invalid rule_id".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let res = sqlx::query("DELETE FROM pii_scrub_rules WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_ctx.tenant_id())
        .bind(rule_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("delete pii_scrub_rule: {err}")))?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit delete pii_scrub_rule: {err}")))?;

    Ok(())
}
