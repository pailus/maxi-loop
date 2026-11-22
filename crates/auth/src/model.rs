use std::collections::{BTreeMap, BTreeSet};

use axum::http::HeaderMap;
use maxi_loop_platform::access::{AccessGuard, ProjectAccess};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::TenantContext;
use serde::{Deserialize, Serialize};

use crate::token::{CSRF_HEADER_NAME, verify_csrf_token};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    TenantOwner,
    TenantAdmin,
    WorkspaceAdmin,
    Member,
    Viewer,
}

impl Role {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw {
            "tenant_owner" => Ok(Self::TenantOwner),
            "tenant_admin" => Ok(Self::TenantAdmin),
            "workspace_admin" => Ok(Self::WorkspaceAdmin),
            "member" => Ok(Self::Member),
            "viewer" => Ok(Self::Viewer),
            _ => Err(AppError::Validation("invalid membership role".to_owned())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TenantOwner => "tenant_owner",
            Self::TenantAdmin => "tenant_admin",
            Self::WorkspaceAdmin => "workspace_admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub fn privilege_rank(self) -> u8 {
        match self {
            Self::TenantOwner => 5,
            Self::TenantAdmin => 4,
            Self::WorkspaceAdmin => 3,
            Self::Member => 2,
            Self::Viewer => 1,
        }
    }

    pub fn can_manage_workspaces(self) -> bool {
        matches!(self, Self::TenantOwner | Self::TenantAdmin)
    }

    pub fn can_manage_projects(self) -> bool {
        matches!(
            self,
            Self::TenantOwner | Self::TenantAdmin | Self::WorkspaceAdmin
        )
    }

    pub fn can_triage_issues(self) -> bool {
        matches!(
            self,
            Self::TenantOwner | Self::TenantAdmin | Self::WorkspaceAdmin | Self::Member
        )
    }

    pub fn is_valid_tenant_role(self) -> bool {
        matches!(
            self,
            Self::TenantOwner | Self::TenantAdmin | Self::Member | Self::Viewer
        )
    }

