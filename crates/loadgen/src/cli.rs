use std::fmt;
use std::time::Duration;

const MAX_CONCURRENCY: usize = 512;
const MAX_TOTAL_REQUESTS: u64 = 10_000_000;
const MAX_DURATION_SECS: u64 = 3_600;
const MAX_UNIQUE_ISSUES: usize = 100_000;
const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionKind {
    None,
    Gzip,
    Deflate,
    Brotli,
}

impl CompressionKind {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" | "identity" => Ok(Self::None),
            "gzip" | "gz" => Ok(Self::Gzip),
            "deflate" => Ok(Self::Deflate),
            "br" | "brotli" => Ok(Self::Brotli),
            other => Err(format!(
                "unsupported compression '{other}' (expected none, gzip, deflate, or br)"
            )),
        }
    }

    pub fn content_encoding_header(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Gzip => Some("gzip"),
            Self::Deflate => Some("deflate"),
            Self::Brotli => Some("br"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Brotli => "br",
        }
    }
}

/// Which Sentry item type each envelope carries (`F021`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    Error,
    Transaction,
    /// Alternates error and transaction envelopes.
    Mixed,
}

impl PayloadKind {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "error" | "event" => Ok(Self::Error),
            "transaction" | "tx" => Ok(Self::Transaction),
            "mixed" => Ok(Self::Mixed),
            other => Err(format!(
                "unsupported kind '{other}' (expected error, transaction, or mixed)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Transaction => "transaction",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadMode {
    TotalRequests(u64),
    Duration(Duration),
}

#[derive(Clone, PartialEq, Eq)]
pub struct ParsedDsn {
    pub public_key: String,
    pub host: String,
    pub port: u16,
    pub project_id: i64,
    pub base_path: String,
}

impl fmt::Debug for ParsedDsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedDsn")
            .field("public_key", &"[REDACTED]")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("project_id", &self.project_id)
            .field("base_path", &self.base_path)
            .finish()
    }
}

impl ParsedDsn {
    /// Parse an HTTP Sentry DSN of the form `http://<public_key>@<host>[:<port>][/<prefix>]/<project_id>`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        let after_scheme = if let Some(rest) = trimmed.strip_prefix("http://") {
            rest
        } else if trimmed.starts_with("https://") {
            return Err(
                "https:// DSN is not supported by maxi-loop-loadgen (use local/internal http:// endpoint)"
                    .to_owned(),
            );
        } else {
            return Err("DSN must start with http://".to_owned());
        };

        let (userinfo, host_and_path) = after_scheme
            .split_once('@')
            .ok_or_else(|| "DSN missing '@' separator between public key and host".to_owned())?;

        let public_key = userinfo.split(':').next().unwrap_or("").trim();
        if public_key.is_empty()
            || public_key.len() > 128
            || !public_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid DSN public key".to_owned());
        }

        let (authority, path_part) = host_and_path
            .split_once('/')
            .ok_or_else(|| "DSN missing '/<project_id>' path".to_owned())?;

        let (host, port) = parse_authority(authority)?;

        let clean_path = path_part.split('?').next().unwrap_or("").trim_matches('/');
        if clean_path.is_empty() {
            return Err("DSN missing numeric project_id".to_owned());
        }

        let (base_path, project_str) = match clean_path.rsplit_once('/') {
            Some((prefix, proj)) => (format!("/{prefix}"), proj),
            None => (String::new(), clean_path),
        };

        let project_id: i64 = project_str
            .parse()
            .map_err(|_| "DSN project_id must be a positive integer".to_owned())?;
        if project_id <= 0 {
            return Err("DSN project_id must be a positive integer".to_owned());
        }

        Ok(Self {
            public_key: public_key.to_owned(),
            host,
            port,
            project_id,
            base_path,
        })
    }

    pub fn connect_addr(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn host_header(&self) -> String {
        if self.port == 80 {
            if self.host.contains(':') {
                format!("[{}]", self.host)
            } else {
                self.host.clone()
            }
        } else {
            self.connect_addr()
        }
    }

    pub fn envelope_path(&self) -> String {
        format!("{}/api/{}/envelope/", self.base_path, self.project_id)
    }

    pub fn sentry_auth_header(&self) -> String {
        format!(
            "Sentry sentry_version=7, sentry_client=maxi-loop-loadgen/0.1, sentry_key={}",
            self.public_key
        )
    }

    pub fn masked_target(&self) -> String {
        format!("http://{}{}", self.host_header(), self.envelope_path())
    }
}

