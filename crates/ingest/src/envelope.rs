use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use maxi_loop_platform::error::AppError;
use password_hash::rand_core::{OsRng, RngCore};
use serde::Deserialize;

use crate::model::{QueuedEnvelope, QueuedEvent};

#[derive(Debug, Default, Deserialize)]
pub struct EnvelopeHeader {
    pub event_id: Option<String>,
    pub dsn: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ItemHeader {
    #[serde(rename = "type")]
    item_type: String,
    length: Option<usize>,
}

/// Minimal subset of event JSON fields parsed at ingest time per Design §5.1.
/// The full raw JSON payload bytes are preserved for asynchronous worker processing.
#[derive(Debug, Default, Deserialize)]
struct MinimalEventFields {
    event_id: Option<String>,
    level: Option<String>,
    environment: Option<String>,
    release: Option<String>,
    platform: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEnvelope {
    pub envelope: QueuedEnvelope,
    pub dropped_items: usize,
}

/// Read the first line of a decompressed envelope body and parse the JSON envelope header.
pub fn parse_envelope_header_line(data: &[u8]) -> Result<(EnvelopeHeader, usize), AppError> {
    let Some(newline_pos) = data.iter().position(|&b| b == b'\n') else {
        return Err(AppError::Validation(
            "envelope missing header newline".to_owned(),
        ));
    };

    let header_line = trim_cr(&data[..newline_pos]);
    if header_line.is_empty() {
        return Err(AppError::Validation(
            "envelope header line must not be empty".to_owned(),
        ));
    }

    let header: EnvelopeHeader = serde_json::from_slice(header_line)
        .map_err(|_| AppError::Validation("invalid envelope header json".to_owned()))?;

    Ok((header, newline_pos + 1))
}

/// Extract the Sentry DSN public key from `X-Sentry-Auth`, query `?sentry_key=`, or envelope header `dsn`.
pub fn extract_dsn_public_key(
    headers: &HeaderMap,
    query_sentry_key: Option<&str>,
    envelope_header_dsn: Option<&str>,
) -> Option<String> {
    if let Some(auth_val) = headers.get("x-sentry-auth").and_then(|v| v.to_str().ok())
        && let Some(key) = parse_sentry_auth_header(auth_val)
    {
        return Some(key);
    }

    if let Some(q_key) = query_sentry_key.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(q_key.to_owned());
    }

    if let Some(dsn_url) = envelope_header_dsn
        && let Some(key) = parse_key_from_dsn_url(dsn_url)
    {
        return Some(key);
    }

