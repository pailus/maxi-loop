use maxi_loop_platform::scrub_notify::{PiiScrubRuleEntry, PiiScrubRuleType};
use serde_json::{Map, Value};

pub const FILTERED_VALUE: &str = "[Filtered]";
const MAX_SCRUB_DEPTH: usize = 32;

/// Scrub baseline sensitive headers (`Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key`, etc.),
/// sensitive fields (`password`, `token`, `secret`, `api_key`, etc.), query parameters, and
/// `Bearer`/`Basic` credentials in place before an event payload is persisted to PostgreSQL.
pub fn scrub_event_payload(value: &mut Value) {
    scrub_value_recursive(value, 0);
}

/// Apply mandatory baseline PII scrubbing FIRST, followed by any enabled inherited
/// tenant/workspace/project custom scrub rules (additive — baseline rules cannot be disabled).
pub fn scrub_event_payload_with_rules(value: &mut Value, custom_rules: &[PiiScrubRuleEntry]) {
    scrub_event_payload(value);

    let active_rules: Vec<CompiledCustomRule> = custom_rules
        .iter()
        .filter(|r| r.is_enabled && !r.pattern.trim().is_empty())
        .map(CompiledCustomRule::from_entry)
        .collect();

    if active_rules.is_empty() {
        return;
    }

    apply_custom_rules_recursive(value, &active_rules, false, 0);
}

struct CompiledCustomRule {
    rule_type: PiiScrubRuleType,
    normalized_key: String,
    raw_pattern: String,
    replacement: String,
}

impl CompiledCustomRule {
    fn from_entry(entry: &PiiScrubRuleEntry) -> Self {
        let trimmed = entry.pattern.trim();
        Self {
            rule_type: entry.rule_type,
            normalized_key: normalize_key_name(trimmed),
            raw_pattern: trimmed.to_owned(),
            replacement: entry.replacement.clone(),
        }
    }
}

fn normalize_key_name(raw: &str) -> String {
    raw.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == '-' || c == '.' { '_' } else { c })
        .collect()
}

