use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub_notify::PiiScrubRuleEntry;
use maxi_loop_sourcemaps::resolver::SourceMapResolver;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::model::QueuedEvent;
use crate::scrub::scrub_event_payload_with_rules;

const MAX_TITLE_CHARS: usize = 512;
const MAX_CULPRIT_CHARS: usize = 256;
const MAX_EXCEPTION_TYPE_CHARS: usize = 128;
const MAX_EXCEPTION_VALUE_CHARS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedStackFrame {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineno: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colno: Option<u64>,
    pub in_app: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_context: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_line: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_context: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedEvent {
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub project_id: i64,
    pub event_id: String,
    pub received_at: chrono::DateTime<chrono::Utc>,
    pub fingerprint: String,
    pub title: String,
    pub exception_type: String,
    pub exception_value: String,
    pub culprit: String,
    pub level: String,
    pub environment: String,
    pub release: Option<String>,
    pub platform: String,
    pub stack_frames: Value,
    pub tags: Value,
    pub payload: Value,
}

#[derive(Clone, Debug)]
struct RawFrame {
    filename: Option<String>,
    abs_path: Option<String>,
    module: Option<String>,
    function: Option<String>,
    lineno: Option<u64>,
    colno: Option<u64>,
    in_app: Option<bool>,
    pre_context: Vec<String>,
    context_line: Option<String>,
    post_context: Vec<String>,
}

/// Scrub default PII from a `QueuedEvent`, extract exception & stack frames, and compute a deterministic
/// grouping fingerprint (exception type + in-app stack frames, with normalized error message fallback).
pub fn process_queued_event(queued: QueuedEvent) -> Result<ProcessedEvent, AppError> {
    process_queued_event_with_rules(queued, &[])
}

/// Scrub mandatory baseline PII + inherited custom PII rules from a `QueuedEvent`, extract exception
/// & stack frames, and compute a deterministic grouping fingerprint.
pub fn process_queued_event_with_rules(
    queued: QueuedEvent,
    custom_rules: &[PiiScrubRuleEntry],
) -> Result<ProcessedEvent, AppError> {
    let mut payload: Value = serde_json::from_slice(&queued.raw_payload)
        .map_err(|_| AppError::Validation("invalid event json payload".to_owned()))?;

    // 1. Always scrub baseline + inherited custom PII before extracting any fields or persisting!
    scrub_event_payload_with_rules(&mut payload, custom_rules);

    finalize_processed_event(queued, payload)
}

/// Scrub mandatory baseline + custom PII, resolve minified JS/React stack frames via the bounded
/// LRU `SourceMapResolver` BEFORE fingerprinting (Design §5.3 & §5.7), and compute the grouping fingerprint.
pub async fn process_queued_event_with_rules_and_sourcemaps(
    queued: QueuedEvent,
    custom_rules: &[PiiScrubRuleEntry],
    resolver: Option<&SourceMapResolver>,
) -> Result<ProcessedEvent, AppError> {
    let mut payload: Value = serde_json::from_slice(&queued.raw_payload)
        .map_err(|_| AppError::Validation("invalid event json payload".to_owned()))?;

    // 1. Always scrub baseline + inherited custom PII first.
    scrub_event_payload_with_rules(&mut payload, custom_rules);

    // 2. Resolve minified JS/React stack frames BEFORE fingerprinting so grouping uses original source.
    if let Some(sm_resolver) = resolver {
        sm_resolver
            .resolve_event_payload(
                queued.tenant_id,
                queued.project_id,
                &queued.platform,
                queued.release.as_deref(),
                &mut payload,
            )
            .await;
    }

    finalize_processed_event(queued, payload)
}

fn finalize_processed_event(
    queued: QueuedEvent,
    payload: Value,
) -> Result<ProcessedEvent, AppError> {
    // 2. Extract primary exception (type, value, raw frames) and fallback message.
    let (exc_type, exc_value, mut raw_frames) = extract_primary_exception(&payload);
    if raw_frames.is_empty() {
        raw_frames = extract_frames_from_stacktrace_node(payload.get("stacktrace"));
    }
    if raw_frames.is_empty() {
        raw_frames = extract_frames_from_threads_node(payload.get("threads"));
    }

    let fallback_message = extract_fallback_message(&payload);
    let effective_exc_value = if !exc_value.is_empty() {
        exc_value
    } else {
        fallback_message.clone()
    };

    // 3. Normalize `in_app` across frames:
    // If at least one frame explicitly sets `in_app: true`, only `Some(true)` frames are in-app.
    // Otherwise, frames that do not explicitly set `in_app: false` are treated as in-app.
    let has_explicit_in_app_true = raw_frames.iter().any(|f| f.in_app == Some(true));
    let normalized_frames: Vec<NormalizedStackFrame> = raw_frames
        .into_iter()
        .map(|f| {
            let in_app = if has_explicit_in_app_true {
                f.in_app == Some(true)
            } else {
                f.in_app.unwrap_or(true)
            };
            NormalizedStackFrame {
                filename: f.filename,
                abs_path: f.abs_path,
                module: f.module,
                function: f.function,
                lineno: f.lineno,
                colno: f.colno,
                in_app,
                pre_context: f.pre_context,
                context_line: f.context_line,
                post_context: f.post_context,
            }
        })
        .collect();

    // 4. Compute fingerprint (exception type + in-app frames, or message fallback).
    let custom_fingerprint = extract_custom_fingerprint(&payload);
    let fingerprint = compute_fingerprint(
        &exc_type,
        &effective_exc_value,
        &normalized_frames,
        custom_fingerprint.as_deref(),
    );

    // 5. Build issue title and culprit.
    let exception_type = truncate_chars(exc_type.trim(), MAX_EXCEPTION_TYPE_CHARS);
    let exception_value = truncate_chars(effective_exc_value.trim(), MAX_EXCEPTION_VALUE_CHARS);
    let title = build_issue_title(&exception_type, &exception_value);
    let transaction_name = payload
        .get("transaction")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let culprit = build_culprit(&normalized_frames, transaction_name);

    // 6. Normalize tags into a JSON object.
    let tags = extract_normalized_tags(payload.get("tags"));
    let stack_frames =
        serde_json::to_value(&normalized_frames).unwrap_or_else(|_| Value::Array(Vec::new()));

    Ok(ProcessedEvent {
        tenant_id: queued.tenant_id,
        workspace_id: queued.workspace_id,
        project_id: queued.project_id,
        event_id: queued.event_id,
        received_at: queued.received_at,
        fingerprint,
        title,
        exception_type,
        exception_value,
        culprit,
        level: queued.level,
        environment: queued.environment,
        release: queued.release,
        platform: queued.platform,
        stack_frames,
        tags,
        payload,
    })
}

/// Compute a 64-character SHA-256 hex fingerprint from exception type, in-app stack frames,
/// or fallback error message.
pub fn compute_fingerprint(
    exception_type: &str,
    error_message: &str,
    frames: &[NormalizedStackFrame],
    custom_fingerprint: Option<&[String]>,
) -> String {
    let default_seed = build_default_fingerprint_seed(exception_type, error_message, frames);

    let final_seed = match custom_fingerprint {
        Some(tokens) if !tokens.is_empty() => {
            let mut parts = Vec::with_capacity(tokens.len());
            for token in tokens {
                let trimmed = token.trim();
                if trimmed == "{{ default }}" || trimmed == "{{default}}" {
                    parts.push(default_seed.clone());
                } else if !trimmed.is_empty() {
                    parts.push(format!("custom:{trimmed}"));
                }
            }
            if parts.is_empty() {
                default_seed
            } else {
                parts.join("|")
            }
        }
        _ => default_seed,
    };

    let mut hasher = Sha256::new();
    hasher.update(final_seed.as_bytes());
    hex::encode(hasher.finalize())
}

fn build_default_fingerprint_seed(
    exception_type: &str,
    error_message: &str,
    frames: &[NormalizedStackFrame],
) -> String {
    let norm_type = exception_type.trim().to_ascii_lowercase();

    // Prefer in-app frames that carry an identifiable module/file or function.
    let mut frame_tokens: Vec<String> = frames
        .iter()
        .filter(|f| f.in_app)
        .filter_map(frame_fingerprint_token)
        .collect();

    // If all frames were marked `in_app = false` (e.g. error inside framework code only),
    // fall back to all identifiable frames before falling back to the raw error message.
    if frame_tokens.is_empty() {
        frame_tokens = frames.iter().filter_map(frame_fingerprint_token).collect();
    }

    if !frame_tokens.is_empty() {
        return format!("exc:{norm_type}|frames:{}", frame_tokens.join(";"));
    }

    let norm_msg = normalize_message_for_fingerprint(error_message);
    format!("exc:{norm_type}|msg:{norm_msg}")
}

fn frame_fingerprint_token(frame: &NormalizedStackFrame) -> Option<String> {
    let location = frame
        .module
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            frame
                .filename
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(normalize_frame_path)
        })
        .or_else(|| {
            frame
                .abs_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(normalize_frame_path)
        });

    let function = frame
        .function
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_rust_symbol);

    match (location, function) {
        (Some(loc), Some(func)) => Some(format!("{loc}:{func}")),
        (Some(loc), None) => {
            if let Some(ctx) = frame
                .context_line
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(format!("{loc}:ctx={ctx}"))
            } else if let Some(line) = frame.lineno {
                Some(format!("{loc}:line={line}"))
            } else {
                Some(loc)
            }
        }
        (None, Some(func)) => Some(format!("<unknown>:{func}")),
        (None, None) => None,
    }
}

