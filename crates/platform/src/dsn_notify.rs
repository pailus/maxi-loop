/// Notification payload emitted when a project's DSN or effective rate limit is created or updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDsnInfo {
    pub project_id: i64,
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub dsn_public_key: String,
    pub rate_limit_per_minute: u32,
}

/// Notification payload emitted when tenant or workspace rate-limit settings change,
/// keeping both the tenant aggregate token bucket and inherited project buckets synchronized in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantRateLimitUpdate {
    pub tenant_id: i64,
    pub aggregate_rate_limit_per_minute: Option<u32>,
    /// Pairs of `(project_id, effective_rate_limit_per_minute)` whose effective limits changed.
    pub project_effective_limits: Vec<(i64, u32)>,
}

/// Observer trait allowing the in-memory ingest DSN & rate-limit cache to stay synchronized
/// when projects, workspaces, or tenant settings change without coupling `org` to `ingest`.
pub trait ProjectDsnObserver: Send + Sync {
    fn on_project_dsn_upserted(&self, info: ProjectDsnInfo);

    fn on_tenant_rate_limits_updated(&self, _update: TenantRateLimitUpdate) {}
}
