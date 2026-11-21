use std::io::Read;

use maxi_loop_platform::error::AppError;

use crate::model::TraceBlobPayload;

/// Hard upper bound on decompressed trace span blob size (4 MiB) to guard against zip-bomb payloads.
pub const MAX_DECOMPRESSED_TRACE_BLOB_BYTES: usize = 4 * 1024 * 1024;
const ZSTD_COMPRESSION_LEVEL: i32 = 3;

/// Serialize a trace's span payload to JSON and compress it with zstd (Design §5.5).
pub fn compress_trace_blob(blob: &TraceBlobPayload) -> Result<Vec<u8>, AppError> {
    let json_bytes = serde_json::to_vec(blob)
        .map_err(|err| AppError::Validation(format!("serialize trace blob: {err}")))?;

    if json_bytes.len() > MAX_DECOMPRESSED_TRACE_BLOB_BYTES {
        return Err(AppError::PayloadTooLarge(
            "trace span blob exceeds maximum allowed size",
        ));
    }

    zstd::stream::encode_all(json_bytes.as_slice(), ZSTD_COMPRESSION_LEVEL)
        .map_err(|err| AppError::DatabaseQuery(format!("zstd compress trace blob: {err}")))
}

/// Decompress a zstd-compressed trace span blob with a strict output size cap.
pub fn decompress_trace_blob(compressed: &[u8]) -> Result<TraceBlobPayload, AppError> {
    let decoder = zstd::stream::read::Decoder::new(compressed)
        .map_err(|err| AppError::DatabaseQuery(format!("zstd init decoder: {err}")))?;

    let limit = (MAX_DECOMPRESSED_TRACE_BLOB_BYTES + 1) as u64;
    let mut decompressed = Vec::new();
    decoder
        .take(limit)
        .read_to_end(&mut decompressed)
        .map_err(|err| AppError::DatabaseQuery(format!("zstd decode trace blob: {err}")))?;

    if decompressed.len() > MAX_DECOMPRESSED_TRACE_BLOB_BYTES {
        return Err(AppError::PayloadTooLarge(
            "decompressed trace span blob exceeds safety limit",
        ));
    }

    serde_json::from_slice(&decompressed)
        .map_err(|err| AppError::DatabaseQuery(format!("deserialize trace blob: {err}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::TraceSpanRecord;

    #[test]
    fn zstd_roundtrip_compresses_and_restores_trace_blob() {
        let spans: Vec<TraceSpanRecord> = (0..40)
            .map(|i| TraceSpanRecord {
                span_id: format!("{i:016x}"),
                parent_span_id: Some("0000000000000001".to_owned()),
                op: "db.sql.query".to_owned(),
                description: format!("SELECT id, name FROM orders WHERE account_id = $1 -- {i}"),
                status: "ok".to_owned(),
                is_error: false,
                start_timestamp_ms: 1_700_000_000_000.0 + (i as f64) * 5.0,
                end_timestamp_ms: 1_700_000_000_000.0 + (i as f64) * 5.0 + 4.0,
                duration_ms: 4.0,
                tags: json!({"db.system": "postgresql"}),
                data: json!({"db.operation": "SELECT"}),
            })
            .collect();

        let original = TraceBlobPayload {
            spans,
            request: Some(json!({"method": "GET", "url": "https://api.example.com/orders"})),
            measurements: Some(json!({"lcp": {"value": 420.0}})),
            contexts: None,
        };

        let raw_len = serde_json::to_vec(&original).unwrap().len();
        let compressed = compress_trace_blob(&original).expect("compress ok");
        assert!(
            compressed.len() < raw_len / 2,
            "expected zstd compression (<50%): compressed={} raw={}",
            compressed.len(),
            raw_len
        );

        let restored = decompress_trace_blob(&compressed).expect("decompress ok");
        assert_eq!(restored.spans.len(), 40);
        assert_eq!(restored.spans[0].op, "db.sql.query");
        assert_eq!(restored.request, original.request);
    }

    #[test]
    fn rejects_zstd_payload_exceeding_decompression_cap() {
        let huge = vec![b'A'; MAX_DECOMPRESSED_TRACE_BLOB_BYTES + 1024];
        let compressed = zstd::stream::encode_all(huge.as_slice(), 1).expect("encode huge");
        let err = decompress_trace_blob(&compressed).unwrap_err();
        assert!(matches!(err, AppError::PayloadTooLarge(_)));
    }
}