fn normalize_frame_path(path: &str) -> String {
    let slash_normalized = path.replace('\\', "/");
    slash_normalized.trim().to_owned()
}

/// Strip compiler hash suffixes like `my_crate::handler::h0123456789abcdef` from Rust backtrace symbols
/// so rebuilds of the same function continue grouping into the same issue.
fn normalize_rust_symbol(func: &str) -> String {
    if let Some((prefix, suffix)) = func.rsplit_once("::h")
        && suffix.len() == 16
        && suffix.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return prefix.to_owned();
    }
    func.to_owned()
}

/// Normalize dynamic tokens (UUIDs, hex addresses, numbers, excess whitespace) in fallback error messages
/// so repeated occurrences with different IDs group cleanly into a single issue.
pub fn normalize_message_for_fingerprint(raw: &str) -> String {
    let first_line = raw.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return "<empty>".to_owned();
    }

    let mut tokens = Vec::new();
    for word in first_line.split_whitespace() {
        let cleaned = word
            .trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']'));
        let replacement = if is_uuid_like(cleaned) {
            "<uuid>"
        } else if is_hex_literal(cleaned) {
            "<hex>"
        } else if is_number_literal(cleaned) {
            "<num>"
        } else {
            cleaned
        };
        tokens.push(replacement.to_ascii_lowercase());
    }

    tokens.join(" ")
}

