use chrono::{DateTime, Duration, Utc};
use maxi_loop_auth::model::Role;
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder, Transaction};

use crate::model::{
    CounterBucketPoint, IssueDetailView, IssueEventView, IssueListFilter, IssueSort,
    IssueStatsResponse, IssueStatus, IssueSummaryView, PaginatedIssuesResponse, StatsInterval,
    UpdateIssueStatusInput,
};
use crate::notes::{
    CreateIssueNoteInput, IssueNoteView, NewIssueNote, NoteActor, add_issue_note,
    insert_issue_note_in_tx, list_issue_notes, validate_commit_ref, validate_note_body,
};
use crate::releases::resolve_release_commit_sha;

/// Service facade owned by `maxi-loop-issues` so upstream layers (like `maxi-loop-mcp`) can call
/// all issue, event, stats, release, and note operations without importing `sqlx` (`D006`).
#[derive(Clone)]
pub struct IssuesService {
    db: PgPool,
}

impl IssuesService {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn list_issues(
        &self,
        tenant_ctx: TenantContext,
        scope: IssueScope<'_>,
        filter: IssueListFilter,
    ) -> Result<PaginatedIssuesResponse, AppError> {
        list_issues(&self.db, tenant_ctx, scope, filter).await
    }

    pub async fn get_issue_detail(
        &self,
        tenant_ctx: TenantContext,
        scope: IssueScope<'_>,
        issue_id: i64,
        hours: Option<u32>,
    ) -> Result<IssueDetailView, AppError> {
        get_issue_detail(&self.db, tenant_ctx, scope, issue_id, hours).await
    }

    pub async fn get_issue_stats(
        &self,
        tenant_ctx: TenantContext,
        scope: IssueScope<'_>,
        issue_id: i64,
        interval: Option<&str>,
        hours: Option<u32>,
    ) -> Result<IssueStatsResponse, AppError> {
        get_issue_stats(&self.db, tenant_ctx, scope, issue_id, interval, hours).await
    }

    pub async fn get_issue_event(
        &self,
        tenant_ctx: TenantContext,
        scope: IssueScope<'_>,
        issue_id: i64,
        event_id: &str,
    ) -> Result<IssueEventView, AppError> {
        get_issue_event(&self.db, tenant_ctx, scope, issue_id, event_id).await
    }

    pub async fn update_issue_status_with_actor(
        &self,
        tenant_ctx: TenantContext,
        role: Role,
        scope: IssueScope<'_>,
        issue_id: i64,
        actor: NoteActor<'_>,
        input: UpdateIssueStatusInput,
    ) -> Result<(IssueSummaryView, Option<IssueNoteView>), AppError> {
        update_issue_status_with_actor(&self.db, tenant_ctx, role, scope, issue_id, actor, input)
            .await
    }

    pub async fn add_issue_note(
        &self,
        tenant_ctx: TenantContext,
        role: Role,
        scope: IssueScope<'_>,
        issue_id: i64,
        actor: NoteActor<'_>,
        input: CreateIssueNoteInput,
    ) -> Result<IssueNoteView, AppError> {
        add_issue_note(&self.db, tenant_ctx, role, scope, issue_id, actor, input).await
    }

    pub async fn list_issue_notes(
        &self,
        tenant_ctx: TenantContext,
        scope: IssueScope<'_>,
        issue_id: i64,
        limit: Option<u32>,
    ) -> Result<Vec<IssueNoteView>, AppError> {
        list_issue_notes(&self.db, tenant_ctx, scope, issue_id, limit).await
    }

    pub async fn resolve_release_commit_sha(
        &self,
        tenant_ctx: TenantContext,
        project_id: i64,
        version: &str,
    ) -> Result<Option<String>, AppError> {
        resolve_release_commit_sha(&self.db, tenant_ctx, project_id, version).await
    }
}

#[derive(Clone, Debug, Default)]
pub struct IssueScope<'a> {
    pub tenant_slug: &'a str,
    pub workspace_slug: Option<&'a str>,
    pub project_slug: Option<&'a str>,
}

