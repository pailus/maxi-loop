/// Signal emitted by the ingest worker when a batch introduces a brand-new issue or reopens
/// a previously resolved issue (regression), allowing the alert engine to react in real time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlertSignalKind {
    NewIssue,
    Regression,
}

impl AlertSignalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NewIssue => "new_issue",
            Self::Regression => "regression",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertSignal {
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub project_id: i64,
    pub issue_id: i64,
    pub kind: AlertSignalKind,
    pub title: String,
    pub exception_type: String,
    pub exception_value: String,
    pub culprit: String,
    pub level: String,
    pub environment: String,
    pub release: Option<String>,
    pub event_count: i64,
}

/// Non-blocking observer trait so `crates/ingest` can notify `crates/alerts` without a crate cycle.
pub trait AlertSignalObserver: Send + Sync {
    fn on_alert_signal(&self, signal: AlertSignal);
}