    None
}

fn parse_sentry_auth_header(raw: &str) -> Option<String> {
    let body = raw
        .trim()
        .strip_prefix("Sentry ")
        .or_else(|| raw.trim().strip_prefix("sentry "))
        .unwrap_or(raw.trim());

    for part in body.split(',') {
        let token = part.trim();
        if let Some((k, v)) = token.split_once('=')
            && k.trim().eq_ignore_ascii_case("sentry_key")
        {
            let key = v.trim().trim_matches('"');
            if !key.is_empty() {
                return Some(key.to_owned());
            }
        }
    }
    None
}

fn parse_key_from_dsn_url(dsn_url: &str) -> Option<String> {
    let trimmed = dsn_url.trim();
    let after_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))?;
    let (userinfo, _host_and_path) = after_scheme.split_once('@')?;
    let public_key = userinfo.split(':').next()?.trim();
    if public_key.is_empty() {
        None
    } else {
        Some(public_key.to_owned())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EnvelopeTarget {
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub project_id: i64,
    pub received_at: DateTime<Utc>,
}

/// Parse an authenticated Sentry envelope body starting after the envelope header (`cursor`),
/// dropping unsupported items (`session`, `replay`, `profile`, `attachment`, etc.) early without
/// deserializing their payloads, and enforcing `max_event_payload_bytes` on `"event"` items.
pub fn parse_envelope_items(
    header: EnvelopeHeader,
    data: &[u8],
    mut cursor: usize,
    max_event_payload_bytes: usize,
    target: EnvelopeTarget,
) -> Result<ParsedEnvelope, AppError> {
    let envelope_id = header
        .event_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .map(ToOwned::to_owned)
        .unwrap_or_else(generate_event_id_hex);

    let mut events = Vec::new();
    let mut dropped_items = 0usize;
    let mut total_items = 0usize;

    while cursor < data.len() {
        // Skip trailing newline(s) at the end of the envelope or after a length-delimited item.
        if data[cursor..].iter().all(|&b| b == b'\n' || b == b'\r') {
            break;
        }

        let rel_newline = data[cursor..]
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or(data.len() - cursor);
        let item_header_line = trim_cr(&data[cursor..cursor + rel_newline]);
        cursor += rel_newline;
        if cursor < data.len() && data[cursor] == b'\n' {
            cursor += 1;
        }

        if item_header_line.is_empty() {
            continue;
        }

        let item_header: ItemHeader = serde_json::from_slice(item_header_line)
            .map_err(|_| AppError::Validation("invalid envelope item header json".to_owned()))?;
        total_items += 1;

        let item_type = item_header.item_type.trim();
        let is_supported_event = item_type == "event";

        // If `length` is declared on a supported event and exceeds `max_event_payload_bytes`,
        // reject immediately with 413 before even reading or slicing the payload.
        if is_supported_event
            && let Some(declared_len) = item_header.length
            && declared_len > max_event_payload_bytes
        {
            return Err(AppError::PayloadTooLarge(
                "event payload exceeds maximum allowed size",
            ));
        }

        let payload_slice: &[u8] = match item_header.length {
            Some(len) => {
                let end = cursor.checked_add(len).ok_or_else(|| {
                    AppError::Validation("envelope item length overflow".to_owned())
                })?;
                if end > data.len() {
                    return Err(AppError::Validation(
                        "envelope item payload truncated".to_owned(),
                    ));
                }
                let slice = &data[cursor..end];
                cursor = end;
                if cursor < data.len() && data[cursor] == b'\r' {
                    cursor += 1;
                }
                if cursor < data.len() && data[cursor] == b'\n' {
                    cursor += 1;
                }
                slice
            }
            None => {
                let rel_end = data[cursor..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .unwrap_or(data.len() - cursor);
                let slice = trim_cr(&data[cursor..cursor + rel_end]);
                cursor += rel_end;
                if cursor < data.len() && data[cursor] == b'\n' {
                    cursor += 1;
                }
                slice
            }
        };

        // Drop unsupported item types (session, replay_*, profile, attachment, client_report, etc.) early!
        if !is_supported_event {
            dropped_items += 1;
            continue;
        }

        if payload_slice.len() > max_event_payload_bytes {
            return Err(AppError::PayloadTooLarge(
                "event payload exceeds maximum allowed size",
            ));
        }
        if payload_slice.is_empty() {
            return Err(AppError::Validation(
                "event item payload must not be empty".to_owned(),
            ));
        }

        let minimal: MinimalEventFields = serde_json::from_slice(payload_slice)
            .map_err(|_| AppError::Validation("invalid event item json".to_owned()))?;

        let event_id = minimal
            .event_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                if events.is_empty() {
                    envelope_id.clone()
                } else {
                    generate_event_id_hex()
                }
            });

        let level = normalize_level(minimal.level.as_deref());
        let environment = minimal
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .unwrap_or("production")
            .to_owned();
        let release = minimal
            .release
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .map(ToOwned::to_owned);
        let platform = minimal
            .platform
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .unwrap_or("other")
            .to_owned();

        events.push(QueuedEvent {
            tenant_id: target.tenant_id,
            workspace_id: target.workspace_id,
            project_id: target.project_id,
            event_id,
            received_at: target.received_at,
            level,
            environment,
            release,
            platform,
            raw_payload: payload_slice.to_vec(),
        });
    }

    if total_items == 0 {
        return Err(AppError::Validation(
            "envelope must contain at least one item".to_owned(),
        ));
    }

    Ok(ParsedEnvelope {
        envelope: QueuedEnvelope {
            tenant_id: target.tenant_id,
            workspace_id: target.workspace_id,
            project_id: target.project_id,
            envelope_id,
            received_at: target.received_at,
            events,
        },
        dropped_items,
    })
}

