//! Project visibility filter applied inside SQL queries.
//!
//! RLS isolates tenants; within a tenant, which projects a caller may see depends on their
//! memberships. Applying that in the query (instead of filtering rows afterwards) keeps
//! `total` counts and pagination honest and avoids leaking other projects' data.

use sqlx::{Postgres, QueryBuilder};

use crate::error::AppError;

/// Authorization checks a service runs against the exact project/workspace it resolved, so the
/// check cannot drift from the record being changed. Implemented by the request session
/// (`AuthSession`) and by a bare tenant-wide `Role` (internal callers and tests).
pub trait AccessGuard: Sync {
    /// Read this project's data.
    fn check_read(&self, project_id: i64, workspace_id: i64) -> Result<(), AppError>;
    /// Triage-level writes in this project (issue status, releases, source maps).
    fn check_triage(&self, project_id: i64, workspace_id: i64) -> Result<(), AppError>;
    /// Admin-level changes to this project's configuration.
    fn check_manage_project(&self, project_id: i64, workspace_id: i64) -> Result<(), AppError>;
    /// Admin-level changes to workspace-wide configuration.
    fn check_manage_workspace(&self, workspace_id: i64) -> Result<(), AppError>;
    /// Admin-level changes to tenant-wide configuration.
    fn check_manage_tenant(&self) -> Result<(), AppError>;

    /// Manage check for a resource scoped to the tenant (`None`, `None`), a workspace or a project.
    fn check_manage_scope(
        &self,
        workspace_id: Option<i64>,
        project_id: Option<i64>,
    ) -> Result<(), AppError> {
        match (workspace_id, project_id) {
            (Some(ws), Some(p)) => self.check_manage_project(p, ws),
            (Some(ws), None) => self.check_manage_workspace(ws),
            (None, Some(_)) => Err(AppError::Forbidden("insufficient permissions")),
            (None, None) => self.check_manage_tenant(),
        }
    }

    /// Read check for a scoped resource: tenant-wide resources are readable by every member.
    fn can_read_scope(&self, workspace_id: Option<i64>, project_id: Option<i64>) -> bool;
}

/// Projects a caller may read. `None` at the call site means "every project in the tenant".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectAccess {
    /// Workspaces whose projects are all visible.
    pub workspace_ids: Vec<i64>,
    /// Individually visible projects (API tokens, project-limited memberships).
    pub project_ids: Vec<i64>,
}

impl ProjectAccess {
    pub fn allows(&self, project_id: i64, workspace_id: i64) -> bool {
        self.workspace_ids.contains(&workspace_id) || self.project_ids.contains(&project_id)
    }

    /// Append ` AND (<ws_col> = ANY(..) OR <project_col> = ANY(..))`. Column names are
    /// compile-time constants supplied by the caller, never user input.
    pub fn push_sql(
        &self,
        qb: &mut QueryBuilder<'_, Postgres>,
        workspace_col: &'static str,
        project_col: &'static str,
    ) {
        qb.push(" AND (")
            .push(workspace_col)
            .push(" = ANY(")
            .push_bind(self.workspace_ids.clone())
            .push(") OR ")
            .push(project_col)
            .push(" = ANY(")
            .push_bind(self.project_ids.clone())
            .push("))");
    }
}

/// Append the filter when access is restricted; no-op for unrestricted callers.
pub fn push_access_sql(
    access: Option<&ProjectAccess>,
    qb: &mut QueryBuilder<'_, Postgres>,
    workspace_col: &'static str,
    project_col: &'static str,
) {
    if let Some(a) = access {
        a.push_sql(qb, workspace_col, project_col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_by_workspace_or_project() {
        let a = ProjectAccess {
            workspace_ids: vec![1],
            project_ids: vec![10],
        };
        assert!(a.allows(99, 1));
        assert!(a.allows(10, 2));
        assert!(!a.allows(11, 2));
        assert!(!ProjectAccess::default().allows(1, 1));
    }

    #[test]
    fn pushes_bound_filter() {
        let a = ProjectAccess {
            workspace_ids: vec![1],
            project_ids: vec![],
        };
        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new("SELECT 1 FROM projects p WHERE 1=1");
        push_access_sql(Some(&a), &mut qb, "p.workspace_id", "p.id");
        assert_eq!(
            qb.sql(),
            "SELECT 1 FROM projects p WHERE 1=1 AND (p.workspace_id = ANY($1) OR p.id = ANY($2))"
        );
        let mut qb2: QueryBuilder<Postgres> = QueryBuilder::new("X");
        push_access_sql(None, &mut qb2, "p.workspace_id", "p.id");
        assert_eq!(qb2.sql(), "X");
    }
}
