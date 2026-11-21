use password_hash::rand_core::{OsRng, RngCore};

/// Generate a 128-bit (16-byte) CSPRNG Sentry DSN public key encoded as 32 lowercase hex characters.
pub fn generate_dsn_public_key() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Format an official Sentry SDK-compatible DSN URL: `{scheme}://{public_key}@{host_and_path}/{project_id}`.
pub fn format_project_dsn(public_base_url: &str, dsn_public_key: &str, project_id: i64) -> String {
    let trimmed = public_base_url.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("https://{dsn_public_key}@{rest}/{project_id}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("http://{dsn_public_key}@{rest}/{project_id}")
    } else {
        format!("http://{dsn_public_key}@{trimmed}/{project_id}")
    }
}

/// Format the envelope ingest endpoint URL for a project.
pub fn format_ingest_endpoint(public_base_url: &str, project_id: i64) -> String {
    let trimmed = public_base_url.trim().trim_end_matches('/');
    format!("{trimmed}/api/{project_id}/envelope/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_32_char_hex_dsn_key_and_formats_dsn_urls() {
        let k1 = generate_dsn_public_key();
        let k2 = generate_dsn_public_key();
        assert_eq!(k1.len(), 32);
        assert_eq!(k2.len(), 32);
        assert_ne!(k1, k2);
        assert!(k1.bytes().all(|b| b.is_ascii_hexdigit()));

        let dsn = format_project_dsn("http://127.0.0.1:8080/", &k1, 42);
        assert_eq!(dsn, format!("http://{k1}@127.0.0.1:8080/42"));

        let https_dsn = format_project_dsn("https://sentry.acme.internal", &k1, 7);
        assert_eq!(https_dsn, format!("https://{k1}@sentry.acme.internal/7"));

        let endpoint = format_ingest_endpoint("http://127.0.0.1:8080/", 42);
        assert_eq!(endpoint, "http://127.0.0.1:8080/api/42/envelope/");
    }
}
