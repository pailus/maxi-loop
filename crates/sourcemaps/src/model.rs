use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMapView {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub project_id: i64,
    pub project_slug: String,
    pub release_version: String,
    pub filename: String,
    pub debug_id: Option<String>,
    pub sha256: String,
    pub raw_size_bytes: i32,
    pub compressed_size_bytes: i32,
    pub uploaded_by_user_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SourceMapListFilter {
    pub project_slug: Option<String>,
    pub release_version: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadSourceMapJsonInput {
    pub project_slug: String,
    pub workspace_slug: Option<String>,
    pub release_version: String,
    pub filename: String,
    pub debug_id: Option<String>,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct UploadSourceMapRawInput {
    pub project_slug: String,
    pub workspace_slug: Option<String>,
    pub release_version: String,
    pub filename: String,
    pub debug_id: Option<String>,
    pub raw_bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMapCacheStats {
    pub entries: usize,
    pub used_bytes: usize,
    pub max_bytes: usize,
    pub max_entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}
