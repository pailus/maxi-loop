use std::io::Read;

use maxi_loop_platform::error::AppError;
use sha2::{Digest, Sha256};
use sourcemap::DecodedMap;

const ZSTD_COMPRESSION_LEVEL: i32 = 3;

pub struct ValidatedSourceMap {
    pub sha256: String,
    pub raw_size_bytes: i32,
    pub compressed_size_bytes: i32,
    pub content_zstd: Vec<u8>,
    pub embedded_debug_id: Option<String>,
}

/// Normalize a JS bundle filename / URL path to a canonical relative path (e.g. `assets/index-abc.js`).
/// Rejects path traversal (`..`), control characters, or empty paths.
pub fn normalize_upload_filename(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 512 {
        return Err(AppError::Validation(
            "filename must be between 1 and 512 characters".to_owned(),
        ));
    }
    if trimmed.chars().any(|c| c.is_control() || c == '\\') {
        return Err(AppError::Validation(
            "filename contains invalid characters".to_owned(),
        ));
    }

    let normalized = canonicalize_js_path(trimmed);
    if normalized.is_empty() || normalized.len() > 512 {
        return Err(AppError::Validation(
            "normalized filename must be between 1 and 512 characters".to_owned(),
        ));
    }

    // Reject any path traversal segments (`..`) or null bytes.
    if normalized.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(AppError::Validation(
            "filename must not contain relative path traversal segments".to_owned(),
        ));
    }

    Ok(normalized)
}

/// Normalize a runtime frame filename/URL or uploaded filename into a canonical matching path:
/// - Strips URL scheme + host (`https://cdn.example.com/assets/app.js?v=1#hash` -> `assets/app.js`)
/// - Strips Sentry `app:///` or `~/` prefix
/// - Strips trailing `.map` suffix so uploading `assets/app.js.map` matches runtime `assets/app.js`
/// - Collapses leading `/`
pub fn canonicalize_js_path(raw: &str) -> String {
    let mut s = raw.trim().replace('\\', "/");

    // Drop query string and fragment
    if let Some((before_q, _)) = s.split_once('?') {
        s = before_q.to_owned();
    }
    if let Some((before_hash, _)) = s.split_once('#') {
        s = before_hash.to_owned();
    }

    // Strip URL scheme + authority (`http(s)://host[:port]/...`, `app:///...`, `webpack:///...`)
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        if let Some((_, path)) = rest.split_once('/') {
            s = path.to_owned();
        } else {
            s = rest.to_owned();
        }
    } else if let Some(rest) = s
        .strip_prefix("app:///")
        .or_else(|| s.strip_prefix("app://"))
        .or_else(|| s.strip_prefix("webpack:///"))
        .or_else(|| s.strip_prefix("webpack://"))
    {
        s = rest.to_owned();
    }

    // Strip leading `~/` and leading `/`
    while let Some(rest) = s.strip_prefix("~/") {
        s = rest.to_owned();
    }
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_owned();
    }
    let s = s.trim_start_matches('/');

    // Strip trailing `.map` if uploader named the artifact `<bundle>.js.map`
    let without_map = s.strip_suffix(".map").unwrap_or(s);
    without_map.trim().to_owned()
}

/// Validate and normalize an optional Debug ID (UUID or 8..=64 hex/hyphen identifier).
pub fn normalize_debug_id(raw: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let lower = trimmed.to_ascii_lowercase();
    if !(8..=64).contains(&lower.len())
        || !lower.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return Err(AppError::Validation(
            "debug_id must be 8..=64 hexadecimal/hyphen characters".to_owned(),
        ));
    }
    Ok(Some(lower))
}

/// Validate raw uploaded Source Map v3 bytes by parsing with `sourcemap::decode_slice`,
/// compute SHA-256, extract any embedded `debugId`/`debug_id`, and compress with zstd.
pub fn validate_and_compress_sourcemap(
    raw_bytes: &[u8],
    max_upload_bytes: usize,
) -> Result<ValidatedSourceMap, AppError> {
    if raw_bytes.is_empty() {
        return Err(AppError::Validation(
            "source map payload must not be empty".to_owned(),
        ));
    }
    if raw_bytes.len() > max_upload_bytes {
        return Err(AppError::PayloadTooLarge(
            "source map payload exceeds maximum allowed upload size",
        ));
    }

    // Validate actual Source Map v3 / Indexed Source Map structure by content (SECURITY.md §7).
    let decoded = sourcemap::decode_slice(raw_bytes)
        .map_err(|_| AppError::Validation("invalid Source Map v3 JSON payload".to_owned()))?;

    // Extract embedded `debugId` from decoded map (or raw JSON fallback for indexed maps).
    let embedded_debug_id = match &decoded {
        DecodedMap::Regular(sm) => sm.get_debug_id(),
        _ => None,
    }
    .map(|id| id.to_string().to_ascii_lowercase())
    .or_else(|| extract_json_debug_id(raw_bytes));

    let sha256 = hex::encode(Sha256::digest(raw_bytes));
    let content_zstd = zstd::stream::encode_all(raw_bytes, ZSTD_COMPRESSION_LEVEL)
        .map_err(|err| AppError::DatabaseQuery(format!("zstd compress source map: {err}")))?;

    let raw_size_bytes = i32::try_from(raw_bytes.len()).map_err(|_| {
        AppError::PayloadTooLarge("source map payload exceeds supported size range")
    })?;
    let compressed_size_bytes = i32::try_from(content_zstd.len()).map_err(|_| {
        AppError::PayloadTooLarge("compressed source map exceeds supported size range")
    })?;

    Ok(ValidatedSourceMap {
        sha256,
        raw_size_bytes,
        compressed_size_bytes,
        content_zstd,
        embedded_debug_id,
    })
}