fn is_uuid_like(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        if matches!(idx, 8 | 13 | 18 | 23) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn is_hex_literal(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit());
    }
    s.len() >= 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_number_literal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && s.bytes().any(|b| b.is_ascii_digit())
}

fn extract_primary_exception(payload: &Value) -> (String, String, Vec<RawFrame>) {
    let values = payload.get("exception").and_then(|exc| {
        exc.get("values")
            .and_then(Value::as_array)
            .or_else(|| exc.as_array())
    });

    let Some(items) = values else {
        return (String::new(), String::new(), Vec::new());
    };

    // Sentry orders chained exceptions oldest-to-newest; select the last item that has data.
    let chosen = items
        .iter()
        .rev()
        .find(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
                || item
                    .get("value")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
                || item.get("stacktrace").is_some()
        })
        .or_else(|| items.last());

    let Some(exc) = chosen else {
        return (String::new(), String::new(), Vec::new());
    };

    let exc_type = exc
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    let exc_value = exc
        .get("value")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    let frames = extract_frames_from_stacktrace_node(exc.get("stacktrace"));

    (exc_type, exc_value, frames)
}

fn extract_frames_from_stacktrace_node(node: Option<&Value>) -> Vec<RawFrame> {
    let Some(st) = node else {
        return Vec::new();
    };
    let frames_val = st
        .get("frames")
        .and_then(Value::as_array)
        .or_else(|| st.as_array());
    let Some(arr) = frames_val else {
        return Vec::new();
    };

    arr.iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            let filename = obj
                .get("filename")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
            let abs_path = obj
                .get("abs_path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
            let module = obj
                .get("module")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    obj.get("package")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .map(ToOwned::to_owned);
            let function = obj
                .get("function")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
            let lineno = obj.get("lineno").and_then(Value::as_u64);
            let colno = obj.get("colno").and_then(Value::as_u64);
            let in_app = obj.get("in_app").and_then(Value::as_bool);
            let pre_context = extract_string_vec(obj.get("pre_context"));
            let context_line = obj
                .get("context_line")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let post_context = extract_string_vec(obj.get("post_context"));

            Some(RawFrame {
                filename,
                abs_path,
                module,
                function,
                lineno,
                colno,
                in_app,
                pre_context,
                context_line,
                post_context,
            })
        })
        .collect()
}