#[derive(FromRow)]
struct IssueSummaryDbRow {
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
    status: String,
    is_regressed: bool,
    event_count: i64,
    stored_full_events: i32,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    last_environment: String,
    first_release: Option<String>,
    last_release: Option<String>,
    sample_event_id: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<IssueSummaryDbRow> for IssueSummaryView {
    fn from(row: IssueSummaryDbRow) -> Self {
        Self {
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
            status: row.status,
            is_regressed: row.is_regressed,
            event_count: row.event_count,
            stored_full_events: row.stored_full_events,
            first_seen: row.first_seen,
            last_seen: row.last_seen,
            last_environment: row.last_environment,
            first_release: row.first_release,
            last_release: row.last_release,
            sample_event_id: row.sample_event_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(FromRow)]
struct IssueEventDbRow {
    event_id: String,
    issue_id: i64,
    project_id: i64,
    received_at: DateTime<Utc>,
    environment: String,
    release: Option<String>,
    level: String,
    platform: String,
    exception_type: String,
    exception_value: String,
    stack_frames: Value,
    tags: Value,
    payload: Value,
    is_sample: bool,
}

impl From<IssueEventDbRow> for IssueEventView {
    fn from(row: IssueEventDbRow) -> Self {
        Self {
            event_id: row.event_id,
            issue_id: row.issue_id,
            project_id: row.project_id,
            received_at: row.received_at,
            environment: row.environment,
            release: row.release,
            level: row.level,
            platform: row.platform,
            exception_type: row.exception_type,
            exception_value: row.exception_value,
            stack_frames: row.stack_frames,
            tags: row.tags,
            payload: row.payload,
            is_sample: row.is_sample,
        }
    }
}

#[derive(FromRow)]
struct CounterBucketDbRow {
    bucket: DateTime<Utc>,
    event_count: i64,
}

pub async fn list_issues(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: IssueScope<'_>,
    mut filter: IssueListFilter,
) -> Result<PaginatedIssuesResponse, AppError> {
    if let Some(ws_slug) = scope.workspace_slug {
        filter.workspace_slug = Some(ws_slug.to_owned());
    }
    if let Some(proj_slug) = scope.project_slug {
        filter.project_slug = Some(proj_slug.to_owned());
    }

    let validated = ValidatedIssueFilter::from_raw(filter)?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    // If a specific workspace_slug or project_slug was requested in the URL path, ensure it
    // actually exists in this tenant so cross-tenant or unknown project paths return 404.
    if let Some(ws_slug) = scope.workspace_slug {
        verify_workspace_slug_in_tx(&mut tx, tenant_ctx.tenant_id(), ws_slug).await?;
    }
    if let Some(proj_slug) = scope.project_slug {
        verify_project_slug_in_tx(
            &mut tx,
            tenant_ctx.tenant_id(),
            scope.workspace_slug,
            proj_slug,
        )
        .await?;
    }

    let mut count_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT COUNT(*) \
         FROM issues i \
         JOIN projects p ON p.tenant_id = i.tenant_id AND p.id = i.project_id \
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id \
         WHERE i.tenant_id = ",
    );
    count_qb.push_bind(tenant_ctx.tenant_id());
    validated.append_where_clauses(&mut count_qb);

    let total: i64 = count_qb
        .build_query_scalar()
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("count issues: {err}")))?;

    let mut list_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT i.id, i.tenant_id, p.workspace_id, w.slug AS workspace_slug, \
                i.project_id, p.slug AS project_slug, i.fingerprint, i.title, \
                i.exception_type, i.exception_value, i.culprit, i.level, \
                i.status, i.is_regressed, i.event_count, i.stored_full_events, \
                i.first_seen, i.last_seen, i.last_environment, i.first_release, i.last_release, \
                i.sample_event_id, i.created_at, i.updated_at \
         FROM issues i \
         JOIN projects p ON p.tenant_id = i.tenant_id AND p.id = i.project_id \
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id \
         WHERE i.tenant_id = ",
    );
    list_qb.push_bind(tenant_ctx.tenant_id());
    validated.append_where_clauses(&mut list_qb);
    list_qb.push(validated.sort.order_by_sql());
    list_qb
        .push(" LIMIT ")
        .push_bind(i64::from(validated.per_page))
        .push(" OFFSET ")
        .push_bind(validated.offset);

    let rows: Vec<IssueSummaryDbRow> = list_qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list issues: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_issues: {err}")))?;

    let total_u64 = u64::try_from(total.max(0)).unwrap_or(0);
    let per_page_u64 = u64::from(validated.per_page.max(1));
    let total_pages = u32::try_from(total_u64.div_ceil(per_page_u64)).unwrap_or(0);

    Ok(PaginatedIssuesResponse {
        items: rows.into_iter().map(IssueSummaryView::from).collect(),
        total,
        page: validated.page,
        per_page: validated.per_page,
        total_pages,
    })
}

