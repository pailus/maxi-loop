use axum::http::HeaderMap;
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
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub retention_days_override: Option<u16>,
    pub is_default: bool,
}

#[derive(Clone, Debug)]
pub struct AuthSession {
    pub session_id: i64,
    pub tenant_ctx: TenantContext,
    pub tenant: TenantSummary,
    pub default_workspace: WorkspaceSummary,
    pub user: AuthenticatedUser,
    pub role: Role,
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

    pub fn to_view(&self, single_tenant_mode: bool) -> AuthSessionView {
        AuthSessionView {
            user: self.user.clone(),
            tenant: self.tenant.clone(),
            default_workspace: self.default_workspace.clone(),
            role: self.role,
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
    pub csrf_token: String,
    pub single_tenant_mode: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    pub tenant_slug: Option<String>,
}
