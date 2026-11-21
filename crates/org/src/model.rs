use chrono::{DateTime, Utc};
use maxi_loop_auth::model::{TenantSummary, WorkspaceSummary};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantOverview {
    pub tenant: TenantSummary,
    pub default_workspace: WorkspaceSummary,
    pub single_tenant_mode: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceView {
    pub id: i64,
    pub tenant_id: i64,
    pub tenant_slug: String,
    pub slug: String,
    pub name: String,
    pub retention_days_override: Option<u16>,
    pub effective_retention_days: u16,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateWorkspaceInput {
    pub slug: String,
    pub name: String,
    pub retention_days_override: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectView {
    pub id: i64,
    pub tenant_id: i64,
    pub tenant_slug: String,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub slug: String,
    pub name: String,
    pub platform: String,
    pub dsn_public_key: String,
    pub dsn: String,
    pub ingest_endpoint: String,
    pub retention_days_override: Option<u16>,
    pub effective_retention_days: u16,
    pub rate_limit_per_minute: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateProjectInput {
    pub slug: String,
    pub name: String,
    pub platform: Option<String>,
    pub retention_days_override: Option<u16>,
    pub rate_limit_per_minute: Option<i32>,
    pub workspace_slug: Option<String>,
}
