use chrono::{DateTime, Utc};
use maxi_loop_auth::model::Role;
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use serde_json::Value;
use sqlx::{FromRow, PgPool};

use crate::delivery::{redact_channel_config_for_api, validate_channel_config};
use crate::model::{
    AlertChannelType, AlertChannelView, AlertDeliveryView, AlertRuleView, AlertTriggerType,
    CreateAlertChannelInput, CreateAlertRuleInput, SpikeMetric, UpdateAlertChannelInput,
    UpdateAlertRuleInput,
};

#[derive(FromRow)]
struct ChannelRow {
    id: i64,
    tenant_id: i64,
    workspace_id: Option<i64>,
    workspace_slug: Option<String>,
    project_id: Option<i64>,
    project_slug: Option<String>,
    name: String,
    channel_type: String,
    config: Value,
    is_enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn map_channel(row: ChannelRow) -> AlertChannelView {
    let scope = match (row.workspace_id, row.project_id) {
        (None, None) => "tenant",
        (Some(_), None) => "workspace",
        (_, Some(_)) => "project",
    }
    .to_owned();
    let config_summary = redact_channel_config_for_api(&row.channel_type, &row.config);
    AlertChannelView {
        id: row.id,
        tenant_id: row.tenant_id,
        workspace_id: row.workspace_id,
        workspace_slug: row.workspace_slug,
        project_id: row.project_id,
        project_slug: row.project_slug,
        scope,
        name: row.name,
        channel_type: row.channel_type,
        config_summary,
        is_enabled: row.is_enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[derive(FromRow)]
struct RuleRow {
    id: i64,
    tenant_id: i64,
    workspace_id: Option<i64>,
    workspace_slug: Option<String>,
    project_id: Option<i64>,
    project_slug: Option<String>,
    channel_id: i64,
    channel_name: String,
    channel_type: String,
    name: String,
    trigger_type: String,
    environment_filter: Option<String>,
    spike_threshold: Option<i32>,
    spike_window_minutes: Option<i32>,
    spike_metric: String,
    cooldown_minutes: i32,
    is_enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn map_rule(row: RuleRow) -> AlertRuleView {
    let scope = match (row.workspace_id, row.project_id) {
        (None, None) => "tenant",
        (Some(_), None) => "workspace",
        (_, Some(_)) => "project",
    }
    .to_owned();
    AlertRuleView {
        id: row.id,
        tenant_id: row.tenant_id,
        workspace_id: row.workspace_id,
        workspace_slug: row.workspace_slug,
        project_id: row.project_id,
        project_slug: row.project_slug,
        scope,
        channel_id: row.channel_id,
        channel_name: row.channel_name,
        channel_type: row.channel_type,
        name: row.name,
        trigger_type: row.trigger_type,
        environment_filter: row.environment_filter,
        spike_threshold: row.spike_threshold,
        spike_window_minutes: row.spike_window_minutes,
        spike_metric: row.spike_metric,
        cooldown_minutes: row.cooldown_minutes,
        is_enabled: row.is_enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[derive(FromRow)]
struct DeliveryRow {
    id: i64,
    tenant_id: i64,
    project_id: i64,
    project_slug: String,
    rule_id: i64,
    rule_name: String,
    channel_id: i64,
    channel_name: String,
    issue_id: i64,
    issue_title: String,
    trigger_type: String,
    channel_type: String,
    status: String,
    http_status: Option<i32>,
    error_message: Option<String>,
    created_at: DateTime<Utc>,
}

pub async fn list_alert_channels(
    pool: &PgPool,
    tenant_ctx: TenantContext,
) -> Result<Vec<AlertChannelView>, AppError> {
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let rows: Vec<ChannelRow> = sqlx::query_as(
        "SELECT c.id, c.tenant_id, c.workspace_id, w.slug AS workspace_slug,
                c.project_id, p.slug AS project_slug,
                c.name, c.channel_type, c.config, c.is_enabled,
                c.created_at, c.updated_at
         FROM alert_channels c
         LEFT JOIN workspaces w ON w.tenant_id = c.tenant_id AND w.id = c.workspace_id
         LEFT JOIN projects p ON p.tenant_id = c.tenant_id AND p.id = c.project_id
         WHERE c.tenant_id = $1
         ORDER BY c.id DESC
         LIMIT 200",
    )
    .bind(tenant_ctx.tenant_id())
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list alert_channels: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list alert_channels: {err}")))?;

    Ok(rows.into_iter().map(map_channel).collect())
}

pub async fn create_alert_channel(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    input: CreateAlertChannelInput,
    allow_private_ips: bool,
) -> Result<AlertChannelView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation(
            "channel name must be between 1 and 128 characters".to_owned(),
        ));
    }

    let channel_type = AlertChannelType::parse(&input.channel_type)?;
    let validated_config = validate_channel_config(channel_type, &input.config, allow_private_ips)?;
    let is_enabled = input.is_enabled.unwrap_or(true);

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let (workspace_id, workspace_slug, project_id, project_slug) = resolve_optional_scope_in_tx(
        &mut tx,
        tenant_ctx.tenant_id(),
        input.workspace_slug.as_deref(),
        input.project_slug.as_deref(),
    )
    .await?;

    let inserted: (i64, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO alert_channels (
             tenant_id, workspace_id, project_id, name, channel_type, config, is_enabled
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, created_at, updated_at",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(workspace_id)
    .bind(project_id)
    .bind(name)
    .bind(channel_type.as_str())
    .bind(&validated_config)
    .bind(is_enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert alert_channel: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create alert_channel: {err}")))?;

    Ok(map_channel(ChannelRow {
        id: inserted.0,
        tenant_id: tenant_ctx.tenant_id(),
        workspace_id,
        workspace_slug,
        project_id,
        project_slug,
        name: name.to_owned(),
        channel_type: channel_type.as_str().to_owned(),
        config: validated_config,
        is_enabled,
        created_at: inserted.1,
        updated_at: inserted.2,
    }))
}

pub async fn update_alert_channel(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    channel_id: i64,
    input: UpdateAlertChannelInput,
) -> Result<AlertChannelView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if channel_id <= 0 {
        return Err(AppError::Validation("invalid channel_id".to_owned()));
    }

    let name_clean = match input.name.as_deref() {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 128 {
                return Err(AppError::Validation(
                    "channel name must be between 1 and 128 characters".to_owned(),
                ));
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let updated: Option<i64> = sqlx::query_scalar(
        "UPDATE alert_channels
         SET name = COALESCE($3, name),
             is_enabled = COALESCE($4, is_enabled),
             updated_at = NOW()
         WHERE tenant_id = $1 AND id = $2
         RETURNING id",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(channel_id)
    .bind(name_clean)
    .bind(input.is_enabled)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update alert_channel: {err}")))?;

    if updated.is_none() {
        return Err(AppError::NotFound);
    }

    let row: ChannelRow = sqlx::query_as(
        "SELECT c.id, c.tenant_id, c.workspace_id, w.slug AS workspace_slug,
                c.project_id, p.slug AS project_slug,
                c.name, c.channel_type, c.config, c.is_enabled,
                c.created_at, c.updated_at
         FROM alert_channels c
         LEFT JOIN workspaces w ON w.tenant_id = c.tenant_id AND w.id = c.workspace_id
         LEFT JOIN projects p ON p.tenant_id = c.tenant_id AND p.id = c.project_id
         WHERE c.tenant_id = $1 AND c.id = $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(channel_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load updated alert_channel: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit update alert_channel: {err}")))?;

    Ok(map_channel(row))
}

pub async fn delete_alert_channel(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    channel_id: i64,
) -> Result<(), AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if channel_id <= 0 {
        return Err(AppError::Validation("invalid channel_id".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let res = sqlx::query("DELETE FROM alert_channels WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_ctx.tenant_id())
        .bind(channel_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("delete alert_channel: {err}")))?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit delete alert_channel: {err}")))?;
    Ok(())
}

pub async fn list_alert_rules(
    pool: &PgPool,
    tenant_ctx: TenantContext,
) -> Result<Vec<AlertRuleView>, AppError> {
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let rows: Vec<RuleRow> = sqlx::query_as(
        "SELECT r.id, r.tenant_id, r.workspace_id, w.slug AS workspace_slug,
                r.project_id, p.slug AS project_slug,
                r.channel_id, c.name AS channel_name, c.channel_type,
                r.name, r.trigger_type, r.environment_filter,
                r.spike_threshold, r.spike_window_minutes, r.spike_metric, r.cooldown_minutes,
                r.is_enabled, r.created_at, r.updated_at
         FROM alert_rules r
         JOIN alert_channels c ON c.tenant_id = r.tenant_id AND c.id = r.channel_id
         LEFT JOIN workspaces w ON w.tenant_id = r.tenant_id AND w.id = r.workspace_id
         LEFT JOIN projects p ON p.tenant_id = r.tenant_id AND p.id = r.project_id
         WHERE r.tenant_id = $1
         ORDER BY r.id DESC
         LIMIT 200",
    )
    .bind(tenant_ctx.tenant_id())
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list alert_rules: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list alert_rules: {err}")))?;

    Ok(rows.into_iter().map(map_rule).collect())
}

pub async fn create_alert_rule(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    input: CreateAlertRuleInput,
) -> Result<AlertRuleView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Validation(
            "rule name must be between 1 and 128 characters".to_owned(),
        ));
    }
    if input.channel_id <= 0 {
        return Err(AppError::Validation("invalid channel_id".to_owned()));
    }

    let trigger_type = AlertTriggerType::parse(&input.trigger_type)?;
    let environment_filter = match input
        .environment_filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(env) => {
            if env.len() > 64 {
                return Err(AppError::Validation(
                    "environment_filter must be at most 64 characters".to_owned(),
                ));
            }
            Some(env.to_owned())
        }
        None => None,
    };

    let (spike_threshold, spike_window_minutes) = match trigger_type {
        AlertTriggerType::Spike => {
            let threshold = input.spike_threshold.ok_or_else(|| {
                AppError::Validation("spike_threshold is required for spike rules".to_owned())
            })?;
            if !(2..=1_000_000).contains(&threshold) {
                return Err(AppError::Validation(
                    "spike_threshold must be between 2 and 1000000".to_owned(),
                ));
            }
            let window = input.spike_window_minutes.unwrap_or(60);
            if !(1..=1440).contains(&window) {
                return Err(AppError::Validation(
                    "spike_window_minutes must be between 1 and 1440".to_owned(),
                ));
            }
            (Some(threshold), Some(window))
        }
        AlertTriggerType::NewIssue | AlertTriggerType::Regression => (None, None),
    };

    let spike_metric = match input.spike_metric.as_deref() {
        Some(raw) => SpikeMetric::parse(raw)?,
        None => SpikeMetric::Events,
    };

    let cooldown_minutes = input.cooldown_minutes.unwrap_or(15);
    if !(1..=1440).contains(&cooldown_minutes) {
        return Err(AppError::Validation(
            "cooldown_minutes must be between 1 and 1440".to_owned(),
        ));
    }

    let is_enabled = input.is_enabled.unwrap_or(true);

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let channel_exists: Option<(String, String)> = sqlx::query_as(
        "SELECT name, channel_type FROM alert_channels WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(input.channel_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup alert_channel: {err}")))?;

    let Some((channel_name, channel_type)) = channel_exists else {
        return Err(AppError::NotFound);
    };

    let (workspace_id, workspace_slug, project_id, project_slug) = resolve_optional_scope_in_tx(
        &mut tx,
        tenant_ctx.tenant_id(),
        input.workspace_slug.as_deref(),
        input.project_slug.as_deref(),
    )
    .await?;

    let inserted: (i64, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO alert_rules (
             tenant_id, workspace_id, project_id, channel_id, name, trigger_type,
             environment_filter, spike_threshold, spike_window_minutes, cooldown_minutes, is_enabled,
             spike_metric
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         RETURNING id, created_at, updated_at",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(workspace_id)
    .bind(project_id)
    .bind(input.channel_id)
    .bind(name)
    .bind(trigger_type.as_str())
    .bind(&environment_filter)
    .bind(spike_threshold)
    .bind(spike_window_minutes)
    .bind(cooldown_minutes)
    .bind(is_enabled)
    .bind(spike_metric.as_str())
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert alert_rule: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit create alert_rule: {err}")))?;

    Ok(map_rule(RuleRow {
        id: inserted.0,
        tenant_id: tenant_ctx.tenant_id(),
        workspace_id,
        workspace_slug,
        project_id,
        project_slug,
        channel_id: input.channel_id,
        channel_name,
        channel_type,
        name: name.to_owned(),
        trigger_type: trigger_type.as_str().to_owned(),
        environment_filter,
        spike_threshold,
        spike_window_minutes,
        spike_metric: spike_metric.as_str().to_owned(),
        cooldown_minutes,
        is_enabled,
        created_at: inserted.1,
        updated_at: inserted.2,
    }))
}

pub async fn update_alert_rule(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    rule_id: i64,
    input: UpdateAlertRuleInput,
) -> Result<AlertRuleView, AppError> {
    if !role.can_manage_projects() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if rule_id <= 0 {
        return Err(AppError::Validation("invalid rule_id".to_owned()));
    }
    if let Some(th) = input.spike_threshold
        && !(2..=1_000_000).contains(&th)
    {
        return Err(AppError::Validation(
            "spike_threshold must be between 2 and 1000000".to_owned(),
        ));
    }
    if let Some(win) = input.spike_window_minutes
        && !(1..=1440).contains(&win)
    {
        return Err(AppError::Validation(
            "spike_window_minutes must be between 1 and 1440".to_owned(),
        ));
    }
    let spike_metric = input
        .spike_metric
        .as_deref()
        .map(SpikeMetric::parse)
        .transpose()?
        .map(SpikeMetric::as_str);
    if let Some(cd) = input.cooldown_minutes
        && !(1..=1440).contains(&cd)
    {
        return Err(AppError::Validation(
            "cooldown_minutes must be between 1 and 1440".to_owned(),
        ));
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

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let updated: Option<i64> = sqlx::query_scalar(
        "UPDATE alert_rules
         SET name = COALESCE($3, name),
             environment_filter = COALESCE($4, environment_filter),
             spike_threshold = COALESCE($5, spike_threshold),
             spike_window_minutes = COALESCE($6, spike_window_minutes),
             cooldown_minutes = COALESCE($7, cooldown_minutes),
             is_enabled = COALESCE($8, is_enabled),
             spike_metric = COALESCE($9, spike_metric),
             updated_at = NOW()
         WHERE tenant_id = $1 AND id = $2
         RETURNING id",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(rule_id)
    .bind(name_clean)
    .bind(
        input
            .environment_filter
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
    )
    .bind(input.spike_threshold)
    .bind(input.spike_window_minutes)
    .bind(input.cooldown_minutes)
    .bind(input.is_enabled)
    .bind(spike_metric)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update alert_rule: {err}")))?;

    if updated.is_none() {
        return Err(AppError::NotFound);
    }

    let row: RuleRow = sqlx::query_as(
        "SELECT r.id, r.tenant_id, r.workspace_id, w.slug AS workspace_slug,
                r.project_id, p.slug AS project_slug,
                r.channel_id, c.name AS channel_name, c.channel_type,
                r.name, r.trigger_type, r.environment_filter,
                r.spike_threshold, r.spike_window_minutes, r.spike_metric, r.cooldown_minutes,
                r.is_enabled, r.created_at, r.updated_at
         FROM alert_rules r
         JOIN alert_channels c ON c.tenant_id = r.tenant_id AND c.id = r.channel_id
         LEFT JOIN workspaces w ON w.tenant_id = r.tenant_id AND w.id = r.workspace_id
         LEFT JOIN projects p ON p.tenant_id = r.tenant_id AND p.id = r.project_id
         WHERE r.tenant_id = $1 AND r.id = $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(rule_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load updated alert_rule: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit update alert_rule: {err}")))?;

    Ok(map_rule(row))
}

pub async fn delete_alert_rule(
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
    let res = sqlx::query("DELETE FROM alert_rules WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_ctx.tenant_id())
        .bind(rule_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("delete alert_rule: {err}")))?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit delete alert_rule: {err}")))?;
    Ok(())
}

pub async fn list_alert_deliveries(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    limit: Option<u32>,
) -> Result<Vec<AlertDeliveryView>, AppError> {
    let limit = i64::from(limit.unwrap_or(50).clamp(1, 200));

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let rows: Vec<DeliveryRow> = sqlx::query_as(
        "SELECT d.id, d.tenant_id, d.project_id, p.slug AS project_slug,
                d.rule_id, r.name AS rule_name, d.channel_id, c.name AS channel_name,
                d.issue_id, i.title AS issue_title, d.trigger_type, d.channel_type,
                d.status, d.http_status, d.error_message, d.created_at
         FROM alert_deliveries d
         JOIN projects p ON p.tenant_id = d.tenant_id AND p.id = d.project_id
         JOIN alert_rules r ON r.tenant_id = d.tenant_id AND r.id = d.rule_id
         JOIN alert_channels c ON c.tenant_id = d.tenant_id AND c.id = d.channel_id
         JOIN issues i ON i.tenant_id = d.tenant_id AND i.project_id = d.project_id AND i.id = d.issue_id
         WHERE d.tenant_id = $1
         ORDER BY d.created_at DESC, d.id DESC
         LIMIT $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list alert_deliveries: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list alert_deliveries: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| AlertDeliveryView {
            id: r.id,
            tenant_id: r.tenant_id,
            project_id: r.project_id,
            project_slug: r.project_slug,
            rule_id: r.rule_id,
            rule_name: r.rule_name,
            channel_id: r.channel_id,
            channel_name: r.channel_name,
            issue_id: r.issue_id,
            issue_title: r.issue_title,
            trigger_type: r.trigger_type,
            channel_type: r.channel_type,
            status: r.status,
            http_status: r.http_status,
            error_message: r.error_message,
            created_at: r.created_at,
        })
        .collect())
}

async fn resolve_optional_scope_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    workspace_slug_opt: Option<&str>,
    project_slug_opt: Option<&str>,
) -> Result<(Option<i64>, Option<String>, Option<i64>, Option<String>), AppError> {
    let ws_slug = workspace_slug_opt.map(str::trim).filter(|s| !s.is_empty());
    let proj_slug = project_slug_opt.map(str::trim).filter(|s| !s.is_empty());

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

    match (ws_slug, proj_slug) {
        (None, None) => Ok((None, None, None, None)),
        (Some(ws), None) => {
            let ws_id: Option<i64> =
                sqlx::query_scalar("SELECT id FROM workspaces WHERE tenant_id = $1 AND slug = $2")
                    .bind(tenant_id)
                    .bind(ws)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|err| AppError::DatabaseQuery(format!("lookup workspace: {err}")))?;
            let Some(ws_id) = ws_id else {
                return Err(AppError::NotFound);
            };
            Ok((Some(ws_id), Some(ws.to_owned()), None, None))
        }
        (ws_opt, Some(p_slug)) => {
            let row: Option<(i64, String, i64, String)> = match ws_opt {
                Some(ws) => sqlx::query_as(
                    "SELECT w.id, w.slug, p.id, p.slug
                         FROM projects p
                         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                         WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
                )
                .bind(tenant_id)
                .bind(ws)
                .bind(p_slug)
                .fetch_optional(&mut **tx)
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
                .fetch_optional(&mut **tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("lookup project: {err}")))?,
            };
            let Some((ws_id, ws_s, p_id, p_s)) = row else {
                return Err(AppError::NotFound);
            };
            Ok((Some(ws_id), Some(ws_s), Some(p_id), Some(p_s)))
        }
    }
}