/// Fallback stack frame extraction from `threads.values[].stacktrace` (used by Go `sentry-go`
/// and Rust `sentry` SDKs for captured messages / tracing events with `attach_stacktrace`).
fn extract_frames_from_threads_node(node: Option<&Value>) -> Vec<RawFrame> {
    let Some(threads) = node else {
        return Vec::new();
    };
    let values = threads
        .get("values")
        .and_then(Value::as_array)
        .or_else(|| threads.as_array());
    let Some(items) = values else {
        return Vec::new();
    };

    let chosen = items
        .iter()
        .find(|t| {
            t.get("crashed").and_then(Value::as_bool) == Some(true) && t.get("stacktrace").is_some()
        })
        .or_else(|| {
            items.iter().find(|t| {
                t.get("current").and_then(Value::as_bool) == Some(true)
                    && t.get("stacktrace").is_some()
            })
        })
        .or_else(|| items.iter().find(|t| t.get("stacktrace").is_some()));

    chosen
        .map(|t| extract_frames_from_stacktrace_node(t.get("stacktrace")))
        .unwrap_or_default()
}

fn extract_string_vec(node: Option<&Value>) -> Vec<String> {
    node.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .take(10)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn extract_fallback_message(payload: &Value) -> String {
    if let Some(logentry) = payload.get("logentry") {
        if let Some(formatted) = logentry
            .get("formatted")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return formatted.to_owned();
        }
        if let Some(msg) = logentry
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return msg.to_owned();
        }
    }

    if let Some(msg_node) = payload.get("message") {
        if let Some(s) = msg_node.as_str().map(str::trim).filter(|s| !s.is_empty()) {
            return s.to_owned();
        }
        if let Some(formatted) = msg_node
            .get("formatted")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return formatted.to_owned();
        }
    }

    String::new()
}

