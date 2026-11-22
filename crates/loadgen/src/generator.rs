use std::io::Write;

use chrono::Utc;
use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};
use serde_json::json;

use crate::cli::{CompressionKind, PayloadKind};

const EXCEPTION_TYPES: [&str; 6] = [
    "ZeroDivisionError",
    "KeyError",
    "ValueError",
    "TimeoutError",
    "IntegrityError",
    "RuntimeError",
];

const MODULES: [&str; 5] = [
    "app.services.billing",
    "app.api.checkout",
    "app.workers.invoice",
    "app.auth.session",
    "app.catalog.inventory",
];

#[derive(Clone, Debug)]
pub struct GeneratedEnvelope {
    pub body: Vec<u8>,
    pub uncompressed_bytes: usize,
    pub content_encoding: Option<&'static str>,
}

/// Every `N_PLUS_ONE_EVERY`-th transaction template issues one query per line item (N+1 DB),
/// so the span performance detectors (`F017`) run on a realistic share of the load.
const N_PLUS_ONE_EVERY: usize = 5;
const N_PLUS_ONE_QUERIES: usize = 6;

/// Build a synthetic Sentry Envelope carrying a realistic Python exception event or transaction
/// (and optionally an unsupported `session` item to exercise early-drop) compressed according to
/// `compression`. `PayloadKind::Mixed` alternates errors (even `seq`) and transactions (odd).
pub fn build_synthetic_envelope(
    seq: u64,
    kind: PayloadKind,
    unique_issues: usize,
    environment: &str,
    release: Option<&str>,
    include_unsupported_item: bool,
    compression: CompressionKind,
) -> Result<GeneratedEnvelope, String> {
    let issue_bucket = usize::try_from(seq % (unique_issues.max(1) as u64)).unwrap_or(0);
    let is_transaction = match kind {
        PayloadKind::Error => false,
        PayloadKind::Transaction => true,
        PayloadKind::Mixed => seq % 2 == 1,
    };
    let (item_type, item_payload) = if is_transaction {
        (
            "transaction",
            transaction_payload(seq, issue_bucket, environment, release),
        )
    } else {
        (
            "event",
            error_payload(seq, issue_bucket, environment, release),
        )
    };
    let event_id = item_payload["event_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let envelope_header = json!({
        "event_id": event_id,
        "sent_at": Utc::now().to_rfc3339(),
        "sdk": {
            "name": "sentry.python.fastapi",
            "version": "2.18.0"
        }
    });

    let header_bytes = serde_json::to_vec(&envelope_header)
        .map_err(|err| format!("serialize envelope header: {err}"))?;
    let item_bytes = serde_json::to_vec(&item_payload)
        .map_err(|err| format!("serialize {item_type} item: {err}"))?;

    let mut raw = Vec::with_capacity(header_bytes.len() + item_bytes.len() + 256);
    raw.extend_from_slice(&header_bytes);
    raw.push(b'\n');

    if include_unsupported_item {
        let session_bytes =
            br#"{"sid":"00000000-0000-4000-8000-000000000001","init":true,"status":"errored"}"#;
        let item_header = format!(
            "{{\"type\":\"session\",\"length\":{}}}\n",
            session_bytes.len()
        );
        raw.extend_from_slice(item_header.as_bytes());
        raw.extend_from_slice(session_bytes);
        raw.push(b'\n');
    }

    let item_header = format!(
        "{{\"type\":\"{item_type}\",\"length\":{}}}\n",
        item_bytes.len()
    );
    raw.extend_from_slice(item_header.as_bytes());
    raw.extend_from_slice(&item_bytes);
    raw.push(b'\n');

    let uncompressed_bytes = raw.len();
    let body = compress_payload(&raw, compression)?;

    Ok(GeneratedEnvelope {
        body,
        uncompressed_bytes,
        content_encoding: compression.content_encoding_header(),
    })
}