pub async fn get_issue_detail(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: IssueScope<'_>,
    issue_id: i64,
    hours: Option<u32>,
) -> Result<IssueDetailView, AppError> {
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let lookback_hours = validate_lookback_hours(hours)?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    let issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;
    let sample_event = load_sample_event_in_tx(
        &mut tx,
        tenant_ctx.tenant_id(),
        issue.project_id,
        issue.id,
        issue.sample_event_id.as_deref(),
    )
    .await?;

    let since = Utc::now() - Duration::hours(i64::from(lookback_hours));
    let hourly_series = query_counter_series_in_tx(
        &mut tx,
        tenant_ctx.tenant_id(),
        issue.project_id,
        issue.id,
        StatsInterval::Hourly,
        since,
    )
    .await?;
    let daily_series = query_counter_series_in_tx(
        &mut tx,
        tenant_ctx.tenant_id(),
        issue.project_id,
        issue.id,
        StatsInterval::Daily,
        since,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit get_issue_detail: {err}")))?;

    let (stack_frames, tags) = match &sample_event {
        Some(ev) => (ev.stack_frames.clone(), ev.tags.clone()),
        None => (json!([]), json!({})),
    };

    Ok(IssueDetailView {
        issue,
        stack_frames,
        tags,
        sample_event,
        hourly_series,
        daily_series,
    })
}

pub async fn get_issue_stats(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: IssueScope<'_>,
    issue_id: i64,
    interval: Option<&str>,
    hours: Option<u32>,
) -> Result<IssueStatsResponse, AppError> {
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let parsed_interval = StatsInterval::parse(interval)?;
    let lookback_hours = validate_lookback_hours(hours)?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    let since = Utc::now() - Duration::hours(i64::from(lookback_hours));
    let series = query_counter_series_in_tx(
        &mut tx,
        tenant_ctx.tenant_id(),
        issue.project_id,
        issue.id,
        parsed_interval,
        since,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit get_issue_stats: {err}")))?;

    Ok(IssueStatsResponse {
        issue_id: issue.id,
        project_id: issue.project_id,
        interval: parsed_interval.as_str().to_owned(),
        series,
    })
}

pub async fn list_issue_events(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: IssueScope<'_>,
    issue_id: i64,
    limit: Option<u32>,
) -> Result<Vec<IssueEventView>, AppError> {
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let max_rows = match limit {
        None => 50i64,
        Some(v) if (1..=100).contains(&v) => i64::from(v),
        Some(_) => {
            return Err(AppError::Validation(
                "limit must be between 1 and 100".to_owned(),
            ));
        }
    };

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    let rows: Vec<IssueEventDbRow> = sqlx::query_as(
        "SELECT event_id, issue_id, project_id, received_at, environment, release, \
                level, platform, exception_type, exception_value, stack_frames, \
                tags, payload, is_sample \
         FROM events \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 \
         ORDER BY received_at DESC, event_id DESC \
         LIMIT $4",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(issue.project_id)
    .bind(issue.id)
    .bind(max_rows)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list issue events: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_issue_events: {err}")))?;

    Ok(rows.into_iter().map(IssueEventView::from).collect())
}

pub async fn get_issue_event(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: IssueScope<'_>,
    issue_id: i64,
    event_id: &str,
) -> Result<IssueEventView, AppError> {
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let event_id = event_id.trim();
    if event_id.is_empty() || event_id.len() > 128 {
        return Err(AppError::Validation("invalid event_id".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    let row: Option<IssueEventDbRow> = sqlx::query_as(
        "SELECT event_id, issue_id, project_id, received_at, environment, release, \
                level, platform, exception_type, exception_value, stack_frames, \
                tags, payload, is_sample \
         FROM events \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND event_id = $4 \
         ORDER BY received_at DESC \
         LIMIT 1",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(issue.project_id)
    .bind(issue.id)
    .bind(event_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("get issue event: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit get_issue_event: {err}")))?;

    row.map(IssueEventView::from).ok_or(AppError::NotFound)
}

pub async fn update_issue_status(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    issue_id: i64,
    input: UpdateIssueStatusInput,
) -> Result<IssueSummaryView, AppError> {
    let (updated, _) =
        update_issue_status_inner(pool, tenant_ctx, role, scope, issue_id, None, input).await?;
    Ok(updated)
}

pub async fn update_issue_status_with_actor(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    issue_id: i64,
    actor: NoteActor<'_>,
    input: UpdateIssueStatusInput,
) -> Result<(IssueSummaryView, Option<IssueNoteView>), AppError> {
    update_issue_status_inner(pool, tenant_ctx, role, scope, issue_id, Some(actor), input).await
}

async fn update_issue_status_inner(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    issue_id: i64,
    actor: Option<NoteActor<'_>>,
    input: UpdateIssueStatusInput,
) -> Result<(IssueSummaryView, Option<IssueNoteView>), AppError> {
    if !role.can_triage_issues() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let target_status = IssueStatus::parse(&input.status)?;
    let note_body = match input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Some(validate_note_body(raw)?),
        None => None,
    };
    let commit_ref = validate_commit_ref(input.commit_ref.as_deref())?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let existing = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    // Clearing `is_regressed` when an issue is marked `resolved` ensures that if it re-occurs
    // later, the worker's regression transition (`resolved` -> `unresolved`, `is_regressed = true`)
    // reflects the new regression state accurately.
    let next_regressed = match target_status {
        IssueStatus::Resolved => false,
        IssueStatus::Unresolved | IssueStatus::Ignored => existing.is_regressed,
    };

    sqlx::query(
        "UPDATE issues \
         SET status = $1, \
             is_regressed = $2, \
             updated_at = NOW() \
         WHERE tenant_id = $3 AND project_id = $4 AND id = $5",
    )
    .bind(target_status.as_str())
    .bind(next_regressed)
    .bind(tenant_ctx.tenant_id())
    .bind(existing.project_id)
    .bind(existing.id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update issue status: {err}")))?;

    let created_note = if let Some(ref act) = actor
        && (note_body.is_some() || commit_ref.is_some())
    {
        let body_text =
            note_body.unwrap_or_else(|| format!("Status changed to {}", target_status.as_str()));
        Some(
            insert_issue_note_in_tx(
                &mut tx,
                NewIssueNote {
                    tenant_id: tenant_ctx.tenant_id(),
                    project_id: existing.project_id,
                    issue_id: existing.id,
                    actor: act,
                    body: &body_text,
                    commit_ref: commit_ref.as_deref(),
                    status_change: Some(target_status),
                },
            )
            .await?,
        )
    } else {
        None
    };

    let updated = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit update_issue_status: {err}")))?;

    Ok((updated, created_note))
}

struct ValidatedIssueFilter {
    workspace_slug: Option<String>,
    project_id: Option<i64>,
    project_slug: Option<String>,
    environment: Option<String>,
    release: Option<String>,
    status: Option<IssueStatus>,
    is_regressed: Option<bool>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    query: Option<String>,
    sort: IssueSort,
    page: u32,
    per_page: u32,
    offset: i64,
}

impl ValidatedIssueFilter {
    fn from_raw(raw: IssueListFilter) -> Result<Self, AppError> {
        let workspace_slug = match raw
            .workspace_slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(slug) => {
                if !is_valid_slug(slug) {
                    return Err(AppError::Validation("invalid workspace_slug".to_owned()));
                }
                Some(slug.to_owned())
            }
            None => None,
        };

        if let Some(pid) = raw.project_id
            && pid <= 0
        {
            return Err(AppError::Validation(
                "project_id must be a positive integer".to_owned(),
            ));
        }

        let project_slug = match raw
            .project_slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(slug) => {
                if !is_valid_slug(slug) {
                    return Err(AppError::Validation("invalid project_slug".to_owned()));
                }
                Some(slug.to_owned())
            }
            None => None,
        };

        let environment = match raw
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(env) => {
                if env.len() > 64 {
                    return Err(AppError::Validation(
                        "environment filter must be at most 64 characters".to_owned(),
                    ));
                }
                Some(env.to_owned())
            }
            None => None,
        };

        let release = match raw
            .release
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(rel) => {
                if rel.len() > 128 {
                    return Err(AppError::Validation(
                        "release filter must be at most 128 characters".to_owned(),
                    ));
                }
                Some(rel.to_owned())
            }
            None => None,
        };

        let status = match raw
            .status
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(st) => Some(IssueStatus::parse(st)?),
            None => None,
        };

        let query = match raw
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(q) => {
                if q.len() > 128 {
                    return Err(AppError::Validation(
                        "search query must be at most 128 characters".to_owned(),
                    ));
                }
                Some(q.to_owned())
            }
            None => None,
        };

        let sort = IssueSort::parse(raw.sort.as_deref())?;
        let page = raw.page.unwrap_or(1);
        if !(1..=10_000).contains(&page) {
            return Err(AppError::Validation(
                "page must be between 1 and 10000".to_owned(),
            ));
        }

        let per_page = raw.per_page.unwrap_or(25);
        if !(1..=100).contains(&per_page) {
            return Err(AppError::Validation(
                "per_page must be between 1 and 100".to_owned(),
            ));
        }

        if let (Some(since), Some(until)) = (raw.since, raw.until)
            && since > until
        {
            return Err(AppError::Validation(
                "since timestamp must be <= until timestamp".to_owned(),
            ));
        }

        let offset = i64::from(page - 1) * i64::from(per_page);

        Ok(Self {
            workspace_slug,
            project_id: raw.project_id,
            project_slug,
            environment,
            release,
            status,
            is_regressed: raw.is_regressed,
            since: raw.since,
            until: raw.until,
            query,
            sort,
            page,
            per_page,
            offset,
        })
    }

    fn append_where_clauses(&self, qb: &mut QueryBuilder<'_, Postgres>) {
        if let Some(ws_slug) = &self.workspace_slug {
            qb.push(" AND w.slug = ").push_bind(ws_slug.clone());
        }
        if let Some(pid) = self.project_id {
            qb.push(" AND i.project_id = ").push_bind(pid);
        }
        if let Some(proj_slug) = &self.project_slug {
            qb.push(" AND p.slug = ").push_bind(proj_slug.clone());
        }
        if let Some(env) = &self.environment {
            qb.push(" AND i.last_environment = ").push_bind(env.clone());
        }
        if let Some(rel) = &self.release {
            qb.push(" AND (i.last_release = ")
                .push_bind(rel.clone())
                .push(
                    " OR EXISTS (SELECT 1 FROM issue_releases ir \
                             WHERE ir.tenant_id = i.tenant_id AND ir.project_id = i.project_id \
                               AND ir.issue_id = i.id AND ir.release_version = ",
                )
                .push_bind(rel.clone())
                .push("))");
        }
        if let Some(st) = self.status {
            qb.push(" AND i.status = ").push_bind(st.as_str());
        }
        if let Some(reg) = self.is_regressed {
            qb.push(" AND i.is_regressed = ").push_bind(reg);
        }
        if let Some(since) = self.since {
            qb.push(" AND i.last_seen >= ").push_bind(since);
        }
        if let Some(until) = self.until {
            qb.push(" AND i.last_seen <= ").push_bind(until);
        }
        if let Some(q) = &self.query {
            let escaped = q
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{escaped}%");
            qb.push(" AND (i.title ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR i.culprit ILIKE ")
                .push_bind(pattern)
                .push(")");
        }
    }
}

