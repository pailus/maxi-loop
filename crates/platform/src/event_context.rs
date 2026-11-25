//! Sentry-style event context (`F029`): derive `browser` / `client_os` from the request
//! User-Agent, build the context tags Sentry shows on every issue (`browser`, `runtime`,
//! `server_name`, `url`, ...), and keep a bounded context snapshot of a transaction so
//! performance issues show the same request / runtime details as error issues.
//!
//! Everything here runs on payloads that were already PII-scrubbed.

use serde_json::{Map, Value, json};

use crate::user_agent::{UaComponent, parse_user_agent};

const MAX_TAG_VALUE_CHARS: usize = 200;
const MAX_SNAPSHOT_HEADERS: usize = 48;
const MAX_SNAPSHOT_STRING_CHARS: usize = 1024;
const MAX_SNAPSHOT_CONTEXTS: usize = 16;
/// A single context larger than this (serialized) is dropped from the snapshot.
const MAX_SNAPSHOT_CONTEXT_BYTES: usize = 2048;
/// Contexts that are either shown elsewhere (`trace`) or meaningless outside the SDK.
const SNAPSHOT_SKIPPED_CONTEXTS: &[&str] = &["trace", "profile"];

/// Add `contexts.browser` and an OS context parsed from `request.headers["User-Agent"]`
/// unless the SDK already sent them. Browser SDKs run on the user's machine, so their OS is
/// `os`; for backend platforms `os` means the server, so the user's OS goes to `client_os`.
pub fn enrich_event_context(payload: &mut Value) {
    let Some(ua) = request_header(payload, "user-agent").map(str::to_owned) else {
        return;
    };
    let parsed = parse_user_agent(&ua);
    let os_key = if payload.get("platform").and_then(Value::as_str) == Some("javascript") {
        "os"
    } else {
        "client_os"
    };
    let Some(root) = payload.as_object_mut() else {
        return;
    };
    let contexts = root
        .entry("contexts")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(contexts) = contexts.as_object_mut() else {
        return;
    };
    if let Some(browser) = parsed.browser {
        contexts
            .entry("browser")
            .or_insert_with(|| component_context(&browser, "browser"));
    }
    if let Some(os) = parsed.os {
        contexts
            .entry(os_key)
            .or_insert_with(|| component_context(&os, "os"));
    }
}

/// Insert Sentry's context tags into `tags` (a JSON object). Tags the SDK set itself win.
pub fn merge_context_tags(tags: &mut Value, payload: &Value) {
    let Some(tags) = tags.as_object_mut() else {
        return;
    };
    for (key, value) in context_tags(payload) {
        tags.entry(key).or_insert(Value::String(value));
    }
}

fn context_tags(payload: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let contexts = payload.get("contexts");
    for (ctx_key, tag_key) in [
        ("browser", "browser"),
        ("client_os", "client_os"),
        ("os", "os"),
        ("runtime", "runtime"),
    ] {
        let Some(ctx) = contexts.and_then(|c| c.get(ctx_key)) else {
            continue;
        };
        let Some(name) = str_field(ctx, "name") else {
            continue;
        };
        let full = match str_field(ctx, "version") {
            Some(version) => format!("{name} {version}"),
            None => name.to_owned(),
        };
        out.push((tag_key.to_owned(), truncate(&full)));
        out.push((format!("{tag_key}.name"), truncate(name)));
    }
    if let Some(device) = contexts.and_then(|c| c.get("device")).and_then(|d| {
        str_field(d, "model")
            .or_else(|| str_field(d, "family"))
            .or_else(|| str_field(d, "name"))
    }) {
        out.push(("device".to_owned(), truncate(device)));
    }
    if let Some(server) = str_field(payload, "server_name") {
        out.push(("server_name".to_owned(), truncate(server)));
    }
    if let Some(url) = payload.get("request").and_then(|r| str_field(r, "url")) {
        // Sentry's `url` tag is the URL without query string / fragment.
        let base = url.split(['?', '#']).next().unwrap_or(url);
        out.push(("url".to_owned(), truncate(base)));
    }
    out
}

