use chrono::{DateTime, Utc};
use maxi_loop_platform::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertChannelType {
    Webhook,
    Telegram,
    Email,
}

impl AlertChannelType {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "webhook" => Ok(Self::Webhook),
            "telegram" => Ok(Self::Telegram),
            "email" => Ok(Self::Email),
            _ => Err(AppError::Validation(
                "channel_type must be 'webhook', 'telegram', or 'email'".to_owned(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Telegram => "telegram",
            Self::Email => "email",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertTriggerType {
    NewIssue,
    Regression,
    Spike,
    /// `F019`: p95 transaction duration (ms) over the window exceeds `perf_threshold`.
    PerfP95,
    /// `F019`: transaction error rate (%) over the window exceeds `perf_threshold`.
    PerfErrorRate,
}

/// What a `spike` rule counts in its window (`F018`): occurrences or distinct affected users.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpikeMetric {
    Events,
    Users,
}

impl SpikeMetric {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "events" => Ok(Self::Events),
            "users" => Ok(Self::Users),
            _ => Err(AppError::Validation(
                "spike_metric must be 'events' or 'users'".to_owned(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Users => "users",
        }
    }
}

impl AlertTriggerType {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "new_issue" => Ok(Self::NewIssue),
            "regression" => Ok(Self::Regression),
            "spike" => Ok(Self::Spike),
            "perf_p95" => Ok(Self::PerfP95),
            "perf_error_rate" => Ok(Self::PerfErrorRate),
            _ => Err(AppError::Validation(
                "trigger_type must be 'new_issue', 'regression', 'spike', 'perf_p95', or \
                 'perf_error_rate'"
                    .to_owned(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewIssue => "new_issue",
            Self::Regression => "regression",
            Self::Spike => "spike",
            Self::PerfP95 => "perf_p95",
            Self::PerfErrorRate => "perf_error_rate",
        }
    }

    pub fn is_performance(self) -> bool {
        matches!(self, Self::PerfP95 | Self::PerfErrorRate)
    }

    /// Validate a performance threshold for this trigger (`F019`): ms for p95, percent for error rate.
    pub fn validate_perf_threshold(self, value: f64) -> Result<f64, AppError> {
        let ok = value.is_finite()
            && match self {
                Self::PerfP95 => (1.0..=600_000.0).contains(&value),
                Self::PerfErrorRate => (0.0..100.0).contains(&value),
                _ => false,
            };
        if ok {
            Ok(value)
        } else {
            Err(AppError::Validation(match self {
                Self::PerfP95 => "perf_threshold (ms) must be between 1 and 600000".to_owned(),
                Self::PerfErrorRate => "perf_threshold (%) must be >= 0 and < 100".to_owned(),
                _ => "perf_threshold only applies to performance rules".to_owned(),
            }))
        }
    }
}

/// Safe public view of an alert channel with all secrets masked or omitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertChannelView {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: Option<i64>,
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub scope: String,
    pub name: String,
    pub channel_type: String,
    pub config_summary: Value,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateAlertChannelInput {
    pub name: String,
    pub channel_type: String,
    pub config: Value,
    pub workspace_slug: Option<String>,
    pub project_slug: Option<String>,
    pub is_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateAlertChannelInput {
    pub name: Option<String>,
    pub is_enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRuleView {
    pub id: i64,
    pub tenant_id: i64,
    pub workspace_id: Option<i64>,
    pub workspace_slug: Option<String>,
    pub project_id: Option<i64>,
    pub project_slug: Option<String>,
    pub scope: String,
    pub channel_id: i64,
    pub channel_name: String,
    pub channel_type: String,
    pub name: String,
    pub trigger_type: String,
    pub environment_filter: Option<String>,
    pub spike_threshold: Option<i32>,
    pub spike_window_minutes: Option<i32>,
    pub spike_metric: String,
    /// `F019` performance rules: exact transaction (None = whole project).
    pub transaction_name: Option<String>,
    pub perf_threshold: Option<f64>,
    pub perf_window_minutes: Option<i32>,
    pub perf_min_count: i32,
    pub cooldown_minutes: i32,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateAlertRuleInput {
    pub name: String,
    pub channel_id: i64,
    pub trigger_type: String,
    pub workspace_slug: Option<String>,
    pub project_slug: Option<String>,
    pub environment_filter: Option<String>,
    pub spike_threshold: Option<i32>,
    pub spike_window_minutes: Option<i32>,
    pub spike_metric: Option<String>,
    pub transaction_name: Option<String>,
    pub perf_threshold: Option<f64>,
    pub perf_window_minutes: Option<i32>,
    pub perf_min_count: Option<i32>,
    pub cooldown_minutes: Option<i32>,
    pub is_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateAlertRuleInput {
    pub name: Option<String>,
    pub environment_filter: Option<String>,
    pub spike_threshold: Option<i32>,
    pub spike_window_minutes: Option<i32>,
    pub spike_metric: Option<String>,
    pub transaction_name: Option<String>,
    pub perf_threshold: Option<f64>,
    pub perf_window_minutes: Option<i32>,
    pub perf_min_count: Option<i32>,
    pub cooldown_minutes: Option<i32>,
    pub is_enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertDeliveryView {
    pub id: i64,
    pub tenant_id: i64,
    pub project_id: i64,
    pub project_slug: String,
    pub rule_id: i64,
    pub rule_name: String,
    pub channel_id: i64,
    pub channel_name: String,
    /// `None` for performance alerts (`F019`), which are not tied to an issue.
    pub issue_id: Option<i64>,
    /// Issue title, or the performance alert subject.
    pub issue_title: String,
    pub transaction_name: Option<String>,
    pub metric_value: Option<f64>,
    pub trigger_type: String,
    pub channel_type: String,
    pub status: String,
    pub http_status: Option<i32>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub fn mask_secret(raw: &str) -> String {
    let trimmed = raw.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 8 {
        "********".to_owned()
    } else {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{prefix}...{suffix}")
    }
}
