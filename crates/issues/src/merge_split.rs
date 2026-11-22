use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use maxi_loop_auth::model::Role;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::model::IssueSummaryView;
use crate::notes::{NewIssueNote, NoteActor, insert_issue_note_in_tx};
use crate::service::{IssueScope, load_scoped_issue_in_tx, verify_tenant_slug_in_tx};

const MAX_MERGE_SOURCES: usize = 50;
const MAX_SPLIT_FINGERPRINTS: usize = 20;
const MAX_SPLIT_EVENT_IDS: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueFingerprintView {
    pub fingerprint: String,
    pub original_title: String,
    pub original_culprit: String,
    pub is_primary: bool,
    pub merged_at: DateTime<Utc>,
    pub stored_event_count: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MergeIssuesInput {
    pub target_issue_id: i64,
    pub source_issue_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeIssuesResponse {
    pub target_issue: IssueSummaryView,
    pub merged_issue_ids: Vec<i64>,
    pub merged_fingerprints: Vec<IssueFingerprintView>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SplitIssueInput {
    #[serde(default)]
    pub fingerprints: Option<Vec<String>>,
    #[serde(default)]
    pub event_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitIssueResponse {
    pub source_issue: IssueSummaryView,
    pub created_issues: Vec<IssueSummaryView>,
}

#[derive(FromRow)]
struct IssueFingerprintDbRow {
    fingerprint: String,
    original_title: String,
    original_culprit: String,
    is_primary: bool,
    merged_at: DateTime<Utc>,
    stored_event_count: i64,
}

#[derive(FromRow)]
struct LockedIssueRow {
    id: i64,
    fingerprint: String,
    title: String,
    exception_type: String,
    exception_value: String,
    culprit: String,
    level: String,
    is_regressed: bool,
    event_count: i64,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    last_environment: String,
    first_release: Option<String>,
    last_release: Option<String>,
    sample_event_id: Option<String>,
    issue_type: String,
}

#[derive(FromRow)]
struct SplitEventMetaRow {
    event_id: String,
    received_at: DateTime<Utc>,
    environment: String,
    release: Option<String>,
    level: String,
    exception_type: String,
    exception_value: String,
    fingerprint: Option<String>,
}

pub(crate) async fn list_issue_fingerprints_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    project_id: i64,
    issue_id: i64,
) -> Result<Vec<IssueFingerprintView>, AppError> {
    let rows: Vec<IssueFingerprintDbRow> = sqlx::query_as(
        "SELECT f.fingerprint, f.original_title, f.original_culprit, f.is_primary, f.merged_at, \
                COALESCE(( \
                    SELECT COUNT(*)::BIGINT \
                    FROM events e \
                    WHERE e.tenant_id = f.tenant_id \
                      AND e.project_id = f.project_id \
                      AND e.issue_id = f.issue_id \
                      AND e.fingerprint = f.fingerprint \
                ), 0) AS stored_event_count \
         FROM issue_fingerprints f \
         WHERE f.tenant_id = $1 AND f.project_id = $2 AND f.issue_id = $3 \
         ORDER BY f.is_primary DESC, f.merged_at ASC, f.fingerprint ASC",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(issue_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("list issue fingerprints: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| IssueFingerprintView {
            fingerprint: r.fingerprint,
            original_title: r.original_title,
            original_culprit: r.original_culprit,
            is_primary: r.is_primary,
            merged_at: r.merged_at,
            stored_event_count: r.stored_event_count,
        })
        .collect())
}

fn validate_merge_input(input: &MergeIssuesInput) -> Result<(i64, Vec<i64>), AppError> {
    if input.target_issue_id <= 0 {
        return Err(AppError::Validation(
            "target_issue_id must be a positive integer".to_owned(),
        ));
    }
    if input.source_issue_ids.is_empty() {
        return Err(AppError::Validation(
            "source_issue_ids must contain at least one issue ID".to_owned(),
        ));
    }
    let mut unique_sources = BTreeSet::new();
    for &id in &input.source_issue_ids {
        if id <= 0 {
            return Err(AppError::Validation(
                "all source_issue_ids must be positive integers".to_owned(),
            ));
        }
        if id == input.target_issue_id {
            return Err(AppError::Validation(
                "source_issue_ids cannot include target_issue_id".to_owned(),
            ));
        }
        unique_sources.insert(id);
    }
    if unique_sources.len() > MAX_MERGE_SOURCES {
        return Err(AppError::Validation(format!(
            "cannot merge more than {MAX_MERGE_SOURCES} issues at once"
        )));
    }
    Ok((input.target_issue_id, unique_sources.into_iter().collect()))
}

#[derive(Debug, PartialEq, Eq)]
enum ValidatedSplitMode {
    Fingerprints(Vec<String>),
    EventIds(Vec<String>),
}

fn validate_split_input(input: &SplitIssueInput) -> Result<ValidatedSplitMode, AppError> {
    let fps: Vec<String> = input
        .fingerprints
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let evs: Vec<String> = input
        .event_ids
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    match (!fps.is_empty(), !evs.is_empty()) {
        (true, true) => Err(AppError::Validation(
            "provide either fingerprints or event_ids, not both".to_owned(),
        )),
        (false, false) => Err(AppError::Validation(
            "must provide at least one fingerprint or event_id to split".to_owned(),
        )),
        (true, false) => {
            let mut dedup = Vec::new();
            let mut seen = BTreeSet::new();
            for fp in fps {
                if fp.len() > 256 || fp.chars().any(char::is_control) {
                    return Err(AppError::Validation("invalid fingerprint".to_owned()));
                }
                if seen.insert(fp.clone()) {
                    dedup.push(fp);
                }
            }
            if dedup.len() > MAX_SPLIT_FINGERPRINTS {
                return Err(AppError::Validation(format!(
                    "cannot split more than {MAX_SPLIT_FINGERPRINTS} fingerprints at once"
                )));
            }
            Ok(ValidatedSplitMode::Fingerprints(dedup))
        }
        (false, true) => {
            let mut dedup = Vec::new();
            let mut seen = BTreeSet::new();
            for ev_id in evs {
                if ev_id.len() > 128 || ev_id.chars().any(char::is_control) {
                    return Err(AppError::Validation("invalid event_id".to_owned()));
                }
                if seen.insert(ev_id.clone()) {
                    dedup.push(ev_id);
                }
            }
            if dedup.len() > MAX_SPLIT_EVENT_IDS {
                return Err(AppError::Validation(format!(
                    "cannot split more than {MAX_SPLIT_EVENT_IDS} events at once"
                )));
            }
            Ok(ValidatedSplitMode::EventIds(dedup))
        }
    }
}

/// Manually merge one or more source issues into a target issue within the same tenant & project (`F-ISS-9`).
/// Re-parents events, hourly counters, releases, notes, alert deliveries, and records secondary
/// fingerprint aliases in `issue_fingerprints` so future incoming events automatically route to target.
pub async fn merge_issues(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    actor: NoteActor<'_>,
    input: MergeIssuesInput,
) -> Result<MergeIssuesResponse, AppError> {
    if !role.can_triage_issues() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    let (target_id, source_ids) = validate_merge_input(&input)?;

    let mut all_ids = source_ids.clone();
    all_ids.push(target_id);
    all_ids.sort_unstable();

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;

    // Verify every involved issue is accessible inside the caller's scope (tenant/workspace/project).
    let target_scoped = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, target_id).await?;
    for &src_id in &source_ids {
        let src_scoped = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, src_id).await?;
        if src_scoped.project_id != target_scoped.project_id {
            return Err(AppError::Validation(
                "cannot merge issues from different projects".to_owned(),
            ));
        }
    }

    let tenant_id = tenant_ctx.tenant_id();
    let project_id = target_scoped.project_id;

    // Lock all involved issue rows in deterministic ascending ID order to avoid deadlocks.
    let locked_rows: Vec<LockedIssueRow> = sqlx::query_as(
        "SELECT id, fingerprint, title, exception_type, exception_value, culprit, \
                level, is_regressed, event_count, first_seen, last_seen, last_environment, \
                first_release, last_release, sample_event_id, issue_type \
         FROM issues \
         WHERE tenant_id = $1 AND project_id = $2 AND id = ANY($3) \
         ORDER BY id ASC \
         FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(&all_ids)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lock issues for merge: {err}")))?;

    if locked_rows.len() != all_ids.len() {
        return Err(AppError::NotFound);
    }

    let target_row = locked_rows
        .iter()
        .find(|r| r.id == target_id)
        .ok_or(AppError::NotFound)?;
    let source_rows: Vec<&LockedIssueRow> =
        locked_rows.iter().filter(|r| r.id != target_id).collect();

    // An error and a performance problem (or two different performance problem kinds) are
    // never the same root cause, and the target keeps only one `issue_type`.
    if source_rows
        .iter()
        .any(|r| r.issue_type != target_row.issue_type)
    {
        return Err(AppError::Validation(
            "cannot merge issues of different issue types".to_owned(),
        ));
    }

    // Preserve original fingerprints on any pre-0008 stored events before moving them.
    for row in &locked_rows {
        sqlx::query(
            "UPDATE events \
             SET fingerprint = $4 \
             WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND fingerprint IS NULL",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(row.id)
        .bind(&row.fingerprint)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("backfill event fingerprint: {err}")))?;
    }

    // 1. Re-parent stored events from source issues to target issue.
    sqlx::query(
        "UPDATE events \
         SET issue_id = $3 \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = ANY($4)",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("reparent merged events: {err}")))?;

    // 2. Merge hourly counters into target issue.
    sqlx::query(
        "INSERT INTO issue_hourly_counters (tenant_id, project_id, issue_id, hour_bucket, event_count) \
         SELECT tenant_id, project_id, $3, hour_bucket, SUM(event_count)::BIGINT \
         FROM issue_hourly_counters \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = ANY($4) \
         GROUP BY tenant_id, project_id, hour_bucket \
         ON CONFLICT (tenant_id, project_id, issue_id, hour_bucket) \
         DO UPDATE SET event_count = issue_hourly_counters.event_count + EXCLUDED.event_count",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("merge issue_hourly_counters: {err}")))?;

    // 3. Merge issue_releases into target issue.
    sqlx::query(
        "INSERT INTO issue_releases (tenant_id, project_id, issue_id, release_version, first_seen, last_seen, event_count) \
         SELECT tenant_id, project_id, $3, release_version, MIN(first_seen), MAX(last_seen), SUM(event_count)::BIGINT \
         FROM issue_releases \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = ANY($4) \
         GROUP BY tenant_id, project_id, release_version \
         ON CONFLICT (tenant_id, project_id, issue_id, release_version) \
         DO UPDATE SET \
             first_seen = LEAST(issue_releases.first_seen, EXCLUDED.first_seen), \
             last_seen = GREATEST(issue_releases.last_seen, EXCLUDED.last_seen), \
             event_count = issue_releases.event_count + EXCLUDED.event_count",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("merge issue_releases: {err}")))?;

    // 4. Re-parent issue_notes and alert_deliveries so history is never lost on merge.
    sqlx::query(
        "UPDATE issue_notes \
         SET issue_id = $3 \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = ANY($4)",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("reparent merged issue_notes: {err}")))?;

    sqlx::query(
        "UPDATE alert_deliveries \
         SET issue_id = $3 \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = ANY($4)",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("reparent merged alert_deliveries: {err}")))?;

    // 5. Re-parent any existing secondary fingerprints owned by source issues and record each
    //    source issue's own fingerprint as a secondary alias routing to `target_id`.
    sqlx::query(
        "UPDATE issue_fingerprints \
         SET issue_id = $3, is_primary = FALSE \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = ANY($4)",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("reparent issue_fingerprints: {err}")))?;

    for src in &source_rows {
        sqlx::query(
            "INSERT INTO issue_fingerprints ( \
                tenant_id, project_id, fingerprint, issue_id, original_title, original_culprit, is_primary \
             ) VALUES ($1, $2, $3, $4, $5, $6, FALSE) \
             ON CONFLICT (tenant_id, project_id, fingerprint) \
             DO UPDATE SET \
                issue_id = EXCLUDED.issue_id, \
                original_title = EXCLUDED.original_title, \
                original_culprit = EXCLUDED.original_culprit, \
                is_primary = FALSE, \
                merged_at = NOW()",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(&src.fingerprint)
        .bind(target_id)
        .bind(&src.title)
        .bind(&src.culprit)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("insert merged fingerprint alias: {err}")))?;
    }

    // 6. Compute combined totals and metadata for the target issue.
    let combined_event_count: i64 = locked_rows.iter().map(|r| r.event_count).sum();
    let combined_is_regressed = locked_rows.iter().any(|r| r.is_regressed);
    let combined_first_seen = locked_rows
        .iter()
        .map(|r| r.first_seen)
        .min()
        .unwrap_or(target_row.first_seen);
    let combined_last_seen = locked_rows
        .iter()
        .map(|r| r.last_seen)
        .max()
        .unwrap_or(target_row.last_seen);
    let combined_first_release = locked_rows
        .iter()
        .filter(|r| r.first_release.is_some())
        .min_by_key(|r| r.first_seen)
        .and_then(|r| r.first_release.clone())
        .or_else(|| target_row.first_release.clone());
    let combined_last_release = locked_rows
        .iter()
        .filter(|r| r.last_release.is_some())
        .max_by_key(|r| r.last_seen)
        .and_then(|r| r.last_release.clone())
        .or_else(|| target_row.last_release.clone());
    let combined_sample_event_id = target_row
        .sample_event_id
        .clone()
        .or_else(|| source_rows.iter().find_map(|r| r.sample_event_id.clone()));

    let stored_full_events: i32 = sqlx::query_scalar(
        "SELECT COUNT(*)::INTEGER \
         FROM events \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("count merged stored events: {err}")))?;

    // 7. Delete source issues (child rows were already moved; residual duplicate counters/releases cascade cleanly).
    sqlx::query(
        "DELETE FROM issues \
         WHERE tenant_id = $1 AND project_id = $2 AND id = ANY($3)",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("delete merged source issues: {err}")))?;

    // 8. Update target issue metadata.
    sqlx::query(
        "UPDATE issues \
         SET event_count = $4, \
             stored_full_events = $5, \
             is_regressed = $6, \
             first_seen = $7, \
             last_seen = $8, \
             first_release = $9, \
             last_release = $10, \
             sample_event_id = COALESCE($11, sample_event_id), \
             updated_at = NOW() \
         WHERE tenant_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(target_id)
    .bind(combined_event_count)
    .bind(stored_full_events)
    .bind(combined_is_regressed)
    .bind(combined_first_seen)
    .bind(combined_last_seen)
    .bind(&combined_first_release)
    .bind(&combined_last_release)
    .bind(&combined_sample_event_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update merged target issue: {err}")))?;

    let merged_list_str = source_ids
        .iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ");
    let note_body = format!(
        "Merged {} issue(s) ({merged_list_str}) into issue #{target_id}.",
        source_ids.len()
    );
    insert_issue_note_in_tx(
        &mut tx,
        NewIssueNote {
            tenant_id,
            project_id,
            issue_id: target_id,
            actor: &actor,
            body: &note_body,
            commit_ref: None,
            status_change: None,
        },
    )
    .await?;

    let target_issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, target_id).await?;
    let merged_fingerprints =
        list_issue_fingerprints_in_tx(&mut tx, tenant_id, project_id, target_id).await?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit merge_issues: {err}")))?;

    Ok(MergeIssuesResponse {
        target_issue,
        merged_issue_ids: source_ids,
        merged_fingerprints,
    })
}