/// Bounded copy of the transaction fields Sentry shows next to a performance issue:
/// request line + headers (never the body), contexts other than `trace` / `profile`,
/// SDK name/version and `server_name`. Returns `Value::Null` when there is nothing to keep.
pub fn transaction_context_snapshot(payload: &Value) -> Value {
    let mut out = Map::new();
    if let Some(request) = payload.get("request").and_then(Value::as_object) {
        let mut req = Map::new();
        for key in ["method", "url", "query_string"] {
            if let Some(value) = request.get(key).and_then(Value::as_str) {
                req.insert(key.to_owned(), Value::String(truncate_snapshot(value)));
            }
        }
        if let Some(headers) = request.get("headers") {
            let bounded = bounded_headers(headers);
            if !bounded.is_empty() {
                req.insert("headers".to_owned(), Value::Object(bounded));
            }
        }
        if !req.is_empty() {
            out.insert("request".to_owned(), Value::Object(req));
        }
    }
    if let Some(contexts) = payload.get("contexts").and_then(Value::as_object) {
        let kept: Map<String, Value> = contexts
            .iter()
            .filter(|(key, value)| {
                !SNAPSHOT_SKIPPED_CONTEXTS.contains(&key.as_str())
                    && value.is_object()
                    && serde_json::to_vec(value)
                        .is_ok_and(|bytes| bytes.len() <= MAX_SNAPSHOT_CONTEXT_BYTES)
            })
            .take(MAX_SNAPSHOT_CONTEXTS)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if !kept.is_empty() {
            out.insert("contexts".to_owned(), Value::Object(kept));
        }
    }
    if let Some(sdk) = payload.get("sdk") {
        let mut sdk_out = Map::new();
        for key in ["name", "version"] {
            if let Some(value) = str_field(sdk, key) {
                sdk_out.insert(key.to_owned(), Value::String(truncate(value)));
            }
        }
        if !sdk_out.is_empty() {
            out.insert("sdk".to_owned(), Value::Object(sdk_out));
        }
    }
    if let Some(server) = str_field(payload, "server_name") {
        out.insert("server_name".to_owned(), Value::String(truncate(server)));
    }
    if out.is_empty() {
        Value::Null
    } else {
        Value::Object(out)
    }
}

/// Sentry SDKs send headers as an object or as `[name, value]` pairs; names are case-insensitive.
fn request_header<'a>(payload: &'a Value, name: &str) -> Option<&'a str> {
    let headers = payload.get("request")?.get("headers")?;
    let value = match headers {
        Value::Object(map) => map
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v),
        Value::Array(pairs) => pairs.iter().find_map(|pair| {
            let pair = pair.as_array()?;
            let key = pair.first()?.as_str()?;
            key.eq_ignore_ascii_case(name)
                .then(|| pair.get(1))
                .flatten()
        }),
        _ => None,
    }?;
    value.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn bounded_headers(headers: &Value) -> Map<String, Value> {
    let pairs: Vec<(&str, &str)> = match headers {
        Value::Object(map) => map
            .iter()
            .filter_map(|(k, v)| Some((k.as_str(), v.as_str()?)))
            .collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(|pair| {
                let pair = pair.as_array()?;
                Some((pair.first()?.as_str()?, pair.get(1)?.as_str()?))
            })
            .collect(),
        _ => Vec::new(),
    };
    pairs
        .into_iter()
        .filter(|(k, _)| !k.is_empty() && k.len() <= 128)
        .take(MAX_SNAPSHOT_HEADERS)
        .map(|(k, v)| (k.to_owned(), Value::String(truncate_snapshot(v))))
        .collect()
}

