use chrono::{DateTime, Utc};
use maxi_loop_auth::model::Role;
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};

use crate::model::{IssueAssigneeView, IssueSummaryView};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseView {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub project_id: i64,
    pub project_slug: String,
    pub version: String,
    pub commit_sha: Option<String>,
    pub environment: Option<String>,
    pub notes: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub deployed_at: Option<DateTime<Utc>>,
    pub event_count: i64,
    pub new_issues_count: i64,
    pub linked_issues_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ReleaseListFilter {
    pub project_slug: Option<String>,
    pub project_id: Option<i64>,
    pub environment: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpsertReleaseInput {
    pub project_slug: String,
    pub workspace_slug: Option<String>,
    pub version: String,
    pub commit_sha: Option<String>,
    pub environment: Option<String>,
    pub notes: Option<String>,
    pub deployed_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct ReleaseJoinedDbRow {
    id: i64,
    tenant_id: i64,
    workspace_id: i64,
    workspace_slug: String,
    project_id: i64,
    project_slug: String,
    version: String,
    commit_sha: Option<String>,
    environment: Option<String>,
    notes: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    deployed_at: Option<DateTime<Utc>>,
    event_count: i64,
    new_issues_count: i64,
    linked_issues_count: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<ReleaseJoinedDbRow> for ReleaseView {
    fn from(r: ReleaseJoinedDbRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            workspace_id: r.workspace_id,
            workspace_slug: r.workspace_slug,
            project_id: r.project_id,
            project_slug: r.project_slug,
            version: r.version,
            commit_sha: r.commit_sha,
            environment: r.environment,
            notes: r.notes,
            first_seen: r.first_seen,
            last_seen: r.last_seen,
            deployed_at: r.deployed_at,
            event_count: r.event_count,
            new_issues_count: r.new_issues_count,
            linked_issues_count: r.linked_issues_count,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(FromRow)]
struct LinkedIssueRow {
    id: i64,
    tenant_id: i64,
    workspace_id: i64,
    workspace_slug: String,
    project_id: i64,
    project_slug: String,
    fingerprint: String,
    title: String,
    exception_type: String,
    exception_value: String,
    culprit: String,
    level: String,
    issue_type: String,
    status: String,
    is_regressed: bool,
    event_count: i64,
    user_count: i64,
    stored_full_events: i32,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    last_environment: String,
    first_release: Option<String>,
    last_release: Option<String>,
    sample_event_id: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    assignee_user_id: Option<i64>,
    assignee_email: Option<String>,
    assignee_name: Option<String>,
}

pub async fn list_releases(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    filter: ReleaseListFilter,
) -> Result<Vec<ReleaseView>, AppError> {
    if let Some(slug) = filter.project_slug.as_deref()
        && !slug.trim().is_empty()
        && !is_valid_slug(slug.trim())
    {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }
    if let Some(pid) = filter.project_id
        && pid <= 0
    {
        return Err(AppError::Validation(
            "project_id must be positive".to_owned(),
        ));
    }
    let limit = i64::from(filter.limit.unwrap_or(50).clamp(1, 200));

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT r.id, r.tenant_id, p.workspace_id, w.slug AS workspace_slug, \
                r.project_id, p.slug AS project_slug, r.version, r.commit_sha, \
                r.environment, r.notes, r.first_seen, r.last_seen, r.deployed_at, \
                r.event_count, r.new_issues_count, \
                (SELECT COUNT(*) FROM issue_releases ir \
                 WHERE ir.tenant_id = r.tenant_id AND ir.project_id = r.project_id \
                   AND ir.release_version = r.version) AS linked_issues_count, \
                r.created_at, r.updated_at \
         FROM releases r \
         JOIN projects p ON p.tenant_id = r.tenant_id AND p.id = r.project_id \
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id \
         WHERE r.tenant_id = ",
    );
    qb.push_bind(tenant_ctx.tenant_id());

    if let Some(pid) = filter.project_id {
        qb.push(" AND r.project_id = ").push_bind(pid);
    }
    if let Some(slug) = filter
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        qb.push(" AND p.slug = ").push_bind(slug);
    }
    if let Some(env) = filter
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if env.len() > 64 {
            return Err(AppError::Validation(
                "environment filter too long".to_owned(),
            ));
        }
        qb.push(" AND r.environment = ").push_bind(env);
    }

    qb.push(" ORDER BY COALESCE(r.deployed_at, r.last_seen) DESC, r.id DESC LIMIT ")
        .push_bind(limit);

    let rows: Vec<ReleaseJoinedDbRow> = qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list releases: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list releases: {err}")))?;

    Ok(rows.into_iter().map(ReleaseView::from).collect())
}

pub async fn upsert_release(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    input: UpsertReleaseInput,
) -> Result<ReleaseView, AppError> {
    if !role.can_triage_issues() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }

    let proj_slug = input.project_slug.trim();
    if !is_valid_slug(proj_slug) {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }
    if let Some(ws) = input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && !is_valid_slug(ws)
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let version = input.version.trim();
    if version.is_empty() || version.len() > 128 || version.chars().any(char::is_whitespace) {
        return Err(AppError::Validation(
            "release version must be 1..=128 non-whitespace characters".to_owned(),
        ));
    }

    let commit_sha = match input
        .commit_sha
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(sha) => {
            if !(7..=40).contains(&sha.len()) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(AppError::Validation(
                    "commit_sha must be 7..=40 hexadecimal characters".to_owned(),
                ));
            }
            Some(sha.to_ascii_lowercase())
        }
        None => infer_commit_sha(version),
    };

    let environment = match input
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(env) => {
            if env.len() > 64 {
                return Err(AppError::Validation(
                    "environment must be at most 64 characters".to_owned(),
                ));
            }
            Some(env.to_owned())
        }
        None => None,
    };

    let notes = input.notes.unwrap_or_default();
    if notes.len() > 2000 {
        return Err(AppError::Validation(
            "notes must be at most 2000 characters".to_owned(),
        ));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;

    let proj_row: Option<(i64, i64, String)> = match input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(ws_slug) => sqlx::query_as(
            "SELECT p.id, p.workspace_id, w.slug
                 FROM projects p
                 JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                 WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
        )
        .bind(tenant_ctx.tenant_id())
        .bind(ws_slug)
        .bind(proj_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for release: {err}")))?,
        None => sqlx::query_as(
            "SELECT p.id, p.workspace_id, w.slug
                 FROM projects p
                 JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
                 WHERE p.tenant_id = $1 AND p.slug = $2
                 ORDER BY w.is_default DESC, p.id ASC
                 LIMIT 1",
        )
        .bind(tenant_ctx.tenant_id())
        .bind(proj_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for release: {err}")))?,
    };

    let Some((project_id, _workspace_id, _workspace_slug)) = proj_row else {
        return Err(AppError::NotFound);
    };

    let deployed_at = input.deployed_at.or_else(|| Some(Utc::now()));
    let release_id: i64 = sqlx::query_scalar(
        "INSERT INTO releases (
             tenant_id, project_id, version, commit_sha, environment, notes, deployed_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (tenant_id, project_id, version) DO UPDATE SET
             commit_sha = COALESCE(EXCLUDED.commit_sha, releases.commit_sha),
             environment = COALESCE(EXCLUDED.environment, releases.environment),
             notes = CASE WHEN EXCLUDED.notes <> '' THEN EXCLUDED.notes ELSE releases.notes END,
             deployed_at = COALESCE(EXCLUDED.deployed_at, releases.deployed_at),
             updated_at = NOW()
         RETURNING id",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(project_id)
    .bind(version)
    .bind(&commit_sha)
    .bind(&environment)
    .bind(&notes)
    .bind(deployed_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("upsert release: {err}")))?;

    let row: ReleaseJoinedDbRow = sqlx::query_as(
        "SELECT r.id, r.tenant_id, p.workspace_id, w.slug AS workspace_slug,
                r.project_id, p.slug AS project_slug, r.version, r.commit_sha,
                r.environment, r.notes, r.first_seen, r.last_seen, r.deployed_at,
                r.event_count, r.new_issues_count,
                (SELECT COUNT(*) FROM issue_releases ir
                 WHERE ir.tenant_id = r.tenant_id AND ir.project_id = r.project_id
                   AND ir.release_version = r.version) AS linked_issues_count,
                r.created_at, r.updated_at
         FROM releases r
         JOIN projects p ON p.tenant_id = r.tenant_id AND p.id = r.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE r.tenant_id = $1 AND r.id = $2",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(release_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load upserted release: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit upsert release: {err}")))?;

    Ok(ReleaseView::from(row))
}