/// Manually split merged fingerprints or specific stored events out of an issue into standalone
/// issue(s) within the same tenant & project (`F-ISS-9`).
pub async fn split_issue(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    role: Role,
    scope: IssueScope<'_>,
    issue_id: i64,
    actor: NoteActor<'_>,
    input: SplitIssueInput,
) -> Result<SplitIssueResponse, AppError> {
    if !role.can_triage_issues() {
        return Err(AppError::Forbidden("insufficient permissions"));
    }
    if issue_id <= 0 {
        return Err(AppError::Validation(
            "issue_id must be a positive integer".to_owned(),
        ));
    }
    let mode = validate_split_input(&input)?;

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    verify_tenant_slug_in_tx(&mut tx, tenant_ctx, scope.tenant_slug).await?;
    let scoped = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;

    let tenant_id = tenant_ctx.tenant_id();
    let project_id = scoped.project_id;

    let source_row: LockedIssueRow = sqlx::query_as(
        "SELECT id, fingerprint, title, exception_type, exception_value, culprit, \
                level, is_regressed, event_count, first_seen, last_seen, last_environment, \
                first_release, last_release, sample_event_id, issue_type \
         FROM issues \
         WHERE tenant_id = $1 AND project_id = $2 AND id = $3 \
         FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(issue_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lock issue for split: {err}")))?
    .ok_or(AppError::NotFound)?;

    let mut created_issue_ids: Vec<i64> = Vec::new();
    let mut total_moved_events: i64 = 0;

    match mode {
        ValidatedSplitMode::Fingerprints(fps) => {
            let existing_aliases =
                list_issue_fingerprints_in_tx(&mut tx, tenant_id, project_id, issue_id).await?;
            let alias_map: BTreeMap<String, IssueFingerprintView> = existing_aliases
                .into_iter()
                .filter(|f| !f.is_primary)
                .map(|f| (f.fingerprint.clone(), f))
                .collect();

            for fp in &fps {
                if !alias_map.contains_key(fp) {
                    return Err(AppError::Validation(format!(
                        "fingerprint '{fp}' is not a merged secondary fingerprint on this issue"
                    )));
                }
            }

            for fp in fps {
                let alias = &alias_map[&fp];
                sqlx::query(
                    "DELETE FROM issue_fingerprints \
                     WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND fingerprint = $4",
                )
                .bind(tenant_id)
                .bind(project_id)
                .bind(issue_id)
                .bind(&fp)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::DatabaseQuery(format!("delete split alias: {err}")))?;

                let moved_events: Vec<SplitEventMetaRow> = sqlx::query_as(
                    "SELECT event_id, received_at, environment, release, level, \
                            exception_type, exception_value, fingerprint \
                     FROM events \
                     WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND fingerprint = $4 \
                     ORDER BY received_at ASC, event_id ASC",
                )
                .bind(tenant_id)
                .bind(project_id)
                .bind(issue_id)
                .bind(&fp)
                .fetch_all(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!("load events for fingerprint split: {err}"))
                })?;

                let new_id = create_split_issue_and_move_events(
                    &mut tx,
                    tenant_id,
                    project_id,
                    &source_row,
                    &fp,
                    Some(&alias.original_title),
                    Some(&alias.original_culprit),
                    alias.merged_at,
                    &moved_events,
                    &actor,
                )
                .await?;

                total_moved_events += i64::try_from(moved_events.len()).unwrap_or(0);
                created_issue_ids.push(new_id);
            }
        }
        ValidatedSplitMode::EventIds(ev_ids) => {
            let moved_events: Vec<SplitEventMetaRow> = sqlx::query_as(
                "SELECT event_id, received_at, environment, release, level, \
                        exception_type, exception_value, fingerprint \
                 FROM events \
                 WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND event_id = ANY($4) \
                 ORDER BY received_at ASC, event_id ASC",
            )
            .bind(tenant_id)
            .bind(project_id)
            .bind(issue_id)
            .bind(&ev_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("load events for event_id split: {err}"))
            })?;

            if moved_events.len() != ev_ids.len() {
                return Err(AppError::Validation(
                    "one or more event_ids do not belong to this issue".to_owned(),
                ));
            }

            let total_stored: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)::BIGINT \
                 FROM events \
                 WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3",
            )
            .bind(tenant_id)
            .bind(project_id)
            .bind(issue_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("count source stored events: {err}")))?;

            let moved_len = i64::try_from(moved_events.len()).unwrap_or(0);
            if total_stored - moved_len <= 0 && source_row.event_count <= moved_len {
                return Err(AppError::Validation(
                    "cannot split all events out of an issue; at least one event must remain"
                        .to_owned(),
                ));
            }

            // If all moved events share a single distinct fingerprint that is a merged alias on
            // this issue, reuse it cleanly; otherwise synthesize a unique `split:<sha256>` key.
            let distinct_fps: BTreeSet<&str> = moved_events
                .iter()
                .filter_map(|e| e.fingerprint.as_deref())
                .collect();
            let chosen_fp = if distinct_fps.len() == 1 {
                let candidate = *distinct_fps.iter().next().unwrap();
                if candidate != source_row.fingerprint {
                    let exists_in_issues: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM issues WHERE tenant_id = $1 AND project_id = $2 AND fingerprint = $3)",
                    )
                    .bind(tenant_id)
                    .bind(project_id)
                    .bind(candidate)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|err| {
                        AppError::DatabaseQuery(format!("check split fingerprint uniqueness: {err}"))
                    })?;

                    if !exists_in_issues {
                        sqlx::query(
                            "DELETE FROM issue_fingerprints \
                             WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND fingerprint = $4",
                        )
                        .bind(tenant_id)
                        .bind(project_id)
                        .bind(issue_id)
                        .bind(candidate)
                        .execute(&mut *tx)
                        .await
                        .map_err(|err| {
                            AppError::DatabaseQuery(format!("remove reused alias on split: {err}"))
                        })?;
                        candidate.to_owned()
                    } else {
                        synthesize_split_fingerprint(
                            tenant_id,
                            project_id,
                            issue_id,
                            &moved_events[0].event_id,
                        )
                    }
                } else {
                    synthesize_split_fingerprint(
                        tenant_id,
                        project_id,
                        issue_id,
                        &moved_events[0].event_id,
                    )
                }
            } else {
                synthesize_split_fingerprint(
                    tenant_id,
                    project_id,
                    issue_id,
                    &moved_events[0].event_id,
                )
            };

            let new_id = create_split_issue_and_move_events(
                &mut tx,
                tenant_id,
                project_id,
                &source_row,
                &chosen_fp,
                None,
                None,
                Utc::now(),
                &moved_events,
                &actor,
            )
            .await?;

            total_moved_events += moved_len;
            created_issue_ids.push(new_id);
        }
    }

    // Recalculate source issue stored_full_events, event_count, and sample_event_id.
    let remaining_stored: i32 = sqlx::query_scalar(
        "SELECT COUNT(*)::INTEGER \
         FROM events \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(issue_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("count remaining stored events: {err}")))?;

    let next_sample_event_id: Option<String> = sqlx::query_scalar(
        "SELECT event_id \
         FROM events \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 \
         ORDER BY is_sample DESC, received_at DESC \
         LIMIT 1",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(issue_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("refresh source sample_event_id: {err}")))?;

    let next_event_count =
        (source_row.event_count - total_moved_events).max(i64::from(remaining_stored));

    sqlx::query(
        "UPDATE issues \
         SET event_count = $4, \
             stored_full_events = $5, \
             sample_event_id = $6, \
             updated_at = NOW() \
         WHERE tenant_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(issue_id)
    .bind(next_event_count)
    .bind(remaining_stored)
    .bind(&next_sample_event_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("update source issue after split: {err}")))?;

    let created_list_str = created_issue_ids
        .iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source_note =
        format!("Split {total_moved_events} event(s) into new issue(s): {created_list_str}.");
    insert_issue_note_in_tx(
        &mut tx,
        NewIssueNote {
            tenant_id,
            project_id,
            issue_id,
            actor: &actor,
            body: &source_note,
            commit_ref: None,
            status_change: None,
        },
    )
    .await?;

    let source_issue = load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, issue_id).await?;
    let mut created_issues = Vec::with_capacity(created_issue_ids.len());
    for new_id in created_issue_ids {
        created_issues.push(load_scoped_issue_in_tx(&mut tx, tenant_ctx, &scope, new_id).await?);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit split_issue: {err}")))?;

    Ok(SplitIssueResponse {
        source_issue,
        created_issues,
    })
}

