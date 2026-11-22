use chrono::{DateTime, Utc};
use maxi_loop_platform::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::breadcrumbs::BreadcrumbView;
use crate::merge_split::IssueFingerprintView;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Unresolved,
    Resolved,
    Ignored,
}

impl IssueStatus {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "unresolved" => Ok(Self::Unresolved),
            "resolved" => Ok(Self::Resolved),
            "ignored" => Ok(Self::Ignored),
            _ => Err(AppError::Validation(
                "issue status must be 'unresolved', 'resolved', or 'ignored'".to_owned(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::Resolved => "resolved",
            Self::Ignored => "ignored",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueSort {
    LastSeenDesc,
    FirstSeenDesc,
    EventsDesc,
    UsersDesc,
    RegressedDesc,
}

impl IssueSort {
    pub fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("last_seen") => Ok(Self::LastSeenDesc),
            Some("first_seen") => Ok(Self::FirstSeenDesc),
            Some("events" | "event_count") => Ok(Self::EventsDesc),
            Some("users" | "user_count") => Ok(Self::UsersDesc),
            Some("regressed" | "regression") => Ok(Self::RegressedDesc),
            Some(_) => Err(AppError::Validation(
                "sort must be 'last_seen', 'first_seen', 'events', 'users', or 'regressed'"
                    .to_owned(),
            )),
        }
    }

    pub fn order_by_sql(self) -> &'static str {
        match self {
            Self::LastSeenDesc => " ORDER BY i.last_seen DESC, i.id DESC",
            Self::FirstSeenDesc => " ORDER BY i.first_seen DESC, i.id DESC",
            Self::EventsDesc => " ORDER BY i.event_count DESC, i.last_seen DESC, i.id DESC",
            Self::UsersDesc => {
                " ORDER BY i.user_count DESC, i.event_count DESC, i.last_seen DESC, i.id DESC"
            }
            Self::RegressedDesc => " ORDER BY i.is_regressed DESC, i.last_seen DESC, i.id DESC",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsInterval {
    Hourly,
    Daily,
}

impl StatsInterval {
    pub fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("1h" | "hourly" | "hour") => Ok(Self::Hourly),
            Some("1d" | "daily" | "day") => Ok(Self::Daily),
            Some(_) => Err(AppError::Validation(
                "stats interval must be '1h' (hourly) or '1d' (daily)".to_owned(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hourly => "1h",
            Self::Daily => "1d",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct IssueListFilter {
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub environment: Option<String>,
    pub release: Option<String>,
    pub status: Option<String>,
    pub is_regressed: Option<bool>,
    /// `error`, `performance` (any performance type) or an exact `performance_*` type.
    pub issue_type: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub query: Option<String>,
    /// `none` (unassigned), a user id, or an email. Callers resolve `me` to the user id first.
    pub assigned_to: Option<String>,
    pub sort: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

/// The member an issue is assigned to (`F024`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueAssigneeView {
    pub user_id: i64,
    pub email: String,
    pub name: String,
}

impl IssueAssigneeView {
    /// Build from the `LEFT JOIN users` columns; `None` when unassigned (or the user row is
    /// not visible in this tenant).
    pub fn from_columns(
        user_id: Option<i64>,
        email: Option<String>,
        name: Option<String>,
    ) -> Option<Self> {
        Some(Self {
            user_id: user_id?,
            email: email?,
            name: name?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSummaryView {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub project_id: i64,
    pub project_slug: String,
    pub fingerprint: String,
    pub title: String,
    pub exception_type: String,
    pub exception_value: String,
    pub culprit: String,
    pub level: String,
    pub issue_type: String,
    pub status: String,
    pub is_regressed: bool,
    pub event_count: i64,
    /// Distinct (hashed) users affected (`F018`).
    pub user_count: i64,
    pub stored_full_events: i32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub last_environment: String,
    pub first_release: Option<String>,
    pub last_release: Option<String>,
    pub sample_event_id: Option<String>,
    #[serde(default)]
    pub assignee: Option<IssueAssigneeView>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginatedIssuesResponse {
    pub items: Vec<IssueSummaryView>,
    pub total: i64,
    pub page: u32,
    pub per_page: u32,
    pub total_pages: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueEventView {
    pub event_id: String,
    pub issue_id: i64,
    pub project_id: i64,
    #[serde(default)]
    pub fingerprint: Option<String>,
    pub received_at: DateTime<Utc>,
    pub environment: String,
    pub release: Option<String>,
    pub level: String,
    pub platform: String,
    pub exception_type: String,
    pub exception_value: String,
    pub stack_frames: Value,
    pub tags: Value,
    #[serde(default)]
    pub breadcrumbs: Vec<BreadcrumbView>,
    pub payload: Value,
    pub is_sample: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterBucketPoint {
    pub bucket: DateTime<Utc>,
    pub event_count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueStatsResponse {
    pub issue_id: i64,
    pub project_id: i64,
    pub interval: String,
    pub series: Vec<CounterBucketPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueDetailView {
    pub issue: IssueSummaryView,
    pub stack_frames: Value,
    pub tags: Value,
    #[serde(default)]
    pub breadcrumbs: Vec<BreadcrumbView>,
    #[serde(default)]
    pub merged_fingerprints: Vec<IssueFingerprintView>,
    pub sample_event: Option<IssueEventView>,
    pub hourly_series: Vec<CounterBucketPoint>,
    pub daily_series: Vec<CounterBucketPoint>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateIssueStatusInput {
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub commit_ref: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_issue_status_sort_and_stats_interval() {
        assert_eq!(
            IssueStatus::parse("unresolved").unwrap(),
            IssueStatus::Unresolved
        );
        assert_eq!(
            IssueStatus::parse("RESOLVED").unwrap(),
            IssueStatus::Resolved
        );
        assert_eq!(IssueStatus::parse("ignored").unwrap(), IssueStatus::Ignored);
        assert!(IssueStatus::parse("archived").is_err());

        assert_eq!(IssueSort::parse(None).unwrap(), IssueSort::LastSeenDesc);
        assert_eq!(
            IssueSort::parse(Some("events")).unwrap(),
            IssueSort::EventsDesc
        );
        assert!(IssueSort::parse(Some("DROP TABLE")).is_err());

        assert_eq!(
            StatsInterval::parse(Some("1h")).unwrap(),
            StatsInterval::Hourly
        );
        assert_eq!(
            StatsInterval::parse(Some("1d")).unwrap(),
            StatsInterval::Daily
        );
        assert!(StatsInterval::parse(Some("5m")).is_err());
    }
}