fn parse_authority(authority: &str) -> Result<(String, u16), String> {
    let trimmed = authority.trim();
    if trimmed.is_empty() {
        return Err("DSN missing host".to_owned());
    }

    if let Some(rest) = trimmed.strip_prefix('[') {
        let (ipv6, after_bracket) = rest
            .split_once(']')
            .ok_or_else(|| "invalid IPv6 host in DSN".to_owned())?;
        if ipv6.is_empty() {
            return Err("empty IPv6 host in DSN".to_owned());
        }
        if after_bracket.is_empty() {
            return Ok((ipv6.to_owned(), 80));
        }
        let port_str = after_bracket
            .strip_prefix(':')
            .ok_or_else(|| "invalid port separator after IPv6 host".to_owned())?;
        let port: u16 = port_str
            .parse()
            .map_err(|_| "invalid port number in DSN".to_owned())?;
        if port == 0 {
            return Err("DSN port must be >= 1".to_owned());
        }
        return Ok((ipv6.to_owned(), port));
    }

    match trimmed.rsplit_once(':') {
        Some((host, port_str)) => {
            if host.is_empty() {
                return Err("DSN missing host".to_owned());
            }
            let port: u16 = port_str
                .parse()
                .map_err(|_| "invalid port number in DSN".to_owned())?;
            if port == 0 {
                return Err("DSN port must be >= 1".to_owned());
            }
            Ok((host.to_owned(), port))
        }
        None => Ok((trimmed.to_owned(), 80)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadgenConfig {
    pub dsn: ParsedDsn,
    pub mode: LoadMode,
    pub concurrency: usize,
    pub unique_issues: usize,
    pub payload_kind: PayloadKind,
    pub compression: CompressionKind,
    pub environment: String,
    pub release: Option<String>,
    pub request_timeout: Duration,
    pub include_unsupported_item: bool,
    pub json_output: bool,
}

pub fn usage_text() -> &'static str {
    "Usage: maxi-loop-loadgen --dsn <HTTP_SENTRY_DSN> [OPTIONS]\n\
     \n\
     Options:\n\
       --dsn <URL>                  Target Sentry DSN (or set SENTRY_DSN env var)\n\
       --total <N>                  Total envelopes to send (default: 2000; mutually exclusive with --duration-secs)\n\
       --duration-secs <S>          Run load test for S seconds (1..=3600)\n\
       --concurrency <C>            Concurrent HTTP/1.1 keep-alive workers (default: 16, max: 512)\n\
       --unique-issues <U>          Distinct synthetic issue / transaction templates (default: 20, max: 100000)\n\
       --kind <KIND>                error | transaction | mixed (default: error)\n\
       --compression <KIND>         none | gzip | deflate | br (default: gzip)\n\
       --environment <ENV>          Event environment tag (default: loadtest)\n\
       --release <REL>              Optional release identifier\n\
       --timeout-ms <MS>            Per-request timeout in ms (default: 5000, range: 100..=60000)\n\
       --include-unsupported-item   Attach a session item to each envelope to test early-drop\n\
       --json                       Print machine-readable JSON summary\n\
       -h, --help                   Show this help message"
}

pub fn parse_args_with_env<I, S>(
    args: I,
    env_dsn: Option<&str>,
) -> Result<Option<LoadgenConfig>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut iter = args.into_iter().map(|s| s.as_ref().to_owned());
    let _bin = iter.next();

    let mut dsn_raw: Option<String> = env_dsn
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let mut total: Option<u64> = None;
    let mut duration_secs: Option<u64> = None;
    let mut concurrency: usize = 16;
    let mut unique_issues: usize = 20;
    let mut compression = CompressionKind::Gzip;
    let mut payload_kind = PayloadKind::Error;
    let mut environment = "loadtest".to_owned();
    let mut release: Option<String> = None;
    let mut timeout_ms: u64 = 5_000;
    let mut include_unsupported_item = false;
    let mut json_output = false;

    while let Some(flag) = iter.next() {
        let take_val = |it: &mut dyn Iterator<Item = String>, name: &str| {
            it.next()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("missing value for {name}"))
        };

        match flag.as_str() {
            "-h" | "--help" => return Ok(None),
            "--dsn" => {
                dsn_raw = Some(take_val(&mut iter, "--dsn")?);
            }
            "--total" => {
                let raw = take_val(&mut iter, "--total")?;
                let val: u64 = raw
                    .parse()
                    .map_err(|_| "--total must be a positive integer".to_owned())?;
                total = Some(val);
            }
            "--duration-secs" => {
                let raw = take_val(&mut iter, "--duration-secs")?;
                let val: u64 = raw
                    .parse()
                    .map_err(|_| "--duration-secs must be a positive integer".to_owned())?;
                duration_secs = Some(val);
            }
            "--concurrency" => {
                let raw = take_val(&mut iter, "--concurrency")?;
                concurrency = raw
                    .parse()
                    .map_err(|_| "--concurrency must be a positive integer".to_owned())?;
            }
            "--unique-issues" => {
                let raw = take_val(&mut iter, "--unique-issues")?;
                unique_issues = raw
                    .parse()
                    .map_err(|_| "--unique-issues must be a positive integer".to_owned())?;
            }
            "--compression" => {
                let raw = take_val(&mut iter, "--compression")?;
                compression = CompressionKind::parse(&raw)?;
            }
            "--kind" => {
                let raw = take_val(&mut iter, "--kind")?;
                payload_kind = PayloadKind::parse(&raw)?;
            }
            "--environment" => {
                environment = take_val(&mut iter, "--environment")?;
            }
            "--release" => {
                release = Some(take_val(&mut iter, "--release")?);
            }
            "--timeout-ms" => {
                let raw = take_val(&mut iter, "--timeout-ms")?;
                timeout_ms = raw
                    .parse()
                    .map_err(|_| "--timeout-ms must be a positive integer".to_owned())?;
            }
            "--include-unsupported-item" => {
                include_unsupported_item = true;
            }
            "--json" => {
                json_output = true;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let dsn_str = dsn_raw.ok_or_else(|| {
        "missing target DSN (pass --dsn http://<key>@<host>:<port>/<project_id> or set SENTRY_DSN)"
            .to_owned()
    })?;
    let dsn = ParsedDsn::parse(&dsn_str)?;

    let mode = match (total, duration_secs) {
        (Some(_), Some(_)) => {
            return Err("--total and --duration-secs are mutually exclusive".to_owned());
        }
        (Some(n), None) => {
            if !(1..=MAX_TOTAL_REQUESTS).contains(&n) {
                return Err(format!(
                    "--total must be between 1 and {MAX_TOTAL_REQUESTS}"
                ));
            }
            LoadMode::TotalRequests(n)
        }
        (None, Some(secs)) => {
            if !(1..=MAX_DURATION_SECS).contains(&secs) {
                return Err(format!(
                    "--duration-secs must be between 1 and {MAX_DURATION_SECS}"
                ));
            }
            LoadMode::Duration(Duration::from_secs(secs))
        }
        (None, None) => LoadMode::TotalRequests(2_000),
    };

    if !(1..=MAX_CONCURRENCY).contains(&concurrency) {
        return Err(format!(
            "--concurrency must be between 1 and {MAX_CONCURRENCY}"
        ));
    }

    if !(1..=MAX_UNIQUE_ISSUES).contains(&unique_issues) {
        return Err(format!(
            "--unique-issues must be between 1 and {MAX_UNIQUE_ISSUES}"
        ));
    }

    let env_trimmed = environment.trim();
    if env_trimmed.is_empty() || env_trimmed.len() > 64 {
        return Err("--environment must be between 1 and 64 characters".to_owned());
    }
    let release_clean = match release {
        Some(r) => {
            let trimmed = r.trim();
            if trimmed.is_empty() || trimmed.len() > 128 {
                return Err("--release must be between 1 and 128 characters".to_owned());
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(format!(
            "--timeout-ms must be between {MIN_TIMEOUT_MS} and {MAX_TIMEOUT_MS}"
        ));
    }

    Ok(Some(LoadgenConfig {
        dsn,
        mode,
        concurrency,
        unique_issues,
        payload_kind,
        compression,
        environment: env_trimmed.to_owned(),
        release: release_clean,
        request_timeout: Duration::from_millis(timeout_ms),
        include_unsupported_item,
        json_output,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_dsn_and_redacts_key_in_debug() {
        let dsn = ParsedDsn::parse("http://abcdef0123456789@127.0.0.1:8080/42").expect("valid dsn");
        assert_eq!(dsn.public_key, "abcdef0123456789");
        assert_eq!(dsn.host, "127.0.0.1");
        assert_eq!(dsn.port, 8080);
        assert_eq!(dsn.project_id, 42);
        assert_eq!(dsn.envelope_path(), "/api/42/envelope/");
        assert_eq!(
            dsn.masked_target(),
            "http://127.0.0.1:8080/api/42/envelope/"
        );

        let debug_str = format!("{dsn:?}");
        assert!(!debug_str.contains("abcdef0123456789"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn rejects_invalid_dsn_and_conflicting_flags() {
        assert!(ParsedDsn::parse("https://key@example.com/1").is_err());
        assert!(ParsedDsn::parse("http://missing_at_host/1").is_err());
        assert!(ParsedDsn::parse("http://key@127.0.0.1:8080/not_a_number").is_err());
        assert!(ParsedDsn::parse("http://key@127.0.0.1:8080/0").is_err());

        let err = parse_args_with_env(
            [
                "maxi-loop-loadgen",
                "--dsn",
                "http://abc123@127.0.0.1:8080/1",
                "--total",
                "100",
                "--duration-secs",
                "5",
            ],
            None,
        )
        .unwrap_err();
        assert!(err.contains("mutually exclusive"));

        let err = parse_args_with_env(
            [
                "maxi-loop-loadgen",
                "--dsn",
                "http://abc123@127.0.0.1:8080/1",
                "--concurrency",
                "0",
            ],
            None,
        )
        .unwrap_err();
        assert!(err.contains("--concurrency"));
    }

    #[test]
    fn parses_payload_kind_and_rejects_unknown() {
        let base = [
            "maxi-loop-loadgen",
            "--dsn",
            "http://abc123@127.0.0.1:8080/1",
        ];
        let cfg = parse_args_with_env(base, None)
            .expect("parse ok")
            .expect("config");
        assert_eq!(cfg.payload_kind, PayloadKind::Error);

        let cfg = parse_args_with_env(base.into_iter().chain(["--kind", "mixed"]), None)
            .expect("parse ok")
            .expect("config");
        assert_eq!(cfg.payload_kind, PayloadKind::Mixed);

        let err =
            parse_args_with_env(base.into_iter().chain(["--kind", "replay"]), None).unwrap_err();
        assert!(err.contains("unsupported kind"));
    }

    #[test]
    fn reads_dsn_from_env_when_flag_omitted() {
        let cfg = parse_args_with_env(
            [
                "maxi-loop-loadgen",
                "--total",
                "500",
                "--compression",
                "br",
                "--include-unsupported-item",
            ],
            Some("http://envkey123@127.0.0.1:8080/7"),
        )
        .expect("parse ok")
        .expect("config present");

        assert_eq!(cfg.dsn.project_id, 7);
        assert_eq!(cfg.mode, LoadMode::TotalRequests(500));
        assert_eq!(cfg.compression, CompressionKind::Brotli);
        assert!(cfg.include_unsupported_item);
    }
}
