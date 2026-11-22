use sha2::{Digest, Sha256};

const MAX_NORMALIZED_DESC_CHARS: usize = 512;

/// Normalize a span description (SQL query, HTTP client URL, cache key, or generic operation)
/// into a stable, low-cardinality signature plus its SHA-256 hex digest (`F-PRF-7`).
///
/// Returns `None` if the description is blank after stripping comments/whitespace.
pub fn normalize_span_operation(op: &str, raw_description: &str) -> Option<(String, String)> {
    let trimmed = raw_description.trim();
    if trimmed.is_empty() {
        return None;
    }

    let op_lower = op.trim().to_ascii_lowercase();
    let normalized = if is_db_op(&op_lower) || looks_like_sql(trimmed) {
        normalize_sql_description(trimmed)
    } else if op_lower.starts_with("http") {
        normalize_http_description(trimmed)
    } else if op_lower.starts_with("cache") || op_lower.starts_with("redis") {
        normalize_cache_description(trimmed)
    } else {
        normalize_generic_description(trimmed)
    };

    let cleaned = collapse_whitespace(&normalized);
    if cleaned.is_empty() {
        return None;
    }

    let bounded = truncate_chars(&cleaned, MAX_NORMALIZED_DESC_CHARS);
    let digest = hex::encode(Sha256::digest(bounded.as_bytes()));
    Some((bounded, digest))
}

/// Returns true when a span `op` represents a database query or database operation.
pub fn is_db_op(op_lower: &str) -> bool {
    op_lower == "db" || op_lower.starts_with("db.")
}

fn looks_like_sql(raw: &str) -> bool {
    let head: String = raw
        .trim_start()
        .chars()
        .take(12)
        .collect::<String>()
        .to_ascii_uppercase();
    head.starts_with("SELECT ")
        || head.starts_with("INSERT ")
        || head.starts_with("UPDATE ")
        || head.starts_with("DELETE ")
        || head.starts_with("WITH ")
}

