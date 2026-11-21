use chrono::{DateTime, Utc};
use maxi_loop_auth::model::Role;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::model::IssueStatus;
use crate::service::{IssueScope, load_scoped_issue_in_tx, verify_tenant_slug_in_tx};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueNoteView {
    pub id: i64,
    pub issue_id: i64,
    pub project_id: i64,
    pub author_user_id: Option<i64>,
    pub author_email: String,
    pub source: String,
    pub body: String,
    pub commit_ref: Option<String>,
    pub status_change: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateIssueNoteInput {
    pub body: String,
    #[serde(default)]
    pub commit_ref: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NoteActor<'a> {
    pub user_id: i64,
    pub email: &'a str,
    pub source: &'a str,
}

#[derive(FromRow)]
struct IssueNoteDbRow {
    id: i64,
    issue_id: i64,
    project_id: i64,
    author_user_id: Option<i64>,
    author_email: String,
    source: String,
    body: String,
    commit_ref: Option<String>,
    status_change: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<IssueNoteDbRow> for IssueNoteView {
    fn from(r: IssueNoteDbRow) -> Self {
        Self {
            id: r.id,
            issue_id: r.issue_id,
            project_id: r.project_id,
            author_user_id: r.author_user_id,
            author_email: r.author_email,
            source: r.source,
            body: r.body,
            commit_ref: r.commit_ref,
            status_change: r.status_change,
            created_at: r.created_at,
        }
    }
}

pub fn validate_note_body(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 4000 {
        return Err(AppError::Validation(
            "note body must be between 1 and 4000 characters".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn validate_commit_ref(raw: Option<&str>) -> Result<Option<String>, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) => {
            if v.len() > 128 || v.chars().any(char::is_control) {
                return Err(AppError::Validation(
                    "commit_ref must be 1..=128 printable characters".to_owned(),
                ));
            }
            Ok(Some(v.to_owned()))
        }
    }
}

pub struct NewIssueNote<'a> {
    pub tenant_id: i64,
    pub project_id: i64,
    pub issue_id: i64,
    pub actor: &'a NoteActor<'a>,
    pub body: &'a str,
    pub commit_ref: Option<&'a str>,
    pub status_change: Option<IssueStatus>,
}

pub async fn insert_issue_note_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    note: NewIssueNote<'_>,
) -> Result<IssueNoteView, AppError> {
    let source = match note.actor.source {
        "mcp" => "mcp",
        _ => "ui",
    };

    let row: IssueNoteDbRow = sqlx::query_as(
        "INSERT INTO issue_notes (
             tenant_id, project_id, issue_id, author_user_id, author_email,
             source, body, commit_ref, status_change
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING id, issue_id, project_id, author_user_id, author_email,
                   source, body, commit_ref, status_change, created_at",
    )
    .bind(note.tenant_id)
    .bind(note.project_id)
    .bind(note.issue_id)
    .bind(note.actor.user_id)
    .bind(note.actor.email)
    .bind(source)
    .bind(note.body)
    .bind(note.commit_ref)
    .bind(note.status_change.map(IssueStatus::as_str))
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert issue note: {err}")))?;

    Ok(IssueNoteView::from(row))
}

pub async fn list_issue_notes(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    scope: IssueScope<'_>,
    issue_id: i64,
    limit: Option<u32>,
) -> Result<Vec<IssueNoteView>, AppError> {
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let max_rows = i64::from(limit.unwrap_or(50).clamp(1, 200));

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    let rows: Vec<IssueNoteDbRow> = sqlx::query_as(
        "SELECT id, issue_id, project_id, author_user_id, author_email,
                source, body, commit_ref, status_change, created_at
         FROM issue_notes
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3
         ORDER BY created_at DESC, id DESC
         LIMIT $4",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(issue.project_id)
    .bind(issue.id)
    .bind(max_rows)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list issue notes: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list_issue_notes: {err}")))?;

    Ok(rows.into_iter().map(IssueNoteView::from).collect())
}

pub async fn add_issue_note(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    issue_id: i64,
    actor: NoteActor<'_>,
    input: CreateIssueNoteInput,
) -> Result<IssueNoteView, AppError> {
    if !role.can_triage_issues() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let body = validate_note_body(&input.body)?;
    let commit_ref = validate_commit_ref(input.commit_ref.as_deref())?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    let created = insert_issue_note_in_tx(
        &mut tx,
        NewIssueNote {
            tenant_id: tenant_ctx.tenant_id(),
            project_id: issue.project_id,
            issue_id: issue.id,
            actor: &actor,
            body: &body,
            commit_ref: commit_ref.as_deref(),
            status_change: None,
        },
    )
    .await?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit add_issue_note: {err}")))?;

    Ok(created)
}