    pub fn is_valid_workspace_role(self) -> bool {
        matches!(self, Self::WorkspaceAdmin | Self::Member | Self::Viewer)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedUser {
    pub id: i64,
    pub email: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantSummary {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub retention_days: u16,
    pub default_rate_limit_per_minute: i32,
    pub aggregate_rate_limit_per_minute: i32,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub retention_days_override: Option<u16>,
    pub default_rate_limit_per_minute_override: Option<i32>,
    pub is_default: bool,
}

/// Scope details when a request is authenticated via a per-project API Bearer token (`mlapi_...`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiTokenAuthScope {
    pub token_id: i64,
    pub token_name: String,
    pub project_id: i64,
    pub project_slug: String,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub can_read: bool,
    pub can_write: bool,
}

#[derive(Clone, Debug)]
pub struct AuthSession {
    pub session_id: i64,
    pub tenant_ctx: TenantContext,
    pub tenant: TenantSummary,
    pub default_workspace: WorkspaceSummary,
    pub user: AuthenticatedUser,
    /// Highest effective role across the tenant (used for UI affordances & legacy checks).
    pub role: Role,
    /// Explicit tenant-wide role (`memberships.workspace_id IS NULL`), if any.
    pub tenant_role: Option<Role>,
    /// Explicit per-workspace roles (`memberships.workspace_id IS NOT NULL`).
    pub workspace_roles: BTreeMap<i64, Role>,
    /// Workspace memberships limited to specific projects (`membership_projects`). Absent =
    /// every project of that workspace. Ignored when the user has a tenant-level role.
    pub workspace_project_limits: BTreeMap<i64, BTreeSet<i64>>,
    /// Present only when authenticated via a per-project API token (`Authorization: Bearer mlapi_...`).
    pub api_token: Option<ApiTokenAuthScope>,
    pub csrf_token: String,
}

impl AuthSession {
    /// Enforce constant-time `X-CSRF-Token` validation against the session's CSRF token.
    pub fn verify_csrf_header(&self, headers: &HeaderMap) -> Result<(), AppError> {
        let provided = headers
            .get(CSRF_HEADER_NAME)
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Forbidden("missing csrf token"))?;

        if !verify_csrf_token(&self.csrf_token, provided) {
            return Err(AppError::Forbidden("invalid csrf token"));
        }
        Ok(())
    }

    pub fn is_api_token(&self) -> bool {
        self.api_token.is_some()
    }

    /// Reject requests authenticated via a per-project API token on admin/user/tenant-wide routes.
    pub fn require_user_session(&self) -> Result<(), AppError> {
        if self.api_token.is_some() {
            return Err(AppError::Forbidden(
                "project api tokens cannot access this endpoint",
            ));
        }
        Ok(())
    }

    pub fn scoped_project_id(&self) -> Option<i64> {
        self.api_token.as_ref().map(|t| t.project_id)
    }

    pub fn scoped_project_slug(&self) -> Option<&str> {
        self.api_token.as_ref().map(|t| t.project_slug.as_str())
    }

    pub fn scoped_workspace_slug(&self) -> Option<&str> {
        self.api_token.as_ref().map(|t| t.workspace_slug.as_str())
    }

    /// Compute the effective role for a specific workspace (`max(tenant_role, workspace_role)`).
    /// Returns `None` if the user has neither a tenant-level role nor a membership on this workspace.
    pub fn effective_role_for_workspace(&self, workspace_id: i64) -> Option<Role> {
        if let Some(tok) = &self.api_token {
            if tok.workspace_id != workspace_id {
                return None;
            }
            return Some(if tok.can_write {
                Role::Member
            } else {
                Role::Viewer
            });
        }

        match (
            self.tenant_role,
            self.workspace_roles.get(&workspace_id).copied(),
        ) {
            (Some(t), Some(w)) => {
                if w.privilege_rank() > t.privilege_rank() {
                    Some(w)
                } else {
                    Some(t)
                }
            }
            (Some(t), None) => Some(t),
            (None, Some(w)) => Some(w),
            (None, None) => None,
        }
    }

    /// Projects this caller may read, for SQL list filters. `None` = every project in the
    /// tenant (tenant-level role). Single-record endpoints use `enforce_project_read`.
    pub fn project_access(&self) -> Option<ProjectAccess> {
        if let Some(tok) = &self.api_token {
            return Some(ProjectAccess {
                workspace_ids: Vec::new(),
                project_ids: if tok.can_read {
                    vec![tok.project_id]
                } else {
                    Vec::new()
                },
            });
        }
        if self.tenant_role.is_some() {
            return None;
        }
        let mut access = ProjectAccess::default();
        for ws_id in self.workspace_roles.keys() {
            match self.workspace_project_limits.get(ws_id) {
                Some(projects) => access.project_ids.extend(projects.iter().copied()),
                None => access.workspace_ids.push(*ws_id),
            }
        }
        Some(access)
    }

    /// Whether the caller may read this project's data (issues, events, performance, ...).
    pub fn can_read_project(&self, project_id: i64, workspace_id: i64) -> bool {
        self.project_access()
            .is_none_or(|a| a.allows(project_id, workspace_id))
    }

    /// Whether the caller sees every project of the workspace (tenant-level role or a
    /// workspace membership). Needed for workspace-wide grants such as workspace MCP tokens.
    pub fn can_read_whole_workspace(&self, workspace_id: i64) -> bool {
        !self.is_api_token()
            && self
                .project_access()
                .is_none_or(|a| a.workspace_ids.contains(&workspace_id))
    }

    /// Whether the caller may triage (change status, assign, upload source maps) in this project.
    pub fn can_triage_project(&self, project_id: i64, workspace_id: i64) -> bool {
        self.can_read_project(project_id, workspace_id) && self.can_triage_workspace(workspace_id)
    }

    pub fn can_access_workspace(&self, workspace_id: i64) -> bool {
        self.effective_role_for_workspace(workspace_id).is_some()
    }

    pub fn can_manage_tenant(&self) -> bool {
        !self.is_api_token() && self.tenant_role.is_some_and(Role::can_manage_workspaces)
    }

    pub fn can_manage_workspace(&self, workspace_id: i64) -> bool {
        !self.is_api_token()
            && self
                .effective_role_for_workspace(workspace_id)
                .is_some_and(Role::can_manage_projects)
    }

    pub fn can_triage_workspace(&self, workspace_id: i64) -> bool {
        self.effective_role_for_workspace(workspace_id)
            .is_some_and(Role::can_triage_issues)
    }

    pub fn can_manage_members(&self) -> bool {
        !self.is_api_token()
            && (self.can_manage_tenant()
                || self
                    .workspace_roles
                    .values()
                    .any(|r| r.can_manage_projects()))
    }

    /// Enforce read access for a specific project (both API token project scoping and workspace RBAC).
    pub fn enforce_project_read(&self, project_id: i64, workspace_id: i64) -> Result<(), AppError> {
        if let Some(tok) = &self.api_token {
            if !tok.can_read || tok.project_id != project_id {
                return Err(AppError::Forbidden(
                    "api token does not grant access to this project",
                ));
            }
            return Ok(());
        }
        if !self.can_read_project(project_id, workspace_id) {
            return Err(AppError::Forbidden("insufficient workspace permissions"));
        }
        Ok(())
    }

    /// Enforce triage/write access for a specific project (API token `can_write` + project match,
    /// or user workspace role `can_triage_issues`).
    pub fn enforce_project_write(
        &self,
        project_id: i64,
        workspace_id: i64,
    ) -> Result<(), AppError> {
        if let Some(tok) = &self.api_token {
            if !tok.can_write || tok.project_id != project_id {
                return Err(AppError::Forbidden(
                    "api token does not grant write access to this project",
                ));
            }
            return Ok(());
        }
        if !self.can_triage_project(project_id, workspace_id) {
            return Err(AppError::Forbidden("insufficient permissions"));
        }
        Ok(())
    }

    /// For API tokens targeting a slug-based project route, fail closed if the requested slug does
    /// not match the token's project.
    pub fn enforce_api_token_project_slug(&self, project_slug: &str) -> Result<(), AppError> {
        if let Some(tok) = &self.api_token
            && tok.project_slug != project_slug
        {
            return Err(AppError::Forbidden(
                "api token is scoped to a different project",
            ));
        }
        Ok(())
    }

    /// Enforce `can_write` for API token write actions.
    pub fn enforce_api_token_write(&self) -> Result<(), AppError> {
        if let Some(tok) = &self.api_token
            && !tok.can_write
        {
            return Err(AppError::Forbidden("api token is read-only"));
        }
        Ok(())
    }

    pub fn to_view(&self, single_tenant_mode: bool) -> AuthSessionView {
        AuthSessionView {
            user: self.user.clone(),
            tenant: self.tenant.clone(),
            default_workspace: self.default_workspace.clone(),
            role: self.role,
            tenant_role: self.tenant_role,
            workspace_roles: self.workspace_roles.clone(),
            csrf_token: self.csrf_token.clone(),
            single_tenant_mode,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSessionView {
    pub user: AuthenticatedUser,
    pub tenant: TenantSummary,
    pub default_workspace: WorkspaceSummary,
    pub role: Role,
    pub tenant_role: Option<Role>,
    pub workspace_roles: BTreeMap<i64, Role>,
    pub csrf_token: String,
    pub single_tenant_mode: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    pub tenant_slug: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

impl AccessGuard for AuthSession {
    fn check_read(&self, project_id: i64, workspace_id: i64) -> Result<(), AppError> {
        self.enforce_project_read(project_id, workspace_id)
    }

    fn check_triage(&self, project_id: i64, workspace_id: i64) -> Result<(), AppError> {
        self.enforce_project_write(project_id, workspace_id)
    }

    fn check_manage_project(&self, project_id: i64, workspace_id: i64) -> Result<(), AppError> {
        if self.can_read_project(project_id, workspace_id)
            && self.can_manage_workspace(workspace_id)
        {
            Ok(())
        } else {
            Err(AppError::Forbidden("insufficient permissions"))
        }
    }

    fn check_manage_workspace(&self, workspace_id: i64) -> Result<(), AppError> {
        if self.can_manage_workspace(workspace_id) {
            Ok(())
        } else {
            Err(AppError::Forbidden("insufficient permissions"))
        }
    }

    fn check_manage_tenant(&self) -> Result<(), AppError> {
        if self.can_manage_tenant() {
            Ok(())
        } else {
            Err(AppError::Forbidden(
                "only tenant owners or tenant admins can manage tenant-wide settings",
            ))
        }
    }

    fn can_read_scope(&self, workspace_id: Option<i64>, project_id: Option<i64>) -> bool {
        match (workspace_id, project_id) {
            (None, None) => true,
            (Some(ws), None) => self.can_access_workspace(ws),
            (Some(ws), Some(p)) => self.can_read_project(p, ws),
            (None, Some(_)) => false,
        }
    }
}

/// A bare role acts as a tenant-wide role (used by internal callers and tests).
impl AccessGuard for Role {
    fn check_read(&self, _project_id: i64, _workspace_id: i64) -> Result<(), AppError> {
        Ok(())
    }

    fn check_triage(&self, _project_id: i64, _workspace_id: i64) -> Result<(), AppError> {
        role_check(self.can_triage_issues())
    }

    fn check_manage_project(&self, _project_id: i64, _workspace_id: i64) -> Result<(), AppError> {
        role_check(self.can_manage_projects())
    }

    fn check_manage_workspace(&self, _workspace_id: i64) -> Result<(), AppError> {
        role_check(self.can_manage_projects())
    }

    fn check_manage_tenant(&self) -> Result<(), AppError> {
        role_check(self.can_manage_workspaces())
    }

    fn can_read_scope(&self, _workspace_id: Option<i64>, _project_id: Option<i64>) -> bool {
        true
    }
}

fn role_check(allowed: bool) -> Result<(), AppError> {
    if allowed {
        Ok(())
    } else {
        Err(AppError::Forbidden("insufficient permissions"))
    }
}
