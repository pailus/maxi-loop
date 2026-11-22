//! Users affected per issue (`F018`).
//!
//! The event `user` interface (`id` > `email` > `username` > `ip_address`, Sentry's precedence) is
//! reduced to an in-memory SHA-256 digest *before* PII scrubbing, so custom scrub rules that mask
//! `user.email` do not collapse every user into `[Filtered]`. The raw identity never leaves this
//! module. At flush time the worker turns the digest into a tenant-scoped HMAC-SHA256 keyed by the
//! installation secret `instance_secrets.user_hash_key`; only that opaque value is persisted.

use hmac::{Hmac, Mac};
use maxi_loop_platform::error::AppError;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

/// Unkeyed in-memory digest of the normalized user identity (never persisted).
pub type UserDigest = [u8; 32];

/// Bytes kept from the keyed HMAC for `issue_users.user_hash` / `events.user_hash`.
pub const USER_HASH_LEN: usize = 16;

const FILTERED_VALUE: &str = "[Filtered]";
const AUTO_IP_PLACEHOLDER: &str = "{{auto}}";

/// Derive the user digest from a parsed (not yet scrubbed) event payload.
pub fn user_digest_from_payload(payload: &Value) -> Option<UserDigest> {
    user_digest_from_user_node(payload.get("user")?)
}

/// Derive the user digest from raw event / transaction JSON bytes, deserializing only `user`.
pub fn user_digest_from_raw(raw: &[u8]) -> Option<UserDigest> {
    #[derive(Deserialize)]
    struct UserOnly {
        #[serde(default)]
        user: Option<Value>,
    }
    let parsed: UserOnly = serde_json::from_slice(raw).ok()?;
    user_digest_from_user_node(parsed.user.as_ref()?)
}

fn user_digest_from_user_node(user: &Value) -> Option<UserDigest> {
    let user = user.as_object()?;
    let identity = identity_part(user.get("id"), "id", false)
        .or_else(|| identity_part(user.get("email"), "email", true))
        .or_else(|| identity_part(user.get("username"), "username", false))
        .or_else(|| {
            identity_part(user.get("ip_address"), "ip", true)
                .filter(|ip| ip != &format!("ip:{AUTO_IP_PLACEHOLDER}"))
        })?;
    Some(Sha256::digest(identity.as_bytes()).into())
}

fn identity_part(value: Option<&Value>, kind: &str, lowercase: bool) -> Option<String> {
    let raw = match value? {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if raw.is_empty() || raw == FILTERED_VALUE {
        return None;
    }
    let normalized = if lowercase { raw.to_lowercase() } else { raw };
    Some(format!("{kind}:{normalized}"))
}

/// Tenant-scoped keyed hash persisted for a user: `HMAC-SHA256(key, tenant_id || digest)[..16]`.
/// Scoping by tenant prevents correlating the same person across tenants.
pub fn tenant_user_hash(key: &[u8], tenant_id: i64, digest: &UserDigest) -> Option<Vec<u8>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(&tenant_id.to_be_bytes());
    mac.update(digest);
    let out = mac.finalize().into_bytes();
    Some(out[..USER_HASH_LEN].to_vec())
}

/// Load the installation user-hash key inside a `maxi_loop_worker` transaction.
pub async fn load_user_hash_key(conn: &mut PgConnection) -> Result<Option<Vec<u8>>, AppError> {
    sqlx::query_scalar("SELECT secret FROM instance_secrets WHERE name = 'user_hash_key'")
        .fetch_optional(conn)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("load user hash key: {err}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn prefers_id_then_email_then_username_then_ip() {
        let by_id = user_digest_from_payload(&json!({"user": {"id": "42", "email": "a@x.io"}}));
        let only_id = user_digest_from_payload(&json!({"user": {"id": 42}}));
        assert!(by_id.is_some());
        assert_eq!(by_id, only_id, "numeric and string ids normalize the same");

        let by_email = user_digest_from_payload(&json!({"user": {"email": " Alice@X.io "}}));
        let by_email_lower = user_digest_from_payload(&json!({"user": {"email": "alice@x.io"}}));
        assert_eq!(by_email, by_email_lower);
        assert_ne!(by_email, by_id);

        let by_ip = user_digest_from_payload(&json!({"user": {"ip_address": "10.0.0.1"}}));
        assert!(by_ip.is_some());
        assert_ne!(by_ip, by_email);
    }

    #[test]
    fn ignores_missing_filtered_and_auto_identities() {
        assert!(user_digest_from_payload(&json!({})).is_none());
        assert!(user_digest_from_payload(&json!({"user": {}})).is_none());
        assert!(user_digest_from_payload(&json!({"user": {"id": "  "}})).is_none());
        assert!(user_digest_from_payload(&json!({"user": {"email": "[Filtered]"}})).is_none());
        assert!(user_digest_from_payload(&json!({"user": {"ip_address": "{{auto}}"}})).is_none());
        assert!(user_digest_from_payload(&json!({"user": "alice"})).is_none());
    }

    #[test]
    fn raw_bytes_match_parsed_payload() {
        let payload = json!({"transaction": "/a", "spans": [1, 2], "user": {"id": "u-1"}});
        let raw = serde_json::to_vec(&payload).expect("serialize");
        assert_eq!(
            user_digest_from_raw(&raw),
            user_digest_from_payload(&payload)
        );
        assert!(user_digest_from_raw(b"not json").is_none());
    }

    #[test]
    fn keyed_hash_is_tenant_scoped_and_hides_identity() {
        let digest =
            user_digest_from_payload(&json!({"user": {"email": "alice@x.io"}})).expect("digest");
        let key = [7u8; 32];
        let t1 = tenant_user_hash(&key, 1, &digest).expect("hash");
        let t1_again = tenant_user_hash(&key, 1, &digest).expect("hash");
        let t2 = tenant_user_hash(&key, 2, &digest).expect("hash");
        let other_key = tenant_user_hash(&[9u8; 32], 1, &digest).expect("hash");
        assert_eq!(t1.len(), USER_HASH_LEN);
        assert_eq!(t1, t1_again);
        assert_ne!(t1, t2);
        assert_ne!(t1, other_key);
        assert_ne!(&t1[..], &digest[..USER_HASH_LEN]);
    }
}