fn apply_custom_rules_recursive(
    value: &mut Value,
    rules: &[CompiledCustomRule],
    in_headers: bool,
    depth: usize,
) {
    if depth > MAX_SCRUB_DEPTH {
        return;
    }

    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                let norm_key = normalize_key_name(key);
                let matched_replacement = rules.iter().find_map(|rule| match rule.rule_type {
                    PiiScrubRuleType::FieldName
                        if !rule.normalized_key.is_empty()
                            && (norm_key == rule.normalized_key
                                || norm_key.contains(&rule.normalized_key)) =>
                    {
                        Some(rule.replacement.as_str())
                    }
                    PiiScrubRuleType::HeaderName
                        if in_headers
                            && !rule.normalized_key.is_empty()
                            && norm_key == rule.normalized_key =>
                    {
                        Some(rule.replacement.as_str())
                    }
                    _ => None,
                });

                if let Some(replacement) = matched_replacement {
                    redact_with_replacement(val, replacement, depth + 1);
                    continue;
                }

                if is_query_string_key(key)
                    && let Value::String(qs) = val
                {
                    *qs = scrub_query_string_custom(qs, rules);
                } else if is_url_key(key)
                    && let Value::String(url) = val
                {
                    *url = scrub_url_query_custom(url, rules);
                }

                let child_in_headers = in_headers || norm_key == "headers";
                apply_custom_rules_recursive(val, rules, child_in_headers, depth + 1);
            }
        }
        Value::Array(items) => {
            if items.len() == 2
                && let Some(key_str) = items[0].as_str()
            {
                let norm_key = normalize_key_name(key_str);
                let matched_replacement = rules.iter().find_map(|rule| match rule.rule_type {
                    PiiScrubRuleType::FieldName
                        if !rule.normalized_key.is_empty()
                            && (norm_key == rule.normalized_key
                                || norm_key.contains(&rule.normalized_key)) =>
                    {
                        Some(rule.replacement.as_str())
                    }
                    PiiScrubRuleType::HeaderName
                        if in_headers
                            && !rule.normalized_key.is_empty()
                            && norm_key == rule.normalized_key =>
                    {
                        Some(rule.replacement.as_str())
                    }
                    _ => None,
                });
                if let Some(replacement) = matched_replacement {
                    redact_with_replacement(&mut items[1], replacement, depth + 1);
                    return;
                }
            }

            for item in items.iter_mut() {
                apply_custom_rules_recursive(item, rules, in_headers, depth + 1);
            }
        }
        Value::String(s) => {
            for rule in rules {
                if rule.rule_type == PiiScrubRuleType::ValueSubstring
                    && !rule.raw_pattern.is_empty()
                    && s.contains(&rule.raw_pattern)
                {
                    *s = s.replace(&rule.raw_pattern, &rule.replacement);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_with_replacement(value: &mut Value, replacement: &str, depth: usize) {
    if depth > MAX_SCRUB_DEPTH {
        *value = Value::String(replacement.to_owned());
        return;
    }
    match value {
        Value::Object(map) => {
            for (_, inner) in map.iter_mut() {
                redact_with_replacement(inner, replacement, depth + 1);
            }
        }
        _ => {
            *value = Value::String(replacement.to_owned());
        }
    }
}

fn scrub_url_query_custom(url: &str, rules: &[CompiledCustomRule]) -> String {
    let Some((base, rest)) = url.split_once('?') else {
        return url.to_owned();
    };
    let (query, fragment) = match rest.split_once('#') {
        Some((q, frag)) => (q, Some(frag)),
        None => (rest, None),
    };
    let scrubbed_q = scrub_query_string_custom(query, rules);
    match fragment {
        Some(frag) => format!("{base}?{scrubbed_q}#{frag}"),
        None => format!("{base}?{scrubbed_q}"),
    }
}

fn scrub_query_string_custom(query: &str, rules: &[CompiledCustomRule]) -> String {
    if query.is_empty() {
        return String::new();
    }
    query
        .split('&')
        .map(|pair| {
            if let Some((k, _v)) = pair.split_once('=') {
                let norm_key = normalize_key_name(k);
                if let Some(rule) = rules.iter().find(|r| {
                    r.rule_type == PiiScrubRuleType::FieldName
                        && !r.normalized_key.is_empty()
                        && (norm_key == r.normalized_key || norm_key.contains(&r.normalized_key))
                }) {
                    return format!("{k}={}", rule.replacement);
                }
            }
            pair.to_owned()
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn scrub_value_recursive(value: &mut Value, depth: usize) {
    if depth > MAX_SCRUB_DEPTH {
        *value = Value::String(FILTERED_VALUE.to_owned());
        return;
    }

    match value {
        Value::Object(map) => scrub_object(map, depth),
        Value::Array(items) => scrub_array(items, depth),
        Value::String(s) => {
            if is_sensitive_auth_value(s) {
                *s = FILTERED_VALUE.to_owned();
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn scrub_object(map: &mut Map<String, Value>, depth: usize) {
    for (key, val) in map.iter_mut() {
        if is_sensitive_key(key) {
            redact_matched_value(val, depth + 1);
            continue;
        }

        if is_query_string_key(key)
            && let Value::String(qs) = val
        {
            *qs = scrub_query_string(qs);
            continue;
        }

        if is_url_key(key)
            && let Value::String(url) = val
        {
            *url = scrub_url_query(url);
            continue;
        }

        scrub_value_recursive(val, depth + 1);
    }
}

fn scrub_array(items: &mut [Value], depth: usize) {
    // Sentry SDKs often represent `request.headers`, `request.query_string`, or `tags`
    // as an array of 2-element `[key, value]` pairs: `[["Authorization", "Bearer ..."], ...]`.
    if items.len() == 2
        && let Some(key_str) = items[0].as_str()
        && is_sensitive_key(key_str)
    {
        redact_matched_value(&mut items[1], depth + 1);
        return;
    }

    for item in items.iter_mut() {
        scrub_value_recursive(item, depth + 1);
    }
}

fn redact_matched_value(value: &mut Value, depth: usize) {
    if depth > MAX_SCRUB_DEPTH {
        *value = Value::String(FILTERED_VALUE.to_owned());
        return;
    }

    match value {
        // If a sensitive container (e.g. `cookies: {"sid": "abc"}`) is a JSON object,
        // preserve the keys while replacing every leaf value inside with `"[Filtered]"`.
        Value::Object(map) => {
            for (_, inner) in map.iter_mut() {
                redact_matched_value(inner, depth + 1);
            }
        }
        _ => {
            *value = Value::String(FILTERED_VALUE.to_owned());
        }
    }
}

/// Returns `true` if a header or field name represents credentials, tokens, cookies, or secrets.
pub fn is_sensitive_key(raw_key: &str) -> bool {
    let trimmed = raw_key.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();
    let normalized: String = lower
        .chars()
        .map(|c| if c == '-' || c == '.' { '_' } else { c })
        .collect();

    matches!(
        normalized.as_str(),
        "authorization"
            | "proxy_authorization"
            | "cookie"
            | "cookies"
            | "set_cookie"
            | "x_api_key"
            | "x_auth_token"
            | "x_csrf_token"
            | "x_xsrf_token"
            | "x_access_token"
            | "pwd"
            | "ssn"
            | "cvv"
            | "cvc"
    ) || normalized.contains("password")
        || normalized.contains("passwd")
        || normalized.contains("passphrase")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("private_key")
        || normalized.contains("access_key")
        || normalized.contains("authorization")
        || normalized.contains("cookie")
        || normalized.contains("session_id")
        || normalized.contains("sessionid")
        || normalized.contains("credit_card")
        || normalized.contains("card_number")
}

fn is_query_string_key(key: &str) -> bool {
    key.trim().eq_ignore_ascii_case("query_string")
}

fn is_url_key(key: &str) -> bool {
    key.trim().eq_ignore_ascii_case("url")
}

fn is_sensitive_auth_value(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    if trimmed.len() < 8 {
        return false;
    }
    let prefix = &trimmed[..7];
    prefix.eq_ignore_ascii_case("bearer ") || trimmed[..6].eq_ignore_ascii_case("basic ")
}

fn scrub_url_query(url: &str) -> String {
    let Some((base, rest)) = url.split_once('?') else {
        return url.to_owned();
    };
    let (query, fragment) = match rest.split_once('#') {
        Some((q, frag)) => (q, Some(frag)),
        None => (rest, None),
    };
    let scrubbed_q = scrub_query_string(query);
    match fragment {
        Some(frag) => format!("{base}?{scrubbed_q}#{frag}"),
        None => format!("{base}?{scrubbed_q}"),
    }
}

fn scrub_query_string(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    query
        .split('&')
        .map(|pair| {
            if let Some((k, _v)) = pair.split_once('=')
                && is_sensitive_key(k)
            {
                format!("{k}={FILTERED_VALUE}")
            } else {
                pair.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn scrubs_sensitive_headers_cookies_and_nested_fields() {
        let mut payload = json!({
            "event_id": "9ec79c33ec9942ab8353589fcb2e04dc",
            "request": {
                "url": "https://api.example.com/v1/login?api_key=real_url_key&page=1",
                "query_string": "token=query_secret_token&sort=asc&password=my_qs_password",
                "headers": {
                    "Authorization": "Bearer eyJhbGciOiJIUzI1NiJ9.secret",
                    "Cookie": "maxi_loop_session=raw_cookie_token",
                    "Set-Cookie": "sid=abc12345",
                    "X-Api-Key": "sk_live_999999",
                    "User-Agent": "sentry.python/2.0.0"
                },
                "cookies": {
                    "session_id": "cookie_val_1",
                    "theme": "cookie_val_2"
                },
                "data": {
                    "username": "alice",
                    "password": "hunter2_password",
                    "nested": {
                        "client_secret": "top_secret_client",
                        "api_key": "ak_123456",
                        "access_token": "at_987654",
                        "safe_field": "kept_value"
                    }
                }
            },
            "extra": {
                "header_tuples": [
                    ["Authorization", "Bearer tuple_secret"],
                    ["Cookie", "sid=tuple_cookie"],
                    ["Accept", "application/json"]
                ],
                "db_password": "postgres_super_secret"
            }
        });

        scrub_event_payload(&mut payload);

        assert_eq!(
            payload["request"]["url"],
            "https://api.example.com/v1/login?api_key=[Filtered]&page=1"
        );
        assert_eq!(
            payload["request"]["query_string"],
            "token=[Filtered]&sort=asc&password=[Filtered]"
        );
        assert_eq!(
            payload["request"]["headers"]["Authorization"],
            FILTERED_VALUE
        );
        assert_eq!(payload["request"]["headers"]["Cookie"], FILTERED_VALUE);
        assert_eq!(payload["request"]["headers"]["Set-Cookie"], FILTERED_VALUE);
        assert_eq!(payload["request"]["headers"]["X-Api-Key"], FILTERED_VALUE);
        assert_eq!(
            payload["request"]["headers"]["User-Agent"],
            "sentry.python/2.0.0"
        );
        assert_eq!(payload["request"]["cookies"]["session_id"], FILTERED_VALUE);
        assert_eq!(payload["request"]["cookies"]["theme"], FILTERED_VALUE);
        assert_eq!(payload["request"]["data"]["username"], "alice");
        assert_eq!(payload["request"]["data"]["password"], FILTERED_VALUE);
        assert_eq!(
            payload["request"]["data"]["nested"]["client_secret"],
            FILTERED_VALUE
        );
        assert_eq!(
            payload["request"]["data"]["nested"]["api_key"],
            FILTERED_VALUE
        );
        assert_eq!(
            payload["request"]["data"]["nested"]["access_token"],
            FILTERED_VALUE
        );
        assert_eq!(
            payload["request"]["data"]["nested"]["safe_field"],
            "kept_value"
        );
        assert_eq!(
            payload["extra"]["header_tuples"][0],
            json!(["Authorization", FILTERED_VALUE])
        );
        assert_eq!(
            payload["extra"]["header_tuples"][1],
            json!(["Cookie", FILTERED_VALUE])
        );
        assert_eq!(
            payload["extra"]["header_tuples"][2],
            json!(["Accept", "application/json"])
        );
        assert_eq!(payload["extra"]["db_password"], FILTERED_VALUE);
    }

    #[test]
    fn applies_configurable_custom_rules_on_top_of_mandatory_baseline_scrubbing() {
        let mut payload = json!({
            "request": {
                "url": "https://api.example.com/v1/users?national_id=ID998877&page=2",
                "headers": {
                    "Authorization": "Bearer raw_baseline_secret",
                    "X-Internal-Employee-Id": "EMP-424242",
                    "User-Agent": "sentry.python/2.0"
                },
                "data": {
                    "customer_tax_code": "TAX-SECRET-99",
                    "note": "Contact internal-secret-domain.corp for escalations"
                }
            }
        });

        let rules = vec![
            PiiScrubRuleEntry {
                id: 1,
                tenant_id: 1,
                workspace_id: None,
                project_id: None,
                rule_type: PiiScrubRuleType::FieldName,
                pattern: "tax_code".to_owned(),
                replacement: "[TaxRedacted]".to_owned(),
                is_enabled: true,
            },
            PiiScrubRuleEntry {
                id: 2,
                tenant_id: 1,
                workspace_id: None,
                project_id: None,
                rule_type: PiiScrubRuleType::FieldName,
                pattern: "national_id".to_owned(),
                replacement: "[Filtered]".to_owned(),
                is_enabled: true,
            },
            PiiScrubRuleEntry {
                id: 3,
                tenant_id: 1,
                workspace_id: Some(1),
                project_id: None,
                rule_type: PiiScrubRuleType::HeaderName,
                pattern: "X-Internal-Employee-Id".to_owned(),
                replacement: "[EmpRedacted]".to_owned(),
                is_enabled: true,
            },
            PiiScrubRuleEntry {
                id: 4,
                tenant_id: 1,
                workspace_id: Some(1),
                project_id: Some(1),
                rule_type: PiiScrubRuleType::ValueSubstring,
                pattern: "internal-secret-domain.corp".to_owned(),
                replacement: "[InternalHost]".to_owned(),
                is_enabled: true,
            },
        ];

        scrub_event_payload_with_rules(&mut payload, &rules);

        assert_eq!(
            payload["request"]["headers"]["Authorization"],
            FILTERED_VALUE
        );
        assert_eq!(
            payload["request"]["headers"]["X-Internal-Employee-Id"],
            "[EmpRedacted]"
        );
        assert_eq!(
            payload["request"]["headers"]["User-Agent"],
            "sentry.python/2.0"
        );
        assert_eq!(
            payload["request"]["url"],
            "https://api.example.com/v1/users?national_id=[Filtered]&page=2"
        );
        assert_eq!(
            payload["request"]["data"]["customer_tax_code"],
            "[TaxRedacted]"
        );
        assert_eq!(
            payload["request"]["data"]["note"],
            "Contact [InternalHost] for escalations"
        );
    }
}
