use axum::http::{HeaderMap, header};
use maxi_loop_platform::error::AppError;
use password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const SESSION_COOKIE_NAME: &str = "maxi_loop_session";
pub const CSRF_HEADER_NAME: &str = "x-csrf-token";

/// Generate a 256-bit (32-byte) CSPRNG token encoded as 64 lowercase hex characters.
pub fn generate_random_token_hex() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Validate a raw session token and return its SHA-256 hex digest for database storage/lookup.
/// Raw session tokens are never stored in the database.
pub fn hash_session_token(raw_token: &str) -> Result<String, AppError> {
    let trimmed = raw_token.trim();
    if trimmed.len() != 64 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::Unauthorized("invalid session token"));
    }
    let digest = Sha256::digest(trimmed.as_bytes());
    Ok(hex::encode(digest))
}

/// Compare two CSRF tokens in constant time to prevent timing side-channel attacks.
pub fn verify_csrf_token(expected: &str, provided: &str) -> bool {
    let provided_trimmed = provided.trim();
    if expected.is_empty() || expected.len() != provided_trimmed.len() {
        return false;
    }
    bool::from(expected.as_bytes().ct_eq(provided_trimmed.as_bytes()))
}

/// Extract a named cookie value from the HTTP `Cookie` request headers.
pub fn extract_cookie_value(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    for header_val in headers.get_all(header::COOKIE) {
        let Ok(raw) = header_val.to_str() else {
            continue;
        };
        for part in raw.split(';') {
            let trimmed = part.trim();
            if let Some((name, value)) = trimmed.split_once('=')
                && name.trim() == cookie_name
            {
                let val = value.trim();
                if !val.is_empty() {
                    return Some(val.to_owned());
                }
            }
        }
    }
    None
}

/// Build an `HttpOnly; SameSite=Lax` (and optionally `Secure`) `Set-Cookie` header value.
pub fn build_session_set_cookie(raw_token: &str, max_age_secs: u64, secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE_NAME}={raw_token}; Path=/; Max-Age={max_age_secs}; HttpOnly; SameSite=Lax{secure_attr}"
    )
}

/// Build a `Set-Cookie` header value that immediately expires the session cookie.
pub fn build_clear_session_cookie(secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure_attr}")
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn generates_distinct_256_bit_hex_tokens_and_hashes() {
        let t1 = generate_random_token_hex();
        let t2 = generate_random_token_hex();
        assert_eq!(t1.len(), 64);
        assert_eq!(t2.len(), 64);
        assert_ne!(t1, t2);

        let h1 = hash_session_token(&t1).expect("valid token hash");
        assert_eq!(h1.len(), 64);
        assert_ne!(h1, t1);
        assert_eq!(h1, hash_session_token(&t1).unwrap());
    }

    #[test]
    fn rejects_malformed_session_tokens() {
        assert!(hash_session_token("too_short").is_err());
        assert!(hash_session_token(&"z".repeat(64)).is_err());
    }

    #[test]
    fn verifies_csrf_token_in_constant_time() {
        let token = generate_random_token_hex();
        assert!(verify_csrf_token(&token, &token));
        assert!(!verify_csrf_token(&token, "wrong"));
        assert!(!verify_csrf_token("", ""));
    }

    #[test]
    fn parses_cookie_and_formats_set_cookie_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; maxi_loop_session=abc123token; lang=en"),
        );
        assert_eq!(
            extract_cookie_value(&headers, SESSION_COOKIE_NAME).as_deref(),
            Some("abc123token")
        );

        let set_cookie = build_session_set_cookie("abc123token", 3600, true);
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
        assert!(set_cookie.contains("Secure"));
        assert!(set_cookie.contains("Max-Age=3600"));

        let cleared = build_clear_session_cookie(false);
        assert!(cleared.contains("Max-Age=0"));
        assert!(!cleared.contains("Secure"));
    }
}
