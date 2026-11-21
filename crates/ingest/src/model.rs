use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedEvent {
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub project_id: i64,
    pub event_id: String,
    pub received_at: DateTime<Utc>,
    pub level: String,
    pub environment: String,
    pub release: Option<String>,
    pub platform: String,
    pub raw_payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedEnvelope {
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub project_id: i64,
    pub envelope_id: String,
    pub received_at: DateTime<Utc>,
    pub events: Vec<QueuedEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeIngestResponse {
    pub id: String,
    pub accepted_items: usize,
    pub dropped_items: usize,
}
