use std::io::Write;

use chrono::Utc;
use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};
use serde_json::json;

use crate::cli::CompressionKind;

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

/// Build a synthetic Sentry Envelope carrying a realistic Python exception event (and optionally an
/// unsupported `session` item to exercise early-drop) compressed according to `compression`.
pub fn build_synthetic_envelope(
    seq: u64,
    unique_issues: usize,
    environment: &str,
    release: Option<&str>,
    include_unsupported_item: bool,
    compression: CompressionKind,
) -> Result<GeneratedEnvelope, String> {
    let issue_bucket = usize::try_from(seq % (unique_issues.max(1) as u64)).unwrap_or(0);
    let exc_type = EXCEPTION_TYPES[issue_bucket % EXCEPTION_TYPES.len()];
    let module = MODULES[(issue_bucket / EXCEPTION_TYPES.len()) % MODULES.len()];
    let fn_name = format!("handle_case_{issue_bucket:04}");
    let filename = format!("{}.py", module.replace('.', "/"));
    let lineno = 40 + (issue_bucket % 180);

    let event_id = deterministic_event_id(seq);
    let sent_at = Utc::now().to_rfc3339();

    let envelope_header = json!({
        "event_id": event_id,
        "sent_at": sent_at,
        "sdk": {
            "name": "sentry.python.fastapi",
            "version": "2.18.0"
        }
    });

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

    let header_bytes = serde_json::to_vec(&envelope_header)
        .map_err(|err| format!("serialize envelope header: {err}"))?;
    let event_bytes =
        serde_json::to_vec(&event_payload).map_err(|err| format!("serialize event item: {err}"))?;

    let mut raw = Vec::with_capacity(header_bytes.len() + event_bytes.len() + 256);
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

    let event_item_header = format!("{{\"type\":\"event\",\"length\":{}}}\n", event_bytes.len());
    raw.extend_from_slice(event_item_header.as_bytes());
    raw.extend_from_slice(&event_bytes);
    raw.push(b'\n');

    let uncompressed_bytes = raw.len();
    let body = compress_payload(&raw, compression)?;

    Ok(GeneratedEnvelope {
        body,
        uncompressed_bytes,
        content_encoding: compression.content_encoding_header(),
    })
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

        let gzipped = build_synthetic_envelope(2, 5, "prod", None, false, CompressionKind::Gzip)
            .expect("gzip ok");
        assert_eq!(gzipped.content_encoding, Some("gzip"));
        let mut gz_out = String::new();
        GzDecoder::new(gzipped.body.as_slice())
            .read_to_string(&mut gz_out)
            .expect("gunzip");
        assert!(gz_out.contains("\"type\":\"event\""));

        let deflated =
            build_synthetic_envelope(3, 5, "prod", None, false, CompressionKind::Deflate)
                .expect("deflate ok");
        assert_eq!(deflated.content_encoding, Some("deflate"));
        let mut def_out = String::new();
        ZlibDecoder::new(deflated.body.as_slice())
            .read_to_string(&mut def_out)
            .expect("inflate");
        assert!(def_out.contains("\"type\":\"event\""));

        let brotlied = build_synthetic_envelope(4, 5, "prod", None, false, CompressionKind::Brotli)
            .expect("brotli ok");
        assert_eq!(brotlied.content_encoding, Some("br"));
        assert!(!brotlied.body.is_empty());
    }
}