/// Normalize SQL queries without regexes (linear O(n) scan, zero ReDoS risk):
/// - Strips `-- ...` line comments and `/* ... */` block comments
/// - Replaces single-quoted string literals (`'...'`, handling `''` escapes) with `?`
/// - Replaces `$1`, `$2` positional parameters with `?`
/// - Replaces UUIDs, hex literals (`0x...`), and standalone numbers with `?`
/// - Collapses repeated `IN (?, ?, ...)` lists to `IN (?)`
fn normalize_sql_description(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len.min(MAX_NORMALIZED_DESC_CHARS + 32));
    let mut i = 0usize;

    while i < len {
        // Strip `-- ...` SQL line comments
        if i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(' ');
            continue;
        }

        // Strip `/* ... */` SQL block comments
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
            out.push(' ');
            continue;
        }

        // Single-quoted SQL string literal -> `?`
        if bytes[i] == b'\'' {
            i += 1;
            while i < len {
                if bytes[i] == b'\'' {
                    if i + 1 < len && bytes[i + 1] == b'\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                    continue;
                }
                i += 1;
            }
            out.push('?');
            continue;
        }

        // Postgres positional parameter `$1`, `$2` -> `?`
        if bytes[i] == b'$' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
            i += 1;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
            out.push('?');
            continue;
        }

        // UUID literal (36 chars: 8-4-4-4-12 hex) -> `?`
        if is_boundary_before(bytes, i)
            && i + 36 <= len
            && is_uuid_at(&bytes[i..i + 36])
            && is_boundary_after(bytes, i + 36)
        {
            out.push('?');
            i += 36;
            continue;
        }

        // Hex literal `0x...` or `0X...` -> `?`
        if is_boundary_before(bytes, i)
            && i + 2 < len
            && bytes[i] == b'0'
            && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X')
            && bytes[i + 2].is_ascii_hexdigit()
        {
            i += 2;
            while i < len && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            out.push('?');
            continue;
        }

        // Standalone numeric literal (integer / float) -> `?`
        if is_boundary_before(bytes, i) && bytes[i].is_ascii_digit() {
            let mut j = i;
            while j < len && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < len && bytes[j] == b'.' && bytes[j + 1].is_ascii_digit() {
                j += 1;
                while j < len && bytes[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if is_boundary_after(bytes, j) {
                out.push('?');
                i = j;
                continue;
            }
        }

        // Preserve UTF-8 character safely
        if let Some(ch) = raw[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    collapse_sql_in_lists(&collapse_whitespace(&out))
}

/// Collapse `IN (?, ?, ...)` / `IN (?,?,...)` placeholder lists into `IN (?)`.
fn collapse_sql_in_lists(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut i = 0usize;

    while i < n {
        if chars[i] == '(' {
            // Check if parenthesized group contains only `?`, `,`, and spaces (with at least 2 `?`).
            let mut j = i + 1;
            let mut q_count = 0usize;
            let mut valid = true;
            while j < n && chars[j] != ')' {
                match chars[j] {
                    '?' => q_count += 1,
                    ',' | ' ' => {}
                    _ => {
                        valid = false;
                        break;
                    }
                }
                j += 1;
            }
            if valid && j < n && chars[j] == ')' && q_count >= 2 {
                out.push_str("(?)");
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Normalize HTTP client descriptions (e.g. `GET https://api.example.com/users/12345/orders?token=abc`).
fn normalize_http_description(raw: &str) -> String {
    let mut parts = raw.split_whitespace();
    let first = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();

    if rest.is_empty() {
        return normalize_url_like(first);
    }

    // e.g. `POST https://...` or `GET /api/orders/123`
    let mut rebuilt = String::with_capacity(raw.len());
    rebuilt.push_str(first);
    for token in rest {
        rebuilt.push(' ');
        if token.contains('/') || token.contains("://") {
            rebuilt.push_str(&normalize_url_like(token));
        } else {
            rebuilt.push_str(token);
        }
    }
    rebuilt
}

fn normalize_url_like(raw: &str) -> String {
    let without_frag = raw.split('#').next().unwrap_or(raw);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);

    let (prefix, path) = if let Some(scheme_pos) = without_query.find("://") {
        let after_scheme = &without_query[scheme_pos + 3..];
        if let Some(slash_pos) = after_scheme.find('/') {
            let split_at = scheme_pos + 3 + slash_pos;
            (&without_query[..split_at], &without_query[split_at..])
        } else {
            return without_query.to_owned();
        }
    } else {
        ("", without_query)
    };

    let mut out = String::with_capacity(without_query.len());
    out.push_str(prefix);
    for (idx, seg) in path.split('/').enumerate() {
        if idx > 0 {
            out.push('/');
        }
        if seg.is_empty() {
            continue;
        }
        if is_numeric_or_uuid_or_hex_id(seg) {
            out.push_str(":id");
        } else {
            out.push_str(seg);
        }
    }
    out
}

/// Normalize cache/Redis span descriptions (e.g. `redis GET cart:42` -> `redis GET cart:?`).
fn normalize_cache_description(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (idx, token) in raw.split_whitespace().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        if token.contains(':') {
            for (p_idx, part) in token.split(':').enumerate() {
                if p_idx > 0 {
                    out.push(':');
                }
                if !part.is_empty() && is_numeric_or_uuid_or_hex_id(part) {
                    out.push('?');
                } else {
                    out.push_str(part);
                }
            }
        } else if is_numeric_or_uuid_or_hex_id(token) {
            out.push('?');
        } else {
            out.push_str(token);
        }
    }
    out
}

fn normalize_generic_description(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (idx, token) in raw.split_whitespace().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        if token.len() == 36 && is_uuid_at(token.as_bytes()) {
            out.push('?');
        } else {
            out.push_str(token);
        }
    }
    out
}

fn is_numeric_or_uuid_or_hex_id(seg: &str) -> bool {
    let b = seg.as_bytes();
    if b.is_empty() {
        return false;
    }
    if b.iter().all(u8::is_ascii_digit) {
        return true;
    }
    if b.len() == 36 && is_uuid_at(b) {
        return true;
    }
    b.len() >= 16 && b.iter().all(u8::is_ascii_hexdigit)
}

fn is_boundary_before(bytes: &[u8], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = bytes[i - 1];
    !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'$')
}

fn is_boundary_after(bytes: &[u8], j: usize) -> bool {
    if j >= bytes.len() {
        return true;
    }
    let next = bytes[j];
    !(next.is_ascii_alphanumeric() || next == b'_')
}

fn is_uuid_at(slice: &[u8]) -> bool {
    if slice.len() != 36 {
        return false;
    }
    for (idx, &ch) in slice.iter().enumerate() {
        if matches!(idx, 8 | 13 | 18 | 23) {
            if ch != b'-' {
                return false;
            }
        } else if !ch.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn collapse_whitespace(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_ws = true;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn truncate_chars(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        raw.to_owned()
    } else {
        raw.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_sql_literals_params_uuids_comments_and_in_lists() {
        let q1 = "SELECT * FROM orders\n  WHERE tenant_id = 42 AND status = 'paid' AND id IN (1, 2, 3) -- trace comment";
        let q2 = "SELECT * FROM orders WHERE tenant_id = $1 AND status = 'pending' AND id IN (99, 100) /* block */";
        let (n1, h1) = normalize_span_operation("db.sql.query", q1).unwrap();
        let (n2, h2) = normalize_span_operation("db.sql.query", q2).unwrap();

        assert_eq!(
            n1,
            "SELECT * FROM orders WHERE tenant_id = ? AND status = ? AND id IN (?)"
        );
        assert_eq!(n1, n2);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn normalizes_http_client_urls_and_cache_keys() {
        let (http1, hh1) = normalize_span_operation(
            "http.client",
            "GET https://billing.internal/v1/users/12345/invoices/550e8400-e29b-41d4-a716-446655440000?secret=abc",
        )
        .unwrap();
        let (http2, hh2) = normalize_span_operation(
            "http.client",
            "GET https://billing.internal/v1/users/99999/invoices/11111111-2222-3333-4444-555555555555",
        )
        .unwrap();
        assert_eq!(
            http1,
            "GET https://billing.internal/v1/users/:id/invoices/:id"
        );
        assert_eq!(http1, http2);
        assert_eq!(hh1, hh2);

        let (c1, ch1) = normalize_span_operation("cache.get", "redis GET cart:42").unwrap();
        let (c2, ch2) = normalize_span_operation("cache.get", "redis GET cart:999").unwrap();
        assert_eq!(c1, "redis GET cart:?");
        assert_eq!(c1, c2);
        assert_eq!(ch1, ch2);
    }

    #[test]
    fn returns_none_for_blank_or_comment_only_descriptions() {
        assert!(normalize_span_operation("db.sql.query", "   ").is_none());
        assert!(normalize_span_operation("db.sql.query", "-- just a comment").is_none());
    }
}
