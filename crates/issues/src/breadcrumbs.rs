use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_BREADCRUMBS_PER_EVENT: usize = 200;
const MAX_FIELD_CHARS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BreadcrumbView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(rename = "type")]
    pub crumb_type: String,
    pub category: String,
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Extract and normalize Sentry event breadcrumbs from a PII-scrubbed event JSON payload (`F-ISS-10`).
/// Supports both standard Sentry `{ "breadcrumbs": { "values": [...] } }` and flat array
/// `{ "breadcrumbs": [...] }` shapes, normalizing numeric Unix timestamps into ISO-8601 UTC.
pub fn extract_breadcrumbs(payload: &Value) -> Vec<BreadcrumbView> {
    let Some(node) = payload.get("breadcrumbs") else {
        return Vec::new();
    };

    let items = node
        .get("values")
        .and_then(Value::as_array)
        .or_else(|| node.as_array());

    let Some(arr) = items else {
        return Vec::new();
    };

    // Keep the most recent MAX_BREADCRUMBS_PER_EVENT breadcrumbs in chronological order.
    let start_idx = arr.len().saturating_sub(MAX_BREADCRUMBS_PER_EVENT);
    arr[start_idx..]
        .iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            let timestamp = obj
                .get("timestamp")
                .and_then(normalize_breadcrumb_timestamp);
            let crumb_type = obj
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate_chars(s, 64))
                .unwrap_or_else(|| "default".to_owned());
            let category = obj
                .get("category")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate_chars(s, 128))
                .unwrap_or_else(|| crumb_type.clone());
            let level = normalize_level(obj.get("level").and_then(Value::as_str));
            let message = obj
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate_chars(s, MAX_FIELD_CHARS));
            let data = obj.get("data").and_then(|v| match v {
                Value::Null => None,
                Value::Object(m) if m.is_empty() => None,
                other => Some(other.clone()),
            });

            Some(BreadcrumbView {
                timestamp,
                crumb_type,
                category,
                level,
                message,
                data,
            })
        })
        .collect()
}

fn normalize_level(raw: Option<&str>) -> String {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("fatal") => "fatal".to_owned(),
        Some("error") => "error".to_owned(),
        Some("warning" | "warn") => "warning".to_owned(),
        Some("debug") => "debug".to_owned(),
        _ => "info".to_owned(),
    }
}

fn normalize_breadcrumb_timestamp(val: &Value) -> Option<String> {
    match val {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(truncate_chars(trimmed, 64))
            }
        }
        Value::Number(n) => {
            let secs_f64 = n.as_f64()?;
            if !secs_f64.is_finite() || secs_f64 <= 0.0 {
                return None;
            }
            // Handle both Unix seconds (standard Sentry) and milliseconds if accidentally sent.
            let normalized_secs = if secs_f64 > 1_000_000_000_000.0 {
                secs_f64 / 1000.0
            } else {
                secs_f64
            };
            let whole_secs = normalized_secs.trunc() as i64;
            let nanos =
                ((normalized_secs.fract() * 1_000_000_000.0).round() as u32).min(999_999_999);
            DateTime::<Utc>::from_timestamp(whole_secs, nanos).map(|dt| dt.to_rfc3339())
        }
        _ => None,
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        s.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_standard_sentry_breadcrumbs_values_and_normalizes_timestamps() {
        let payload = json!({
            "breadcrumbs": {
                "values": [
                    {
                        "timestamp": 1732250000.5,
                        "type": "navigation",
                        "category": "navigation",
                        "level": "info",
                        "data": { "from": "/cart", "to": "/checkout" }
                    },
                    {
                        "timestamp": "2026-11-22T01:02:03Z",
                        "type": "http",
                        "category": "fetch",
                        "level": "warn",
                        "message": "POST /api/pay returned 502",
                        "data": { "method": "POST", "status_code": 502, "token": "[Filtered]" }
                    },
                    {
                        "category": "console",
                        "level": "error",
                        "message": "Checkout failed"
                    }
                ]
            }
        });

        let crumbs = extract_breadcrumbs(&payload);
        assert_eq!(crumbs.len(), 3);
        assert_eq!(crumbs[0].crumb_type, "navigation");
        assert_eq!(crumbs[0].category, "navigation");
        assert_eq!(crumbs[0].level, "info");
        assert!(
            crumbs[0]
                .timestamp
                .as_deref()
                .is_some_and(|t| t.contains("2024-11-22"))
        );
        assert_eq!(crumbs[1].level, "warning");
        assert_eq!(
            crumbs[1].message.as_deref(),
            Some("POST /api/pay returned 502")
        );
        assert_eq!(crumbs[1].data.as_ref().unwrap()["token"], "[Filtered]");
        assert_eq!(crumbs[2].crumb_type, "default");
        assert_eq!(crumbs[2].category, "console");
        assert_eq!(crumbs[2].level, "error");
        assert!(crumbs[2].data.is_none());
    }

    #[test]
    fn supports_flat_array_and_bounds_breadcrumb_count() {
        let items: Vec<Value> = (0..250)
            .map(|i| json!({ "category": "ui.click", "message": format!("click-{i}") }))
            .collect();
        let payload = json!({ "breadcrumbs": items });

        let crumbs = extract_breadcrumbs(&payload);
        assert_eq!(crumbs.len(), MAX_BREADCRUMBS_PER_EVENT);
        assert_eq!(crumbs[0].message.as_deref(), Some("click-50"));
        assert_eq!(crumbs[199].message.as_deref(), Some("click-249"));
    }
}