/// Decompress a stored zstd source map with a strict byte cap and parse lazily via `sourcemap`.
pub fn decompress_and_parse_sourcemap(
    compressed: &[u8],
    max_raw_bytes: usize,
) -> Result<(DecodedMap, usize), AppError> {
    let decoder = zstd::stream::read::Decoder::new(compressed)
        .map_err(|err| AppError::DatabaseQuery(format!("zstd init sourcemap decoder: {err}")))?;

    let limit = (max_raw_bytes + 1) as u64;
    let mut raw = Vec::new();
    decoder
        .take(limit)
        .read_to_end(&mut raw)
        .map_err(|err| AppError::DatabaseQuery(format!("zstd decode sourcemap: {err}")))?;

    if raw.len() > max_raw_bytes {
        return Err(AppError::PayloadTooLarge(
            "decompressed source map exceeds safety limit",
        ));
    }

    let raw_len = raw.len();
    let decoded = sourcemap::decode_slice(&raw)
        .map_err(|err| AppError::DatabaseQuery(format!("parse stored source map: {err}")))?;

    Ok((decoded, raw_len))
}

fn extract_json_debug_id(raw_bytes: &[u8]) -> Option<String> {
    let val: serde_json::Value = serde_json::from_slice(raw_bytes).ok()?;
    let raw_id = val
        .get("debugId")
        .or_else(|| val.get("debug_id"))
        .and_then(serde_json::Value::as_str)?;
    normalize_debug_id(Some(raw_id)).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SOURCEMAP: &str = r#"{
        "version": 3,
        "file": "app.min.js",
        "debugId": "8f2b0d2c-1a2b-4c3d-9e8f-0123456789ab",
        "sources": ["src/Checkout.tsx"],
        "sourcesContent": ["export function submitOrder() {\n  throw new Error('boom');\n}\n"],
        "names": ["submitOrder"],
        "mappings": "AAAA,OAAO,SAASA,cACd,MAAM,IAAI,MAAM,MAClB"
    }"#;

    #[test]
    fn normalizes_urls_and_rejects_traversal() {
        assert_eq!(
            canonicalize_js_path("https://cdn.example.com/assets/index-a1b2.js?v=42#top"),
            "assets/index-a1b2.js"
        );
        assert_eq!(
            canonicalize_js_path("app:///~/assets/index-a1b2.js.map"),
            "assets/index-a1b2.js"
        );
        assert_eq!(
            normalize_upload_filename("~/assets/main.js.map").unwrap(),
            "assets/main.js"
        );
        assert!(normalize_upload_filename("../../etc/passwd").is_err());
        assert!(normalize_upload_filename("assets/../secret.js").is_err());
    }

    #[test]
    fn validates_extracts_debug_id_compresses_and_parses_sourcemap() {
        let validated =
            validate_and_compress_sourcemap(SAMPLE_SOURCEMAP.as_bytes(), 1024 * 1024).unwrap();
        assert_eq!(
            validated.embedded_debug_id.as_deref(),
            Some("8f2b0d2c-1a2b-4c3d-9e8f-0123456789ab")
        );
        assert_eq!(validated.sha256.len(), 64);

        let (decoded, raw_len) =
            decompress_and_parse_sourcemap(&validated.content_zstd, 1024 * 1024).unwrap();
        assert_eq!(raw_len, SAMPLE_SOURCEMAP.len());
        let token = decoded.lookup_token(0, 0).expect("token at 0:0");
        assert_eq!(token.get_source(), Some("src/Checkout.tsx"));
    }

    #[test]
    fn rejects_invalid_content_and_oversized_uploads() {
        assert!(matches!(
            validate_and_compress_sourcemap(b"not json at all", 1024),
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            validate_and_compress_sourcemap(SAMPLE_SOURCEMAP.as_bytes(), 32),
            Err(AppError::PayloadTooLarge(_))
        ));
    }
}