pub async fn list_release_issues(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    release_id: i64,
    limit: Option<u32>,
) -> Result<Vec<IssueSummaryView>, AppError> {
    if release_id <= 0 {
        return Err(AppError::Validation("invalid release_id".to_owned()));
    }
    let limit = i64::from(limit.unwrap_or(50).clamp(1, 200));

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let rel: Option<(i64, String)> =
        sqlx::query_as("SELECT project_id, version FROM releases WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_ctx.tenant_id())
            .bind(release_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("lookup release: {err}")))?;

    let Some((project_id, version)) = rel else {
        return Err(AppError::NotFound);
    };

    let rows: Vec<LinkedIssueRow> = sqlx::query_as(
        "SELECT i.id, i.tenant_id, p.workspace_id, w.slug AS workspace_slug,
                i.project_id, p.slug AS project_slug, i.fingerprint, i.title,
                i.exception_type, i.exception_value, i.culprit, i.level, i.issue_type,
                i.status, i.is_regressed, i.event_count, i.user_count, i.stored_full_events,
                i.first_seen, i.last_seen, i.last_environment, i.first_release,
                i.last_release, i.sample_event_id, i.created_at, i.updated_at,
                i.assignee_user_id, au.email AS assignee_email, au.name AS assignee_name
         FROM issue_releases ir
         JOIN issues i ON i.tenant_id = ir.tenant_id AND i.project_id = ir.project_id AND i.id = ir.issue_id
         JOIN projects p ON p.tenant_id = i.tenant_id AND p.id = i.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         LEFT JOIN users au ON au.id = i.assignee_user_id
         WHERE ir.tenant_id = $1 AND ir.project_id = $2 AND ir.release_version = $3
         ORDER BY ir.last_seen DESC, i.id DESC
         LIMIT $4",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(project_id)
    .bind(&version)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list release issues: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list release issues: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|row| IssueSummaryView {
            id: row.id,
            tenant_id: row.tenant_id,
            workspace_id: row.workspace_id,
            workspace_slug: row.workspace_slug,
            project_id: row.project_id,
            project_slug: row.project_slug,
            fingerprint: row.fingerprint,
            title: row.title,
            exception_type: row.exception_type,
            exception_value: row.exception_value,
            culprit: row.culprit,
            level: row.level,
            issue_type: row.issue_type,
            status: row.status,
            is_regressed: row.is_regressed,
            event_count: row.event_count,
            user_count: row.user_count,
            stored_full_events: row.stored_full_events,
            first_seen: row.first_seen,
            last_seen: row.last_seen,
            last_environment: row.last_environment,
            first_release: row.first_release,
            last_release: row.last_release,
            sample_event_id: row.sample_event_id,
            assignee: IssueAssigneeView::from_columns(
                row.assignee_user_id,
                row.assignee_email,
                row.assignee_name,
            ),
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

pub async fn resolve_release_commit_sha(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    project_id: i64,
    version: &str,
) -> Result<Option<String>, AppError> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let stored: Option<Option<String>> = sqlx::query_scalar(
        "SELECT commit_sha
         FROM releases
         WHERE tenant_id = $1 AND project_id = $2 AND version = $3",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(project_id)
    .bind(trimmed)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("resolve release commit_sha: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::DatabaseQuery(format!("commit resolve_release_commit_sha: {err}"))
    })?;

    if let Some(Some(sha)) = stored {
        return Ok(Some(sha));
    }

    Ok(infer_commit_sha(trimmed))
}

pub fn infer_commit_sha(version: &str) -> Option<String> {
    let candidate = version.rsplit('@').next().unwrap_or(version).trim();
    if (7..=40).contains(&candidate.len()) && candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(candidate.to_ascii_lowercase())
    } else {
        None
    }
}
