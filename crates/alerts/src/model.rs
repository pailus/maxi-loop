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
}

impl AlertTriggerType {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "new_issue" => Ok(Self::NewIssue),
            "regression" => Ok(Self::Regression),
            "spike" => Ok(Self::Spike),
            _ => Err(AppError::Validation(
                "trigger_type must be 'new_issue', 'regression', or 'spike'".to_owned(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewIssue => "new_issue",
            Self::Regression => "regression",
            Self::Spike => "spike",
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    pub cooldown_minutes: Option<i32>,
    pub is_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateAlertRuleInput {
    pub name: Option<String>,
    pub environment_filter: Option<String>,
    pub spike_threshold: Option<i32>,
    pub spike_window_minutes: Option<i32>,
    pub cooldown_minutes: Option<i32>,
    pub is_enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertDeliveryView {
    pub id: i64,
    pub tenant_id: i64,
    pub project_id: i64,
    pub project_slug: String,
    pub rule_id: i64,
    pub rule_name: String,
    pub channel_id: i64,
    pub channel_name: String,
    pub issue_id: i64,
    pub issue_title: String,
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