fn trim_cr(slice: &[u8]) -> &[u8] {
    slice.strip_suffix(b"\r").unwrap_or(slice)
}

fn normalize_level(raw: Option<&str>) -> String {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("fatal") => "fatal".to_owned(),
        Some("error") => "error".to_owned(),
        Some("warning" | "warn") => "warning".to_owned(),
        Some("info" | "log") => "info".to_owned(),
        Some("debug") => "debug".to_owned(),
        _ => "error".to_owned(),
    }
}

fn generate_event_id_hex() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn extracts_dsn_key_from_header_query_or_envelope_dsn() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-sentry-auth",
            HeaderValue::from_static(
                "Sentry sentry_version=7, sentry_client=sentry.python/2.0.0, sentry_key=key_from_header_123",
            ),
        );
        assert_eq!(
            extract_dsn_public_key(
                &headers,
                Some("key_from_query"),
                Some("http://key_from_dsn@host/1")
            )
            .as_deref(),
            Some("key_from_header_123")
        );

        let empty_headers = HeaderMap::new();
        assert_eq!(
            extract_dsn_public_key(&empty_headers, Some("key_from_query"), None).as_deref(),
            Some("key_from_query")
        );
        assert_eq!(
            extract_dsn_public_key(
                &empty_headers,
                None,
                Some("https://key_from_dsn_url@sentry.example.com/42")
            )
            .as_deref(),
            Some("key_from_dsn_url")
        );
    }

    #[test]
    fn drops_unsupported_items_early_and_parses_event_items() {
        let raw = concat!(
            "{\"event_id\":\"9ec79c33ec9942ab8353589fcb2e04dc\"}\n",
            "{\"type\":\"session\"}\n",
            "{\"sid\":\"ignored-session-payload\"}\n",
            "{\"type\":\"replay_event\",\"length\":20}\n",
            "01234567890123456789\n",
            "{\"type\":\"profile\"}\n",
            "{\"profile\":\"dropped\"}\n",
            "{\"type\":\"attachment\",\"length\":11}\n",
            "binary_blob\n",
            "{\"type\":\"event\"}\n",
            "{\"event_id\":\"9ec79c33ec9942ab8353589fcb2e04dc\",\"level\":\"error\",\"environment\":\"staging\",\"release\":\"abc1234\",\"platform\":\"python\"}\n"
        );

        let (header, cursor) = parse_envelope_header_line(raw.as_bytes()).expect("header ok");
        let parsed = parse_envelope_items(
            header,
            raw.as_bytes(),
            cursor,
            4096,
            EnvelopeTarget {
                tenant_id: 1,
                workspace_id: 2,
                project_id: 3,
                received_at: Utc::now(),
            },
        )
        .expect("items ok");

        assert_eq!(parsed.dropped_items, 4);
        assert_eq!(parsed.envelope.events.len(), 1);
        let ev = &parsed.envelope.events[0];
        assert_eq!(ev.event_id, "9ec79c33ec9942ab8353589fcb2e04dc");
        assert_eq!(ev.level, "error");
        assert_eq!(ev.environment, "staging");
        assert_eq!(ev.release.as_deref(), Some("abc1234"));
        assert_eq!(ev.platform, "python");
    }

    #[test]
    fn enforces_max_event_payload_bytes() {
        let raw = concat!(
            "{\"event_id\":\"9ec79c33ec9942ab8353589fcb2e04dc\"}\n",
            "{\"type\":\"event\",\"length\":500}\n",
            "{}\n"
        );
        let (header, cursor) = parse_envelope_header_line(raw.as_bytes()).expect("header ok");
        let err = parse_envelope_items(
            header,
            raw.as_bytes(),
            cursor,
            100,
            EnvelopeTarget {
                tenant_id: 1,
                workspace_id: 2,
                project_id: 3,
                received_at: Utc::now(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, AppError::PayloadTooLarge(_)));
    }
}
