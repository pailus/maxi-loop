/// Notification payload emitted when a project's DSN or rate limit is created or updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDsnInfo {
    pub project_id: i64,
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub dsn_public_key: String,
    pub rate_limit_per_minute: u32,
}

/// Observer trait allowing the in-memory ingest DSN cache to stay synchronized
/// when projects are created or DSN keys are rotated without coupling `org` to `ingest`.
pub trait ProjectDsnObserver: Send + Sync {
    fn on_project_dsn_upserted(&self, info: ProjectDsnInfo);
}
