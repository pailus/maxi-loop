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
    pub default_rate_limit_per_minute_override: Option<i32>,
    pub effective_rate_limit_per_minute: i32,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateWorkspaceInput {
    pub slug: String,
    pub name: String,
    pub retention_days_override: Option<u16>,
    pub default_rate_limit_per_minute_override: Option<i32>,
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
    pub rate_limit_per_minute_override: Option<i32>,
    /// Effective rate limit (explicit project override, or inherited from workspace -> tenant).
    pub rate_limit_per_minute: i32,
    pub effective_rate_limit_per_minute: i32,
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

/// Optional double-option deserialization helper so PATCH requests distinguish
/// omitted field (`None`) vs explicit `null` (`Some(None)`, reset to inherit) vs explicit value (`Some(Some(v))`).
fn deserialize_double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UpdateTenantSettingsInput {
    pub name: Option<String>,
    pub retention_days: Option<u16>,
    pub default_rate_limit_per_minute: Option<i32>,
    pub aggregate_rate_limit_per_minute: Option<i32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UpdateWorkspaceSettingsInput {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub retention_days_override: Option<Option<u16>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub default_rate_limit_per_minute_override: Option<Option<i32>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UpdateProjectSettingsInput {
    pub name: Option<String>,
    pub platform: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub retention_days_override: Option<Option<u16>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub rate_limit_per_minute_override: Option<Option<i32>>,
}