fn error_payload(
    seq: u64,
    issue_bucket: usize,
    environment: &str,
    release: Option<&str>,
) -> serde_json::Value {
    let exc_type = EXCEPTION_TYPES[issue_bucket % EXCEPTION_TYPES.len()];
    let module = MODULES[(issue_bucket / EXCEPTION_TYPES.len()) % MODULES.len()];
    let fn_name = format!("handle_case_{issue_bucket:04}");
    let filename = format!("{}.py", module.replace('.', "/"));
    let lineno = 40 + (issue_bucket % 180);

    let event_id = deterministic_event_id(seq);

    let mut event_payload = json!({
        "event_id": event_id,
        "level": "error",
        "platform": "python",
        "environment": environment,
        "transaction": format!("POST /api/v1/case/{issue_bucket}"),
        "tags": {
            "service": "loadgen-synthetic",
            "bucket": issue_bucket.to_string(),
            "runtime": "cpython-3.12"
        },
        "exception": {
            "values": [
                {
                    "type": exc_type,
                    "value": format!("synthetic {exc_type} on request #{seq} (bucket {issue_bucket})"),
                    "mechanism": {
                        "type": "fastapi",
                        "handled": false
                    },
                    "stacktrace": {
                        "frames": [
                            {
                                "filename": "site-packages/fastapi/routing.py",
                                "module": "fastapi.routing",
                                "function": "run_endpoint_function",
                                "lineno": 191,
                                "in_app": false
                            },
                            {
                                "filename": filename,
                                "module": module,
                                "function": fn_name,
                                "lineno": lineno,
                                "in_app": true,
                                "pre_context": [
                                    "def execute_step(ctx):",
                                    "    ratio = ctx.denominator"
                                ],
                                "context_line": "    return ctx.total / ratio",
                                "post_context": [
                                    "    # unreachable on zero denominator"
                                ]
                            }
                        ]
                    }
                }
            ]
        },
        "request": {
            "url": format!("http://app.internal/api/v1/case/{issue_bucket}?page=1"),
            "method": "POST",
            "headers": {
                "User-Agent": "maxi-loop-loadgen/0.1",
                "Content-Type": "application/json"
            }
        }
    });

    if let Some(rel) = release
        && let Some(obj) = event_payload.as_object_mut()
    {
        obj.insert("release".to_owned(), json!(rel));
    }
    event_payload
}

/// A FastAPI-style transaction: auth query, optional N+1 line-item queries, an outbound HTTP
/// call and an audit insert. Durations vary with `seq` so p50/p95 histograms are non-trivial.
fn transaction_payload(
    seq: u64,
    bucket: usize,
    environment: &str,
    release: Option<&str>,
) -> serde_json::Value {
    let event_id = deterministic_event_id(seq);
    let trace_id = deterministic_event_id(seq.rotate_left(17) ^ 0x5bd1_e995);
    let root_span_id = &trace_id[..16];
    let jitter_ms = (seq.wrapping_mul(2_654_435_761) % 120) as f64;

    let mut spans = Vec::new();
    let mut cursor_ms = 2.0;
    let mut push_span = |op: &str, description: String, duration_ms: f64, gap_ms: f64| {
        let n = spans.len() as u64;
        spans.push((
            format!("{:016x}", seq.wrapping_mul(31).wrapping_add(n + 1)),
            op.to_owned(),
            description,
            cursor_ms,
            cursor_ms + duration_ms,
        ));
        cursor_ms += duration_ms + gap_ms;
    };

    push_span(
        "db",
        "SELECT id, email FROM users WHERE session_token = %s".to_owned(),
        3.0 + jitter_ms / 40.0,
        0.5,
    );
    if bucket.is_multiple_of(N_PLUS_ONE_EVERY) {
        for item in 0..N_PLUS_ONE_QUERIES {
            push_span(
                "db",
                format!("SELECT * FROM line_items WHERE order_id = {}", 1000 + item),
                12.0,
                0.2,
            );
        }
    }
    push_span(
        "http.client",
        format!("GET http://inventory.internal/api/stock/{bucket}"),
        18.0 + jitter_ms / 4.0,
        0.5,
    );
    push_span(
        "db",
        "INSERT INTO audit_log (actor, action) VALUES (%s, %s)".to_owned(),
        2.0,
        0.0,
    );
    let total_ms = cursor_ms + 5.0 + jitter_ms;

    let end = Utc::now().timestamp_micros() as f64 / 1e6;
    let start = end - total_ms / 1000.0;
    let at = |offset_ms: f64| start + offset_ms / 1000.0;

    let span_values: Vec<serde_json::Value> = spans
        .into_iter()
        .map(|(span_id, op, description, from_ms, to_ms)| {
            json!({
                "span_id": span_id,
                "parent_span_id": root_span_id,
                "trace_id": trace_id,
                "op": op,
                "description": description,
                "start_timestamp": at(from_ms),
                "timestamp": at(to_ms),
                "status": "ok"
            })
        })
        .collect();

    let mut payload = json!({
        "event_id": event_id,
        "type": "transaction",
        "platform": "python",
        "environment": environment,
        "transaction": format!("GET /api/v1/orders/{bucket}/:id"),
        "transaction_info": { "source": "route" },
        "start_timestamp": start,
        "timestamp": end,
        "contexts": {
            "trace": {
                "trace_id": trace_id,
                "span_id": root_span_id,
                "op": "http.server",
                "status": if seq.is_multiple_of(50) { "internal_error" } else { "ok" }
            }
        },
        "tags": { "service": "loadgen-synthetic" },
        "spans": span_values
    });
    if let Some(rel) = release
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("release".to_owned(), json!(rel));
    }
    payload
}

fn deterministic_event_id(seq: u64) -> String {
    let now_nanos = Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&now_nanos.to_be_bytes());
    bytes[8..].copy_from_slice(&seq.to_be_bytes());
    hex::encode(bytes)
}