#[allow(clippy::too_many_arguments)]
async fn create_split_issue_and_move_events(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    project_id: i64,
    source_row: &LockedIssueRow,
    new_fingerprint: &str,
    fallback_title: Option<&str>,
    fallback_culprit: Option<&str>,
    fallback_ts: DateTime<Utc>,
    moved_events: &[SplitEventMetaRow],
    actor: &NoteActor<'_>,
) -> Result<i64, AppError> {
    let moved_count = i64::try_from(moved_events.len()).unwrap_or(0);
    let stored_full = i32::try_from(moved_events.len()).unwrap_or(0);

    let latest = moved_events.last();
    let earliest = moved_events.first();

    let (title, exception_type, exception_value, level, environment) = match latest {
        Some(ev) => {
            let constructed_title = build_split_title(&ev.exception_type, &ev.exception_value);
            let title = if constructed_title != "<unlabeled event>" {
                constructed_title
            } else {
                fallback_title
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&source_row.title)
                    .to_owned()
            };
            (
                title,
                ev.exception_type.clone(),
                ev.exception_value.clone(),
                ev.level.clone(),
                ev.environment.clone(),
            )
        }
        None => (
            fallback_title
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&source_row.title)
                .to_owned(),
            source_row.exception_type.clone(),
            source_row.exception_value.clone(),
            source_row.level.clone(),
            source_row.last_environment.clone(),
        ),
    };

    let culprit = fallback_culprit
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&source_row.culprit)
        .to_owned();
    let first_seen = earliest.map(|e| e.received_at).unwrap_or(fallback_ts);
    let last_seen = latest.map(|e| e.received_at).unwrap_or(fallback_ts);
    let first_release = moved_events.iter().find_map(|e| e.release.clone());
    let last_release = moved_events.iter().rev().find_map(|e| e.release.clone());
    let sample_event_id = latest.map(|e| e.event_id.clone());

    let new_issue_id: i64 = sqlx::query_scalar(
        "INSERT INTO issues ( \
            tenant_id, project_id, fingerprint, title, exception_type, exception_value, \
            culprit, level, status, is_regressed, event_count, stored_full_events, \
            first_seen, last_seen, last_environment, first_release, last_release, sample_event_id, \
            issue_type \
         ) VALUES ( \
            $1, $2, $3, $4, $5, $6, \
            $7, $8, 'unresolved', FALSE, $9, $10, \
            $11, $12, $13, $14, $15, $16, \
            $17 \
         ) RETURNING id",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(new_fingerprint)
    .bind(&title)
    .bind(&exception_type)
    .bind(&exception_value)
    .bind(&culprit)
    .bind(&level)
    .bind(moved_count)
    .bind(stored_full)
    .bind(first_seen)
    .bind(last_seen)
    .bind(&environment)
    .bind(&first_release)
    .bind(&last_release)
    .bind(&sample_event_id)
    .bind(&source_row.issue_type)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert split issue: {err}")))?;

    if !moved_events.is_empty() {
        let moved_ids: Vec<String> = moved_events.iter().map(|e| e.event_id.clone()).collect();
        sqlx::query(
            "UPDATE events \
             SET issue_id = $4, fingerprint = $5 \
             WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND event_id = ANY($6)",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(source_row.id)
        .bind(new_issue_id)
        .bind(new_fingerprint)
        .bind(&moved_ids)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("move events to split issue: {err}")))?;

        // Re-allocate hourly counters for the moved events.
        let mut hour_counts: BTreeMap<DateTime<Utc>, i64> = BTreeMap::new();
        let mut release_counts: BTreeMap<String, (DateTime<Utc>, DateTime<Utc>, i64)> =
            BTreeMap::new();
        for ev in moved_events {
            let bucket = truncate_to_hour_utc(ev.received_at);
            *hour_counts.entry(bucket).or_insert(0) += 1;

            if let Some(rel) = ev.release.as_deref() {
                let entry = release_counts.entry(rel.to_owned()).or_insert((
                    ev.received_at,
                    ev.received_at,
                    0,
                ));
                entry.0 = entry.0.min(ev.received_at);
                entry.1 = entry.1.max(ev.received_at);
                entry.2 += 1;
            }
        }

        for (bucket, count) in hour_counts {
            sqlx::query(
                "INSERT INTO issue_hourly_counters (tenant_id, project_id, issue_id, hour_bucket, event_count) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (tenant_id, project_id, issue_id, hour_bucket) \
                 DO UPDATE SET event_count = issue_hourly_counters.event_count + EXCLUDED.event_count",
            )
            .bind(tenant_id)
            .bind(project_id)
            .bind(new_issue_id)
            .bind(bucket)
            .bind(count)
            .execute(&mut **tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("insert split hourly counter: {err}"))
            })?;

            sqlx::query(
                "UPDATE issue_hourly_counters \
                 SET event_count = GREATEST(0, event_count - $5) \
                 WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND hour_bucket = $4",
            )
            .bind(tenant_id)
            .bind(project_id)
            .bind(source_row.id)
            .bind(bucket)
            .bind(count)
            .execute(&mut **tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("decrement source hourly counter: {err}"))
            })?;
        }

        for (rel_ver, (r_first, r_last, count)) in release_counts {
            // Only insert issue_releases when the release row exists (FK safety).
            let rel_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM releases WHERE tenant_id = $1 AND project_id = $2 AND version = $3)",
            )
            .bind(tenant_id)
            .bind(project_id)
            .bind(&rel_ver)
            .fetch_one(&mut **tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("check release exists: {err}")))?;

            if rel_exists {
                sqlx::query(
                    "INSERT INTO issue_releases (tenant_id, project_id, issue_id, release_version, first_seen, last_seen, event_count) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7) \
                     ON CONFLICT (tenant_id, project_id, issue_id, release_version) \
                     DO UPDATE SET \
                        first_seen = LEAST(issue_releases.first_seen, EXCLUDED.first_seen), \
                        last_seen = GREATEST(issue_releases.last_seen, EXCLUDED.last_seen), \
                        event_count = issue_releases.event_count + EXCLUDED.event_count",
                )
                .bind(tenant_id)
                .bind(project_id)
                .bind(new_issue_id)
                .bind(&rel_ver)
                .bind(r_first)
                .bind(r_last)
                .bind(count)
                .execute(&mut **tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!("insert split issue_releases: {err}"))
                })?;

                sqlx::query(
                    "UPDATE issue_releases \
                     SET event_count = GREATEST(0, event_count - $5) \
                     WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3 AND release_version = $4",
                )
                .bind(tenant_id)
                .bind(project_id)
                .bind(source_row.id)
                .bind(&rel_ver)
                .bind(count)
                .execute(&mut **tx)
                .await
                .map_err(|err| {
                    AppError::DatabaseQuery(format!("decrement source issue_releases: {err}"))
                })?;
            }
        }
    }

    let split_note = format!(
        "Split from issue #{} ({moved_count} event(s) moved).",
        source_row.id
    );
    insert_issue_note_in_tx(
        tx,
        NewIssueNote {
            tenant_id,
            project_id,
            issue_id: new_issue_id,
            actor,
            body: &split_note,
            commit_ref: None,
            status_change: None,
        },
    )
    .await?;

    Ok(new_issue_id)
}

