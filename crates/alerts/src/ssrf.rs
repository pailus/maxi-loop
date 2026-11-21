use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use maxi_loop_platform::error::AppError;
use tokio::net::lookup_host;
use url::Url;

#[derive(Clone, Debug)]
pub struct ValidatedTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
    pub resolved_addr: SocketAddr,
}

/// Static URL validation executed at channel creation time (before any network call).
/// Rejects non-HTTP(S) schemes, embedded credentials, loopback/private IPs, and internal hostnames.
pub fn validate_webhook_url_static(
    raw_url: &str,
    allow_private_ips: bool,
) -> Result<Url, AppError> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() || trimmed.len() > 1024 {
        return Err(AppError::Validation(
            "webhook url must be between 1 and 1024 characters".to_owned(),
        ));
    }

    let parsed = Url::parse(trimmed)
        .map_err(|_| AppError::Validation("webhook url must be a valid URL".to_owned()))?;

    let scheme = parsed.scheme();
    if scheme != "https" && scheme != "http" {
        return Err(AppError::Validation(
            "webhook url must use https:// or http://".to_owned(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::Validation(
            "webhook url must not contain embedded credentials".to_owned(),
        ));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::Validation("webhook url missing host".to_owned()))?;

    if !allow_private_ips {
        validate_public_hostname(host)?;
    }

    Ok(parsed)
}

/// Resolve the target URL via DNS and validate EVERY resolved IP address against private, loopback,
/// link-local, cloud metadata (`169.254.169.254`), and CGNAT ranges before connecting.
/// Returns a pinned `SocketAddr` so the caller connects directly to the validated IP (preventing
/// DNS rebinding TOCTOU attacks).
pub async fn resolve_and_validate_url(
    raw_url: &str,
    allow_private_ips: bool,
) -> Result<ValidatedTarget, AppError> {
    let parsed = validate_webhook_url_static(raw_url, allow_private_ips)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::Validation("url missing host".to_owned()))?
        .to_owned();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| AppError::Validation("url missing port".to_owned()))?;

    let addrs: Vec<SocketAddr> = lookup_host((host.as_str(), port))
        .await
        .map_err(|_| AppError::Validation("failed to resolve target host".to_owned()))?
        .collect();

    if addrs.is_empty() {
        return Err(AppError::Validation(
            "target host resolved to no addresses".to_owned(),
        ));
    }

    if !allow_private_ips {
        for addr in &addrs {
            if is_disallowed_ip(addr.ip()) {
                return Err(AppError::Validation(
                    "outbound target resolves to a blocked private, loopback, or metadata IP"
                        .to_owned(),
                ));
            }
        }
    }

    let mut path_and_query = parsed.path().to_owned();
    if path_and_query.is_empty() {
        path_and_query = "/".to_owned();
    }
    if let Some(q) = parsed.query() {
        path_and_query.push('?');
        path_and_query.push_str(q);
    }

    Ok(ValidatedTarget {
        scheme: parsed.scheme().to_owned(),
        host,
        port,
        path_and_query,
        resolved_addr: addrs[0],
    })
}

fn validate_public_hostname(host: &str) -> Result<(), AppError> {
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if lower.is_empty() {
        return Err(AppError::Validation("host must not be empty".to_owned()));
    }

    // Reject bracketed IPv6 or raw IP literals that are private/loopback/metadata.
    let unbracketed = lower
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&lower);
    if let Ok(ip) = unbracketed.parse::<IpAddr>() {
        if is_disallowed_ip(ip) {
            return Err(AppError::Validation(
                "private, loopback, link-local, and metadata IPs are not allowed".to_owned(),
            ));
        }
        return Ok(());
    }

    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower == "metadata.google.internal"
    {
        return Err(AppError::Validation(
            "loopback and internal hostnames are not allowed".to_owned(),
        ));
    }

    Ok(())
}

pub fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_disallowed_v4(v4),
        IpAddr::V6(v6) => is_disallowed_v6(v6),
    }
}

fn is_disallowed_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_documentation()
        // 0.0.0.0/8 ("this host on this network")
        || a == 0
        // 100.64.0.0/10 (Carrier-Grade NAT / shared address space)
        || (a == 100 && (64..=127).contains(&b))
        // 198.18.0.0/15 (benchmarking)
        || (a == 198 && (b == 18 || b == 19))
        // 240.0.0.0/4 (reserved)
        || a >= 240
}

fn is_disallowed_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped_v4) = ip.to_ipv4_mapped() {
        return is_disallowed_v4(mapped_v4);
    }
    let seg0 = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // fc00::/7 (Unique Local Addresses)
        || (seg0 & 0xfe00) == 0xfc00
        // fe80::/10 (Link-Local)
        || (seg0 & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_ssrf_loopback_private_metadata_and_bad_schemes() {
        let blocked = [
            "http://127.0.0.1:8080/hook",
            "http://localhost:8080/hook",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/internal",
            "http://172.16.0.10/internal",
            "http://192.168.1.1/router",
            "http://100.64.0.1/cgnat",
            "http://[::1]:8080/hook",
            "http://[::ffff:127.0.0.1]/hook",
            "https://metadata.google.internal/computeMetadata/v1/",
            "https://user:pass@hooks.example.com/path",
            "file:///etc/passwd",
            "gopher://127.0.0.1:6379/",
        ];
        for url in blocked {
            assert!(
                validate_webhook_url_static(url, false).is_err(),
                "expected SSRF block for {url}"
            );
        }

        assert!(validate_webhook_url_static("https://hooks.example.com/maxi-loop", false).is_ok());
        assert!(validate_webhook_url_static("https://93.184.216.34/webhook", false).is_ok());
    }
}