fn component_context(component: &UaComponent, kind: &str) -> Value {
    let mut ctx = json!({ "type": kind, "name": component.name });
    if let (Some(version), Some(obj)) = (&component.version, ctx.as_object_mut()) {
        obj.insert("version".to_owned(), Value::String(version.clone()));
    }
    ctx
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn truncate(s: &str) -> String {
    truncate_to(s, MAX_TAG_VALUE_CHARS)
}

fn truncate_snapshot(s: &str) -> String {
    truncate_to(s, MAX_SNAPSHOT_STRING_CHARS)
}

fn truncate_to(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => s[..idx].to_owned(),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME_WIN: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

    fn django_payload() -> Value {
        json!({
            "platform": "python",
            "server_name": "api-1",
            "contexts": {
                "runtime": {"name": "CPython", "version": "3.13.4"},
                "trace": {"trace_id": "abc", "op": "http.server"},
                "profile": {"profiler_id": "p"},
            },
            "request": {
                "method": "GET",
                "url": "https://api.example.com/pegawai/?page=2",
                "query_string": "page=2",
                "headers": {"user-agent": CHROME_WIN, "Authorization": "[Filtered]"},
                "data": {"secret_body": "x"},
            },
            "sdk": {"name": "sentry.python.django", "version": "2.29.1", "integrations": ["django"]},
            "tags": {"browser": "custom"},
        })
    }

    #[test]
    fn backend_event_gets_browser_and_client_os_from_user_agent() {
        let mut payload = django_payload();
        enrich_event_context(&mut payload);
        let ctx = &payload["contexts"];
        assert_eq!(ctx["browser"]["name"], "Chrome");
        assert_eq!(ctx["browser"]["version"], "131.0.0.0");
        assert_eq!(ctx["client_os"]["name"], "Windows");
        assert!(ctx.get("os").is_none(), "server OS must not be overwritten");
    }

    #[test]
    fn browser_sdk_os_goes_to_os_and_sdk_contexts_are_kept() {
        let mut payload = json!({
            "platform": "javascript",
            "contexts": {"browser": {"name": "Firefox", "version": "1"}},
            "request": {"headers": [["User-Agent", CHROME_WIN]]},
        });
        enrich_event_context(&mut payload);
        assert_eq!(payload["contexts"]["browser"]["name"], "Firefox");
        assert_eq!(payload["contexts"]["os"]["name"], "Windows");
    }

    #[test]
    fn context_tags_follow_sentry_names_and_sdk_tags_win() {
        let mut payload = django_payload();
        enrich_event_context(&mut payload);
        let mut tags = json!({"browser": "custom"});
        merge_context_tags(&mut tags, &payload);
        assert_eq!(tags["browser"], "custom");
        assert_eq!(tags["browser.name"], "Chrome");
        assert_eq!(tags["client_os"], "Windows 10");
        assert_eq!(tags["runtime"], "CPython 3.13.4");
        assert_eq!(tags["runtime.name"], "CPython");
        assert_eq!(tags["server_name"], "api-1");
        assert_eq!(tags["url"], "https://api.example.com/pegawai/");
    }

    #[test]
    fn snapshot_keeps_request_line_headers_contexts_and_sdk_but_not_body_or_trace() {
        let mut payload = django_payload();
        enrich_event_context(&mut payload);
        let snap = transaction_context_snapshot(&payload);
        assert_eq!(snap["request"]["method"], "GET");
        assert_eq!(snap["request"]["headers"]["Authorization"], "[Filtered]");
        assert!(snap["request"].get("data").is_none());
        assert_eq!(snap["contexts"]["runtime"]["name"], "CPython");
        assert_eq!(snap["contexts"]["browser"]["name"], "Chrome");
        assert!(snap["contexts"].get("trace").is_none());
        assert!(snap["contexts"].get("profile").is_none());
        assert_eq!(
            snap["sdk"],
            json!({"name": "sentry.python.django", "version": "2.29.1"})
        );
        assert_eq!(snap["server_name"], "api-1");
    }

    #[test]
    fn snapshot_is_bounded() {
        let headers: Map<String, Value> = (0..200)
            .map(|i| (format!("X-H{i}"), Value::String("v".repeat(5000))))
            .collect();
        let payload = json!({
            "request": {"headers": headers},
            "contexts": {"huge": {"blob": "x".repeat(10_000)}, "app": {"name": "a"}},
        });
        let snap = transaction_context_snapshot(&payload);
        let kept = snap["request"]["headers"].as_object().map(Map::len);
        assert_eq!(kept, Some(MAX_SNAPSHOT_HEADERS));
        let one = snap["request"]["headers"]["X-H0"].as_str().map(str::len);
        assert_eq!(one, Some(MAX_SNAPSHOT_STRING_CHARS));
        assert!(snap["contexts"].get("huge").is_none());
        assert_eq!(snap["contexts"]["app"]["name"], "a");
    }

    #[test]
    fn nothing_to_enrich_without_user_agent() {
        let mut payload = json!({"platform": "python"});
        enrich_event_context(&mut payload);
        assert_eq!(payload, json!({"platform": "python"}));
        assert_eq!(transaction_context_snapshot(&payload), Value::Null);
    }
}