fn truncate_to_hour_utc(ts: DateTime<Utc>) -> DateTime<Utc> {
    let secs = ts.timestamp();
    let hour_secs = secs - secs.rem_euclid(3600);
    DateTime::<Utc>::from_timestamp(hour_secs, 0).unwrap_or(ts)
}

fn build_split_title(exc_type: &str, exc_val: &str) -> String {
    let t = exc_type.trim();
    let v = exc_val.lines().next().unwrap_or("").trim();
    match (!t.is_empty(), !v.is_empty()) {
        (true, true) => format!("{t}: {v}"),
        (true, false) => t.to_owned(),
        (false, true) => v.to_owned(),
        (false, false) => "<unlabeled event>".to_owned(),
    }
}

fn synthesize_split_fingerprint(
    tenant_id: i64,
    project_id: i64,
    source_issue_id: i64,
    first_event_id: &str,
) -> String {
    let now_nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(
        format!("split:{tenant_id}:{project_id}:{source_issue_id}:{first_event_id}:{now_nanos}")
            .as_bytes(),
    );
    format!("split:{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_merge_input_bounds_and_self_merge_rejection() {
        let ok = validate_merge_input(&MergeIssuesInput {
            target_issue_id: 10,
            source_issue_ids: vec![12, 11, 12],
        })
        .expect("valid merge input");
        assert_eq!(ok, (10, vec![11, 12]));

        assert!(
            validate_merge_input(&MergeIssuesInput {
                target_issue_id: 10,
                source_issue_ids: vec![10],
            })
            .is_err()
        );
        assert!(
            validate_merge_input(&MergeIssuesInput {
                target_issue_id: 10,
                source_issue_ids: vec![],
            })
            .is_err()
        );
        assert!(
            validate_merge_input(&MergeIssuesInput {
                target_issue_id: 0,
                source_issue_ids: vec![2],
            })
            .is_err()
        );
    }

    #[test]
    fn validates_split_input_mutual_exclusion_and_dedup() {
        let mode = validate_split_input(&SplitIssueInput {
            fingerprints: Some(vec![" fp_a ".to_owned(), "fp_a".to_owned()]),
            event_ids: None,
        })
        .expect("valid fingerprint split");
        assert_eq!(
            mode,
            ValidatedSplitMode::Fingerprints(vec!["fp_a".to_owned()])
        );

        assert!(
            validate_split_input(&SplitIssueInput {
                fingerprints: Some(vec!["fp_a".to_owned()]),
                event_ids: Some(vec!["ev1".to_owned()]),
            })
            .is_err()
        );
        assert!(
            validate_split_input(&SplitIssueInput {
                fingerprints: None,
                event_ids: None,
            })
            .is_err()
        );
    }
}