fn validate_lookback_hours(hours: Option<u32>) -> Result<u32, AppError> {
    let h = hours.unwrap_or(168);
    if !(1..=2160).contains(&h) {
        return Err(AppError::Validation(
            "hours must be between 1 and 2160".to_owned(),
        ));
    }
    Ok(h)
}

pub(crate) async fn verify_tenant_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_ctx: TenantContext,
    expected_slug: &str,
) -> Result<(), AppError> {
    if !is_valid_slug(expected_slug) {
        return Err(AppError::Validation("invalid tenant_slug".to_owned()));
    }

    let exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM tenants WHERE id = $1 AND slug = $2")
            .bind(tenant_ctx.tenant_id())
            .bind(expected_slug)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("verify tenant slug: {err}")))?;

    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(())
}

async fn verify_workspace_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    workspace_slug: &str,
) -> Result<i64, AppError> {
    if !is_valid_slug(workspace_slug) {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }
    let ws_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE tenant_id = $1 AND slug = $2")
            .bind(tenant_id)
            .bind(workspace_slug)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("verify workspace slug: {err}")))?;
    ws_id.ok_or(AppError::NotFound)
}

async fn verify_project_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    workspace_slug: Option<&str>,
    project_slug: &str,
) -> Result<i64, AppError> {
    if !is_valid_slug(project_slug) {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let proj_id: Option<i64> = match workspace_slug {
        Some(ws_slug) => sqlx::query_scalar(
            "SELECT p.id \
                 FROM projects p \
                 JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id \
                 WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
        )
        .bind(tenant_id)
        .bind(ws_slug)
        .bind(project_slug)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("verify project slug: {err}")))?,
        None => {
            sqlx::query_scalar("SELECT id FROM projects WHERE tenant_id = $1 AND slug = $2 LIMIT 1")
                .bind(tenant_id)
                .bind(project_slug)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("verify project slug: {err}")))?
        }
    };

    proj_id.ok_or(AppError::NotFound)
}

