pub use maxi_loop_platform::scrub::{
    FILTERED_VALUE, is_sensitive_key, scrub_event_payload, scrub_event_payload_with_rules,
    scrub_text_with_rules,
};

#[cfg(test)]
mod tests {
    use maxi_loop_platform::scrub_notify::{PiiScrubRuleEntry, PiiScrubRuleType};
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