fn extract_custom_fingerprint(payload: &Value) -> Option<Vec<String>> {
    let arr = payload.get("fingerprint")?.as_array()?;
    let tokens: Vec<String> = arr
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

fn build_issue_title(exception_type: &str, exception_value: &str) -> String {
    let first_line_val = exception_value.lines().next().unwrap_or("").trim();
    let raw = match (!exception_type.is_empty(), !first_line_val.is_empty()) {
        (true, true) => format!("{exception_type}: {first_line_val}"),
        (true, false) => exception_type.to_owned(),
        (false, true) => first_line_val.to_owned(),
        (false, false) => "<unlabeled event>".to_owned(),
    };
    truncate_chars(&raw, MAX_TITLE_CHARS)
}

fn build_culprit(frames: &[NormalizedStackFrame], transaction: Option<&str>) -> String {
    let top_frame = frames
        .iter()
        .rev()
        .find(|f| f.in_app && (f.module.is_some() || f.filename.is_some() || f.function.is_some()))
        .or_else(|| {
            frames
                .iter()
                .rev()
                .find(|f| f.module.is_some() || f.filename.is_some() || f.function.is_some())
        });

    if let Some(frame) = top_frame {
        let loc = frame
            .module
            .as_deref()
            .or(frame.filename.as_deref())
            .or(frame.abs_path.as_deref());
        let func = frame.function.as_deref();
        let raw = match (loc, func) {
            (Some(l), Some(fn_name)) => format!("{l} in {fn_name}"),
            (Some(l), None) => l.to_owned(),
            (None, Some(fn_name)) => fn_name.to_owned(),
            (None, None) => String::new(),
        };
        if !raw.is_empty() {
            return truncate_chars(&raw, MAX_CULPRIT_CHARS);
        }
    }

    if let Some(tx) = transaction {
        return truncate_chars(tx, MAX_CULPRIT_CHARS);
    }

    String::new()
}

fn extract_normalized_tags(node: Option<&Value>) -> Value {
    let mut map = Map::new();
    let Some(tags_val) = node else {
        return Value::Object(map);
    };

    match tags_val {
        Value::Object(obj) => {
            for (k, v) in obj {
                let key = k.trim();
                if key.is_empty() {
                    continue;
                }
                let val_str = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                map.insert(key.to_owned(), Value::String(val_str));
            }
        }
        Value::Array(arr) => {
            for item in arr {
                if let Some(pair) = item.as_array()
                    && pair.len() == 2
                    && let Some(key) = pair[0].as_str().map(str::trim).filter(|s| !s.is_empty())
                {
                    let val_str = match &pair[1] {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => continue,
                    };
                    map.insert(key.to_owned(), Value::String(val_str));
                }
            }
        }
        _ => {}
    }

    Value::Object(map)
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
    use chrono::Utc;
    use serde_json::json;

    use super::*;

    fn make_queued_event(payload: Value) -> QueuedEvent {
        QueuedEvent {
            tenant_id: 1,
            workspace_id: 1,
            project_id: 1,
            event_id: "11111111111111111111111111111111".to_owned(),
            received_at: Utc::now(),
            level: "error".to_owned(),
            environment: "production".to_owned(),
            release: Some("v1.0.0".to_owned()),
            platform: "python".to_owned(),
            raw_payload: serde_json::to_vec(&payload).expect("serialize test event"),
        }
    }

    #[test]
    fn groups_same_exception_type_and_in_app_frames_despite_different_messages_or_library_frames() {
        let ev1 = process_queued_event(make_queued_event(json!({
            "exception": {
                "values": [{
                    "type": "ZeroDivisionError",
                    "value": "division by zero for invoice 1001",
                    "stacktrace": {
                        "frames": [
                            {
                                "filename": "site-packages/fastapi/routing.py",
                                "module": "fastapi.routing",
                                "function": "run_endpoint_function",
                                "lineno": 190,
                                "in_app": false
                            },
                            {
                                "filename": "app/services/billing.py",
                                "module": "app.services.billing",
                                "function": "compute_discount_ratio",
                                "lineno": 42,
                                "in_app": true
                            }
                        ]
                    }
                }]
            }
        })))
        .expect("process ev1");

        let ev2 = process_queued_event(make_queued_event(json!({
            "exception": {
                "values": [{
                    "type": "ZeroDivisionError",
                    "value": "division by zero for invoice 9999",
                    "stacktrace": {
                        "frames": [
                            {
                                "filename": "site-packages/starlette/concurrency.py",
                                "module": "starlette.concurrency",
                                "function": "run_in_threadpool",
                                "lineno": 35,
                                "in_app": false
                            },
                            {
                                "filename": "app/services/billing.py",
                                "module": "app.services.billing",
                                "function": "compute_discount_ratio",
                                "lineno": 88,
                                "in_app": true
                            }
                        ]
                    }
                }]
            }
        })))
        .expect("process ev2");

        assert_eq!(
            ev1.fingerprint, ev2.fingerprint,
            "same exception type + same in-app module/function must produce identical fingerprint"
        );
        assert_eq!(
            ev1.culprit,
            "app.services.billing in compute_discount_ratio"
        );

        // Different exception type at the same stack trace must produce a DIFFERENT fingerprint.
        let ev_different_type = process_queued_event(make_queued_event(json!({
            "exception": {
                "values": [{
                    "type": "ValueError",
                    "value": "invalid discount",
                    "stacktrace": {
                        "frames": [{
                            "filename": "app/services/billing.py",
                            "module": "app.services.billing",
                            "function": "compute_discount_ratio",
                            "lineno": 42,
                            "in_app": true
                        }]
                    }
                }]
            }
        })))
        .expect("process different type");

        assert_ne!(ev1.fingerprint, ev_different_type.fingerprint);
    }

    #[test]
    fn falls_back_to_normalized_error_message_when_no_stack_frames_present() {
        let ev1 = process_queued_event(make_queued_event(json!({
            "message": "Failed to charge user 550e8400-e29b-41d4-a716-446655440000 after 3 retries"
        })))
        .expect("ev1");

        let ev2 = process_queued_event(make_queued_event(json!({
            "logentry": {
                "formatted": "Failed to charge user 123e4567-e89b-12d3-a456-426614174000 after 5 retries"
            }
        })))
        .expect("ev2");

        let ev3 = process_queued_event(make_queued_event(json!({
            "message": "Database connection pool exhausted"
        })))
        .expect("ev3");

        assert_eq!(ev1.fingerprint, ev2.fingerprint);
        assert_ne!(ev1.fingerprint, ev3.fingerprint);
    }
}