fn compress_payload(raw: &[u8], kind: CompressionKind) -> Result<Vec<u8>, String> {
    match kind {
        CompressionKind::None => Ok(raw.to_vec()),
        CompressionKind::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            encoder
                .write_all(raw)
                .map_err(|err| format!("gzip write: {err}"))?;
            encoder
                .finish()
                .map_err(|err| format!("gzip finish: {err}"))
        }
        CompressionKind::Deflate => {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder
                .write_all(raw)
                .map_err(|err| format!("deflate write: {err}"))?;
            encoder
                .finish()
                .map_err(|err| format!("deflate finish: {err}"))
        }
        CompressionKind::Brotli => {
            let mut out = Vec::new();
            {
                let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, 4, 22);
                encoder
                    .write_all(raw)
                    .map_err(|err| format!("brotli write: {err}"))?;
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use flate2::read::{GzDecoder, ZlibDecoder};

    use super::*;

    #[test]
    fn builds_valid_uncompressed_and_compressed_envelopes() {
        let plain = build_synthetic_envelope(
            1,
            PayloadKind::Error,
            5,
            "staging",
            Some("sha-abc1234"),
            true,
            CompressionKind::None,
        )
        .expect("plain ok");
        assert_eq!(plain.content_encoding, None);
        let text = String::from_utf8(plain.body.clone()).expect("utf8");
        assert!(text.contains("\"type\":\"session\""));
        assert!(text.contains("\"type\":\"event\""));
        assert!(text.contains("\"release\":\"sha-abc1234\""));

        let gzipped = build_synthetic_envelope(
            2,
            PayloadKind::Error,
            5,
            "prod",
            None,
            false,
            CompressionKind::Gzip,
        )
        .expect("gzip ok");
        assert_eq!(gzipped.content_encoding, Some("gzip"));
        let mut gz_out = String::new();
        GzDecoder::new(gzipped.body.as_slice())
            .read_to_string(&mut gz_out)
            .expect("gunzip");
        assert!(gz_out.contains("\"type\":\"event\""));

        let deflated = build_synthetic_envelope(
            3,
            PayloadKind::Error,
            5,
            "prod",
            None,
            false,
            CompressionKind::Deflate,
        )
        .expect("deflate ok");
        assert_eq!(deflated.content_encoding, Some("deflate"));
        let mut def_out = String::new();
        ZlibDecoder::new(deflated.body.as_slice())
            .read_to_string(&mut def_out)
            .expect("inflate");
        assert!(def_out.contains("\"type\":\"event\""));

        let brotlied = build_synthetic_envelope(
            4,
            PayloadKind::Error,
            5,
            "prod",
            None,
            false,
            CompressionKind::Brotli,
        )
        .expect("brotli ok");
        assert_eq!(brotlied.content_encoding, Some("br"));
        assert!(!brotlied.body.is_empty());
    }

    #[test]
    fn transaction_envelopes_carry_spans_and_n_plus_one_patterns() {
        let n_plus_one = build_synthetic_envelope(
            0,
            PayloadKind::Transaction,
            10,
            "prod",
            Some("1.0.0"),
            false,
            CompressionKind::None,
        )
        .expect("tx ok");
        let text = String::from_utf8(n_plus_one.body).expect("utf8");
        let mut lines = text.lines();
        let _header = lines.next().expect("envelope header");
        assert!(
            lines
                .next()
                .expect("item header")
                .contains("\"type\":\"transaction\"")
        );
        let item: serde_json::Value =
            serde_json::from_str(lines.next().expect("item")).expect("item json");
        let spans = item["spans"].as_array().expect("spans");
        let repeated = spans
            .iter()
            .filter(|s| {
                s["description"]
                    .as_str()
                    .is_some_and(|d| d.starts_with("SELECT * FROM line_items"))
            })
            .count();
        assert_eq!(repeated, N_PLUS_ONE_QUERIES);
        assert_eq!(item["release"], "1.0.0");
        let root_end = item["timestamp"].as_f64().expect("end");
        assert!(
            spans
                .iter()
                .all(|s| s["timestamp"].as_f64().expect("span end") <= root_end)
        );

        let plain = build_synthetic_envelope(
            1,
            PayloadKind::Transaction,
            10,
            "prod",
            None,
            false,
            CompressionKind::None,
        )
        .expect("tx ok");
        assert!(
            !String::from_utf8(plain.body)
                .expect("utf8")
                .contains("line_items")
        );
    }

    #[test]
    fn mixed_kind_alternates_errors_and_transactions() {
        let item_type = |seq| {
            let env = build_synthetic_envelope(
                seq,
                PayloadKind::Mixed,
                4,
                "prod",
                None,
                false,
                CompressionKind::None,
            )
            .expect("ok");
            let text = String::from_utf8(env.body).expect("utf8");
            text.lines().nth(1).expect("item header").to_owned()
        };
        assert!(item_type(2).contains("\"type\":\"event\""));
        assert!(item_type(3).contains("\"type\":\"transaction\""));
    }
}
