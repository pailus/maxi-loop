//! F024: issue assignment.
//!
//! Who may be assigned is decided by the SQL function `user_can_triage_project` (migration 0015),
//! which is also used by the trigger that unassigns issues when a membership is removed or
//! downgraded. Keeping the rule in one place means a future per-project access rule only has to
//! change that function.

use maxi_loop_auth::model::Role;
use maxi_loop_platform::config::is_valid_email;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use tracing::info;

use crate::model::{IssueAssigneeView, IssueSummaryView};
use crate::notes::{IssueNoteView, NewIssueNote, NoteActor, insert_issue_note_in_tx};
use crate::service::{IssueScope, load_scoped_issue_in_tx, verify_tenant_slug_in_tx};

const MAX_ASSIGN_NOTE_LEN: usize = 3000;
const MAX_ASSIGNABLE_MEMBERS: i64 = 500;

/// Who to assign. HTTP callers pass a user id; MCP callers may pass an email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssigneeTarget {
    Unassign,
    UserId(i64),
    Email(String),
}

impl AssigneeTarget {
    /// Parse the MCP form: `"me"`, an email, or empty/`null` to unassign.
    pub fn parse_mcp(raw: Option<&str>, me_user_id: i64) -> Result<Self, AppError> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(Self::Unassign),
            Some(v) if v.eq_ignore_ascii_case("me") => Ok(Self::UserId(me_user_id)),
            Some(v) => {
                let email = v.to_ascii_lowercase();
                if !is_valid_email(&email) {
                    return Err(AppError::Validation(
                        "assignee must be 'me', a member's email, or null to unassign".to_owned(),
                    ));
                }
                Ok(Self::Email(email))
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AssignIssueInput {
    /// `null` (or omitted) unassigns the issue.
    #[serde(default)]
    pub assignee_user_id: Option<i64>,
    #[serde(default)]
    pub note: Option<String>,
}

impl AssignIssueInput {
    pub fn target(&self) -> Result<AssigneeTarget, AppError> {
        match self.assignee_user_id {
            None => Ok(AssigneeTarget::Unassign),
            Some(id) if id > 0 => Ok(AssigneeTarget::UserId(id)),
            Some(_) => Err(AppError::Validation(
                "assignee_user_id must be a positive integer".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AssignIssueResult {
    pub issue: IssueSummaryView,
    /// Timeline entry recording the change; `None` when the assignee did not change.
    pub note: Option<IssueNoteView>,
    pub changed: bool,
}

#[derive(FromRow)]
struct MemberRow {
    id: i64,
    email: String,
    name: String,
}

impl From<MemberRow> for IssueAssigneeView {
    fn from(r: MemberRow) -> Self {
        Self {
            user_id: r.id,
            email: r.email,
            name: r.name,
        }
    }
}

fn validate_assign_note(raw: Option<&str>) -> Result<Option<String>, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) if v.chars().count() <= MAX_ASSIGN_NOTE_LEN => Ok(Some(v.to_owned())),
        Some(_) => Err(AppError::Validation(format!(
            "note must be at most {MAX_ASSIGN_NOTE_LEN} characters"
        ))),
    }
}

/// Assign (or unassign) an issue. The caller must be allowed to triage the issue's project;
/// the assignee must be an active member who can triage it too.
#[allow(clippy::too_many_arguments)] // mirrors the other actor-recording issue mutations
pub async fn assign_issue(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    issue_id: i64,
    actor: NoteActor<'_>,
    target: AssigneeTarget,
    note: Option<&str>,
) -> Result<AssignIssueResult, AppError> {
    if !role.can_triage_issues() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let extra_note = validate_assign_note(note)?;
    let tenant_id = tenant_ctx.tenant_id();

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let existing = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    // `users` is only visible under tenant RLS for members of this tenant, so unknown or
    // foreign users resolve to "not assignable" without revealing whether they exist.
    let new_assignee: Option<IssueAssigneeView> = match &target {
        AssigneeTarget::Unassign => None,
        AssigneeTarget::UserId(id) => {
            let row: Option<MemberRow> = sqlx::query_as(
                "SELECT u.id, u.email, u.name FROM users u
                 WHERE u.id = $1 AND user_can_triage_project($2, u.id, $3)",
            )
            .bind(id)
            .bind(tenant_id)
            .bind(existing.project_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("resolve assignee by id: {err}")))?;
            Some(row.map(Into::into).ok_or_else(not_assignable)?)
        }
        AssigneeTarget::Email(email) => {
            let row: Option<MemberRow> = sqlx::query_as(
                "SELECT u.id, u.email, u.name FROM users u
                 WHERE u.email = $1 AND user_can_triage_project($2, u.id, $3)",
            )
            .bind(email)
            .bind(tenant_id)
            .bind(existing.project_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("resolve assignee by email: {err}")))?;
            Some(row.map(Into::into).ok_or_else(not_assignable)?)
        }
    };

    let current_id = existing.assignee.as_ref().map(|a| a.user_id);
    let new_id = new_assignee.as_ref().map(|a| a.user_id);
    if current_id == new_id {
        tx.commit()
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("commit noop assign: {err}")))?;
        return Ok(AssignIssueResult {
            issue: existing,
            note: None,
            changed: false,
        });
    }