pub(crate) async fn load_scoped_issue_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_ctx: TenantContext,
    scope: &IssueScope<'_>,
    issue_id: i64,
) -> Result<IssueSummaryView, AppError> {
    if let Some(ws_slug) = scope.workspace_slug
        && !is_valid_slug(ws_slug)
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }
    if let Some(proj_slug) = scope.project_slug
        && !is_valid_slug(proj_slug)
    {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT i.id, i.tenant_id, p.workspace_id, w.slug AS workspace_slug, \
                i.project_id, p.slug AS project_slug, i.fingerprint, i.title, \
                i.exception_type, i.exception_value, i.culprit, i.level, \
                i.status, i.is_regressed, i.event_count, i.stored_full_events, \
                i.first_seen, i.last_seen, i.last_environment, i.first_release, i.last_release, \
                i.sample_event_id, i.created_at, i.updated_at \
         FROM issues i \
         JOIN projects p ON p.tenant_id = i.tenant_id AND p.id = i.project_id \
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id \
         WHERE i.tenant_id = ",
    );
    qb.push_bind(tenant_ctx.tenant_id())
        .push(" AND i.id = ")
        .push_bind(issue_id);

    if let Some(ws_slug) = scope.workspace_slug {
        qb.push(" AND w.slug = ").push_bind(ws_slug);
    }
    if let Some(proj_slug) = scope.project_slug {
        qb.push(" AND p.slug = ").push_bind(proj_slug);
    }

    let row: Option<IssueSummaryDbRow> = qb
        .build_query_as()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("load scoped issue: {err}")))?;

    row.map(IssueSummaryView::from).ok_or(AppError::NotFound)
}

