//! Stage B: limiting a workspace membership (or invitation) to specific projects.
//!
//! Only workspace-scoped `member` / `viewer` grants can be limited; admins and tenant-level
//! roles always cover their whole scope. An empty list means "every project of the workspace".

use std::collections::HashMap;

use maxi_loop_platform::error::AppError;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};

use crate::model::Role;

const MAX_PROJECT_LIMITS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct ProjectRef {
    pub id: i64,
    pub slug: String,
    pub name: String,
}

/// Dedupe and sanity-check requested project ids (`None` = no limit).
pub(crate) fn normalize_project_ids(raw: Option<&[i64]>) -> Result<Vec<i64>, AppError> {
    let mut ids: Vec<i64> = raw.unwrap_or_default().to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.iter().any(|id| *id <= 0) {
        return Err(AppError::Validation(
            "project_ids must be positive integers".to_owned(),
        ));
    }
    if ids.len() > MAX_PROJECT_LIMITS {
        return Err(AppError::Validation(format!(
            "at most {MAX_PROJECT_LIMITS} projects can be selected"
        )));
    }
    Ok(ids)
}

/// Check that `ids` may limit a grant of `role` in `workspace_id` and that every project
/// belongs to that workspace. Returns the resolved projects (empty = no limit).
pub(crate) async fn validate_limits_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    workspace_id: Option<i64>,
    role: Role,
    ids: &[i64],
) -> Result<Vec<ProjectRef>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let Some(ws_id) = workspace_id else {
        return Err(AppError::Validation(
            "project limits require a workspace-scoped role".to_owned(),
        ));
    };
    if !matches!(role, Role::Member | Role::Viewer) {
        return Err(AppError::Validation(
            "only member or viewer roles can be limited to specific projects".to_owned(),
        ));
    }
    let projects: Vec<ProjectRef> = sqlx::query_as(
        "SELECT id, slug, name FROM projects
         WHERE tenant_id = $1 AND workspace_id = $2 AND id = ANY($3)
         ORDER BY lower(name), id",
    )
    .bind(tenant_id)
    .bind(ws_id)
    .bind(ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("validate project limits: {err}")))?;
    if projects.len() != ids.len() {
        return Err(AppError::Validation(
            "every selected project must belong to the chosen workspace".to_owned(),
        ));
    }
    Ok(projects)
}

/// Replace a membership's project limits (empty = no limit) and set its `project_limited` flag.
///
/// Unassign triggers fire after every statement, so statements are ordered to never pass
/// through a narrower state than the final one: widen first (add rows / clear the flag), narrow
/// last (remove rows / set the flag).
pub(crate) async fn replace_membership_limits_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    membership_id: i64,
    ids: &[i64],
) -> Result<(), AppError> {
    let set_flag = |limited: bool| {
        sqlx::query("UPDATE memberships SET project_limited = $3 WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(membership_id)
            .bind(limited)
    };
    if ids.is_empty() {
        set_flag(false)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("clear limited flag: {err}")))?;
        sqlx::query("DELETE FROM membership_projects WHERE tenant_id = $1 AND membership_id = $2")
            .bind(tenant_id)
            .bind(membership_id)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("clear membership limits: {err}")))?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO membership_projects (tenant_id, membership_id, project_id)
         SELECT $1, $2, unnest($3::BIGINT[])
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .bind(membership_id)
    .bind(ids)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("insert membership limits: {err}")))?;
    sqlx::query(
        "DELETE FROM membership_projects
         WHERE tenant_id = $1 AND membership_id = $2 AND NOT (project_id = ANY($3))",
    )
    .bind(tenant_id)
    .bind(membership_id)
    .bind(ids)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("prune membership limits: {err}")))?;
    set_flag(true)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("set limited flag: {err}")))?;
    Ok(())
}

#[derive(FromRow)]
struct LimitRow {
    owner_id: i64,
    id: Option<i64>,
    slug: Option<String>,
    name: Option<String>,
}

/// Project limits per *limited* membership id (missing key = not limited; an empty list =
/// limited but none of its projects exist any more).
pub(crate) async fn load_membership_limits_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    membership_ids: &[i64],
) -> Result<HashMap<i64, Vec<ProjectRef>>, AppError> {
    let rows: Vec<LimitRow> = sqlx::query_as(
        "SELECT m.id AS owner_id, p.id, p.slug, p.name
         FROM memberships m
         LEFT JOIN membership_projects mp ON mp.tenant_id = m.tenant_id AND mp.membership_id = m.id
         LEFT JOIN projects p ON p.tenant_id = mp.tenant_id AND p.id = mp.project_id
         WHERE m.tenant_id = $1 AND m.id = ANY($2) AND m.project_limited
         ORDER BY lower(p.name), p.id",
    )
    .bind(tenant_id)
    .bind(membership_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load membership limits: {err}")))?;
    Ok(group(rows))
}

/// Projects per *limited* invitation id (same convention as memberships).
pub(crate) async fn load_invitation_limits_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: i64,
    invitation_ids: &[i64],
) -> Result<HashMap<i64, Vec<ProjectRef>>, AppError> {
    let rows: Vec<LimitRow> = sqlx::query_as(
        "SELECT i.id AS owner_id, p.id, p.slug, p.name
         FROM invitations i
         LEFT JOIN invitation_projects ip ON ip.tenant_id = i.tenant_id AND ip.invitation_id = i.id
         LEFT JOIN projects p ON p.tenant_id = ip.tenant_id AND p.id = ip.project_id
         WHERE i.tenant_id = $1 AND i.id = ANY($2) AND i.project_limited
         ORDER BY lower(p.name), p.id",
    )
    .bind(tenant_id)
    .bind(invitation_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load invitation limits: {err}")))?;
    Ok(group(rows))
}

fn group(rows: Vec<LimitRow>) -> HashMap<i64, Vec<ProjectRef>> {
    let mut map: HashMap<i64, Vec<ProjectRef>> = HashMap::new();
    for r in rows {
        let entry = map.entry(r.owner_id).or_default();
        if let (Some(id), Some(slug), Some(name)) = (r.id, r.slug, r.name) {
            entry.push(ProjectRef { id, slug, name });
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ids() {
        assert_eq!(normalize_project_ids(None).unwrap(), Vec::<i64>::new());
        assert_eq!(normalize_project_ids(Some(&[3, 1, 3])).unwrap(), vec![1, 3]);
        assert!(normalize_project_ids(Some(&[0])).is_err());
        assert!(normalize_project_ids(Some(&(1..=201).collect::<Vec<_>>())).is_err());
    }
}