    sqlx::query(
        "UPDATE issues SET assignee_user_id = $1, updated_at = NOW()
         WHERE tenant_id = $2 AND project_id = $3 AND id = $4",
    )
    .bind(new_id)
    .bind(tenant_id)
    .bind(existing.project_id)
    .bind(existing.id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update issue assignee: {err}")))?;

    let headline = match (&new_assignee, &existing.assignee) {
        (Some(a), _) => format!("Assigned to {} <{}>", a.name, a.email),
        (None, Some(prev)) => format!("Unassigned (was {})", prev.email),
        (None, None) => "Unassigned".to_owned(),
    };
    let body = match extra_note {
        Some(extra) => format!("{headline}\n\n{extra}"),
        None => headline,
    };
    let created_note = insert_issue_note_in_tx(
        &mut tx,
        NewIssueNote {
            tenant_id,
            project_id: existing.project_id,
            issue_id: existing.id,
            actor: &actor,
            body: &body,
            commit_ref: None,
            status_change: None,
        },
    )
    .await?;

    let updated = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit assign_issue: {err}")))?;

    info!(
        tenant_id,
        issue_id = existing.id,
        actor_user_id = actor.user_id,
        source = actor.source,
        assignee_user_id = ?new_id,
        "issue assignee changed"
    );

    Ok(AssignIssueResult {
        issue: updated,
        note: Some(created_note),
        changed: true,
    })
}

fn not_assignable() -> AppError {
    AppError::Validation(
        "assignee must be an active member who can triage issues in this project".to_owned(),
    )
}

/// Members who can be assigned to the given issue. Restricted to callers who can triage it
/// (they are the only ones who can assign), so member emails are not exposed to viewers.
pub async fn list_assignable_members(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    issue_id: i64,
) -> Result<Vec<IssueAssigneeView>, AppError> {
    if !role.can_triage_issues() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    let rows: Vec<MemberRow> = sqlx::query_as(
        "SELECT u.id, u.email, u.name FROM users u
         WHERE u.id IN (SELECT m.user_id FROM memberships m WHERE m.tenant_id = $1)
           AND user_can_triage_project($1, u.id, $2)
         ORDER BY lower(u.name), u.email
         LIMIT $3",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(issue.project_id)
    .bind(MAX_ASSIGNABLE_MEMBERS)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list assignable members: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit assignable members: {err}")))?;

    Ok(rows.into_iter().map(Into::into).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mcp_assignee_targets() {
        assert_eq!(
            AssigneeTarget::parse_mcp(None, 7).unwrap(),
            AssigneeTarget::Unassign
        );
        assert_eq!(
            AssigneeTarget::parse_mcp(Some("  "), 7).unwrap(),
            AssigneeTarget::Unassign
        );
        assert_eq!(
            AssigneeTarget::parse_mcp(Some("ME"), 7).unwrap(),
            AssigneeTarget::UserId(7)
        );
        assert_eq!(
            AssigneeTarget::parse_mcp(Some(" Dev@Example.com "), 7).unwrap(),
            AssigneeTarget::Email("dev@example.com".to_owned())
        );
        assert!(AssigneeTarget::parse_mcp(Some("not-an-email"), 7).is_err());
    }

    #[test]
    fn validates_http_input_and_note() {
        let input = AssignIssueInput {
            assignee_user_id: Some(-1),
            note: None,
        };
        assert!(input.target().is_err());
        assert!(validate_assign_note(Some(&"x".repeat(3001))).is_err());
        assert_eq!(validate_assign_note(Some("  ")).unwrap(), None);
    }
}