async fn load_sample_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    project_id: i64,
    issue_id: i64,
    preferred_event_id: Option<&str>,
) -> Result<Option<IssueEventView>, AppError> {
    if let Some(sample_id) = preferred_event_id.filter(|s| !s.is_empty()) {
        let exact: Option<IssueEventDbRow> = sqlx::query_as(
            "SELECT event_id, issue_id, project_id, received_at, environment, release, \
                    level, platform, exception_type, exception_value, stack_frames, \
                    tags, payload, is_sample \
             FROM events \
             WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND event_id = $4 \
             ORDER BY received_at DESC \
             LIMIT 1",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(issue_id)
        .bind(sample_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("load exact sample event: {err}")))?;

        if let Some(row) = exact {
            return Ok(Some(IssueEventView::from(row)));
        }
    }

    let fallback: Option<IssueEventDbRow> = sqlx::query_as(
        "SELECT event_id, issue_id, project_id, received_at, environment, release, \
                level, platform, exception_type, exception_value, stack_frames, \
                tags, payload, is_sample \
         FROM events \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 \
         ORDER BY is_sample DESC, received_at DESC \
         LIMIT 1",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(issue_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load fallback sample event: {err}")))?;

    Ok(fallback.map(IssueEventView::from))
}

async fn query_counter_series_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    project_id: i64,
    issue_id: i64,
    interval: StatsInterval,
    since: DateTime<Utc>,
) -> Result<Vec<CounterBucketPoint>, AppError> {
    let rows: Vec<CounterBucketDbRow> = match interval {
        StatsInterval::Hourly => sqlx::query_as(
            "SELECT hour_bucket AS bucket, event_count \
             FROM issue_hourly_counters \
             WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND hour_bucket >= $4 \
             ORDER BY hour_bucket ASC \
             LIMIT 2160",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(issue_id)
        .bind(since)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("query hourly counters: {err}")))?,

        StatsInterval::Daily => sqlx::query_as(
            "SELECT (date_trunc('day', hour_bucket AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS bucket, \
                    COALESCE(SUM(event_count)::BIGINT, 0) AS event_count \
             FROM issue_hourly_counters \
             WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND hour_bucket >= $4 \
             GROUP BY 1 \
             ORDER BY 1 ASC \
             LIMIT 366",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(issue_id)
        .bind(since)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("query daily counters: {err}")))?,
    };

    Ok(rows
        .into_iter()
        .map(|r| CounterBucketPoint {
            bucket: r.bucket,
            event_count: r.event_count,
        })
        .collect())
}
