use std::fmt;
use std::net::{IpAddr, SocketAddr};

use crate::error::ConfigError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub from_address: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub use_tls: bool,
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_pw = self.password.as_ref().map(|_| "<redacted>");
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("from_address", &self.from_address)
            .field("username", &self.username)
            .field("password", &redacted_pw)
            .field("use_tls", &self.use_tls)
            .finish()
    }
}

#[derive(Clone)]
pub struct Config {
    pub app_host: IpAddr,
    pub app_port: u16,
    pub public_base_url: String,
    pub database_url: String,
    pub db_max_connections: u32,
    pub retention_default_days: u16,
    pub max_full_events_per_issue: u32,
    pub trace_slow_threshold_ms: u32,
    pub trace_random_sample_rate: f64,
    pub max_envelope_compressed_bytes: usize,
    pub max_envelope_decompressed_bytes: usize,
    pub max_event_payload_bytes: usize,
    pub ingest_channel_capacity: usize,
    pub log_format: LogFormat,
    pub single_tenant_mode: bool,
    pub cookie_secure: bool,
    pub session_ttl_hours: u32,
    pub default_tenant_slug: String,
    pub default_tenant_name: String,
    pub default_workspace_slug: String,
    pub default_workspace_name: String,
    pub bootstrap_admin_email: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    pub bootstrap_admin_name: String,
    pub smtp: Option<SmtpConfig>,
}

// Redact sensitive fields so credentials can never be logged via Debug.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_admin_pw = self.bootstrap_admin_password.as_ref().map(|_| "<redacted>");
        f.debug_struct("Config")
            .field("app_host", &self.app_host)
            .field("app_port", &self.app_port)
            .field("public_base_url", &self.public_base_url)
            .field("database_url", &"<redacted>")
            .field("db_max_connections", &self.db_max_connections)
            .field("retention_default_days", &self.retention_default_days)
            .field("max_full_events_per_issue", &self.max_full_events_per_issue)
            .field("trace_slow_threshold_ms", &self.trace_slow_threshold_ms)
            .field("trace_random_sample_rate", &self.trace_random_sample_rate)
            .field(
                "max_envelope_compressed_bytes",
                &self.max_envelope_compressed_bytes,
            )
            .field(
                "max_envelope_decompressed_bytes",
                &self.max_envelope_decompressed_bytes,
            )
            .field("max_event_payload_bytes", &self.max_event_payload_bytes)
            .field("ingest_channel_capacity", &self.ingest_channel_capacity)
            .field("log_format", &self.log_format)
            .field("single_tenant_mode", &self.single_tenant_mode)
            .field("cookie_secure", &self.cookie_secure)
            .field("session_ttl_hours", &self.session_ttl_hours)
            .field("default_tenant_slug", &self.default_tenant_slug)
            .field("default_tenant_name", &self.default_tenant_name)
            .field("default_workspace_slug", &self.default_workspace_slug)
            .field("default_workspace_name", &self.default_workspace_name)
            .field("bootstrap_admin_email", &self.bootstrap_admin_email)
            .field("bootstrap_admin_password", &redacted_admin_pw)
            .field("bootstrap_admin_name", &self.bootstrap_admin_name)
            .field("smtp", &self.smtp)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let host_raw = lookup("APP_HOST").unwrap_or_else(|| "127.0.0.1".to_owned());
        let app_host: IpAddr = host_raw
            .parse()
            .map_err(|_| ConfigError::InvalidValue("APP_HOST", "must be a valid IP address"))?;

        let app_port =
            parse_u16_range(lookup("APP_PORT").as_deref(), 8080, 1, u16::MAX, "APP_PORT")?;

        let default_base_url = format!("http://{app_host}:{app_port}");
        let public_base_url = parse_public_base_url(
            lookup("PUBLIC_BASE_URL").as_deref(),
            &default_base_url,
            "PUBLIC_BASE_URL",
        )?;

        let database_url = lookup("DATABASE_URL")
            .filter(|v| !v.trim().is_empty())
            .ok_or(ConfigError::Missing("DATABASE_URL"))?;
        if !(database_url.starts_with("postgres://") || database_url.starts_with("postgresql://")) {
            return Err(ConfigError::InvalidValue(
                "DATABASE_URL",
                "must start with postgres:// or postgresql://",
            ));
        }

        let db_max_connections = parse_u32_range(
            lookup("DB_MAX_CONNECTIONS").as_deref(),
            10,
            1,
            100,
            "DB_MAX_CONNECTIONS",
        )?;

        let retention_default_days = parse_u16_range(
            lookup("RETENTION_DEFAULT_DAYS").as_deref(),
            30,
            1,
            3650,
            "RETENTION_DEFAULT_DAYS",
        )?;

        let max_full_events_per_issue = parse_u32_range(
            lookup("MAX_FULL_EVENTS_PER_ISSUE").as_deref(),
            50,
            1,
            1000,
            "MAX_FULL_EVENTS_PER_ISSUE",
        )?;

        let trace_slow_threshold_ms = parse_u32_range(
            lookup("TRACE_SLOW_THRESHOLD_MS").as_deref(),
            500,
            1,
            600_000,
            "TRACE_SLOW_THRESHOLD_MS",
        )?;

        let trace_random_sample_rate = parse_f64_range(
            lookup("TRACE_RANDOM_SAMPLE_RATE").as_deref(),
            0.05,
            0.0,
            1.0,
            "TRACE_RANDOM_SAMPLE_RATE",
        )?;

        let max_envelope_compressed_bytes = parse_u32_range(
            lookup("MAX_ENVELOPE_COMPRESSED_BYTES").as_deref(),
            1_048_576,
            1_024,
            33_554_432,
            "MAX_ENVELOPE_COMPRESSED_BYTES",
        )? as usize;

        let max_envelope_decompressed_bytes = parse_u32_range(
            lookup("MAX_ENVELOPE_DECOMPRESSED_BYTES").as_deref(),
            2_097_152,
            1_024,
            67_108_864,
            "MAX_ENVELOPE_DECOMPRESSED_BYTES",
        )? as usize;

        let max_event_payload_bytes = parse_u32_range(
            lookup("MAX_EVENT_PAYLOAD_BYTES").as_deref(),
            524_288,
            1_024,
            16_777_216,
            "MAX_EVENT_PAYLOAD_BYTES",
        )? as usize;

        let ingest_channel_capacity = parse_u32_range(
            lookup("INGEST_CHANNEL_CAPACITY").as_deref(),
            1_024,
            1,
            100_000,
            "INGEST_CHANNEL_CAPACITY",
        )? as usize;

        let log_format = match lookup("LOG_FORMAT")
            .unwrap_or_else(|| "pretty".to_owned())
            .to_ascii_lowercase()
            .as_str()
        {
            "pretty" => LogFormat::Pretty,
            "json" => LogFormat::Json,
            _ => {
                return Err(ConfigError::InvalidValue(
                    "LOG_FORMAT",
                    "must be 'pretty' or 'json'",
                ));
            }
        };

        let single_tenant_mode = parse_bool(
            lookup("SINGLE_TENANT_MODE").as_deref(),
            true,
            "SINGLE_TENANT_MODE",
        )?;

        let cookie_secure = parse_bool(
            lookup("COOKIE_SECURE").as_deref(),
            public_base_url.starts_with("https://"),
            "COOKIE_SECURE",
        )?;

        let session_ttl_hours = parse_u32_range(
            lookup("SESSION_TTL_HOURS").as_deref(),
            168,
            1,
            720,
            "SESSION_TTL_HOURS",
        )?;

        let default_tenant_slug = parse_slug(
            lookup("DEFAULT_TENANT_SLUG").as_deref(),
            "default",
            "DEFAULT_TENANT_SLUG",
        )?;
        let default_tenant_name = parse_display_name(
            lookup("DEFAULT_TENANT_NAME").as_deref(),
            "Default Organization",
            "DEFAULT_TENANT_NAME",
        )?;
        let default_workspace_slug = parse_slug(
            lookup("DEFAULT_WORKSPACE_SLUG").as_deref(),
            "default",
            "DEFAULT_WORKSPACE_SLUG",
        )?;
        let default_workspace_name = parse_display_name(
            lookup("DEFAULT_WORKSPACE_NAME").as_deref(),
            "Default Workspace",
            "DEFAULT_WORKSPACE_NAME",
        )?;

        let bootstrap_admin_email = match lookup("BOOTSTRAP_ADMIN_EMAIL")
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
        {
            Some(email) => {
                if !is_valid_email(&email) {
                    return Err(ConfigError::InvalidValue(
                        "BOOTSTRAP_ADMIN_EMAIL",
                        "must be a valid email address",
                    ));
                }
                Some(email)
            }
            None => None,
        };

        let bootstrap_admin_password =
            match lookup("BOOTSTRAP_ADMIN_PASSWORD").filter(|v| !v.is_empty()) {
                Some(pw) => {
                    if pw.len() < 12 || pw.len() > 256 {
                        return Err(ConfigError::InvalidValue(
                            "BOOTSTRAP_ADMIN_PASSWORD",
                            "must be between 12 and 256 characters",
                        ));
                    }
                    Some(pw)
                }
                None => None,
            };

        if bootstrap_admin_email.is_some() != bootstrap_admin_password.is_some() {
            return Err(ConfigError::InvalidValue(
                "BOOTSTRAP_ADMIN_EMAIL",
                "BOOTSTRAP_ADMIN_EMAIL and BOOTSTRAP_ADMIN_PASSWORD must be provided together",
            ));
        }

        let bootstrap_admin_name = parse_display_name(
            lookup("BOOTSTRAP_ADMIN_NAME").as_deref(),
            "Administrator",
            "BOOTSTRAP_ADMIN_NAME",
        )?;

        let smtp = parse_smtp_config(&lookup)?;

        Ok(Self {
            app_host,
            app_port,
            public_base_url,
            database_url,
            db_max_connections,
            retention_default_days,
            max_full_events_per_issue,
            trace_slow_threshold_ms,
            trace_random_sample_rate,
            max_envelope_compressed_bytes,
            max_envelope_decompressed_bytes,
            max_event_payload_bytes,
            ingest_channel_capacity,
            log_format,
            single_tenant_mode,
            cookie_secure,
            session_ttl_hours,
            default_tenant_slug,
            default_tenant_name,
            default_workspace_slug,
            default_workspace_name,
            bootstrap_admin_email,
            bootstrap_admin_password,
            bootstrap_admin_name,
            smtp,
        })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, ConfigError> {
        Ok(SocketAddr::new(self.app_host, self.app_port))
    }
}

fn parse_smtp_config<F>(lookup: &F) -> Result<Option<SmtpConfig>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(host) = lookup("SMTP_HOST")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };

    if host.len() > 253 || host.chars().any(char::is_whitespace) {
        return Err(ConfigError::InvalidValue(
            "SMTP_HOST",
            "must be a valid hostname",
        ));
    }

    let port = parse_u16_range(
        lookup("SMTP_PORT").as_deref(),
        587,
        1,
        u16::MAX,
        "SMTP_PORT",
    )?;
    let from_address = lookup("SMTP_FROM")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or(ConfigError::InvalidValue(
            "SMTP_FROM",
            "must be set when SMTP_HOST is configured",
        ))?;
    if !is_valid_email(&from_address) {
        return Err(ConfigError::InvalidValue(
            "SMTP_FROM",
            "must be a valid email address",
        ));
    }

    let username = lookup("SMTP_USERNAME")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let password = lookup("SMTP_PASSWORD").filter(|s| !s.is_empty());
    let use_tls = parse_bool(lookup("SMTP_USE_TLS").as_deref(), true, "SMTP_USE_TLS")?;

    Ok(Some(SmtpConfig {
        host,
        port,
        from_address,
        username,
        password,
        use_tls,
    }))
}

fn parse_public_base_url(
    raw: Option<&str>,
    default: &str,
    name: &'static str,
) -> Result<String, ConfigError> {
    let val = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => v.trim_end_matches('/'),
        None => default.trim_end_matches('/'),
    };
    if !(val.starts_with("http://") || val.starts_with("https://"))
        || val.len() > 256
        || val.chars().any(char::is_whitespace)
    {
        return Err(ConfigError::InvalidValue(
            name,
            "must be a valid http:// or https:// base URL",
        ));
    }
    let after_scheme = val
        .strip_prefix("https://")
        .or_else(|| val.strip_prefix("http://"))
        .unwrap_or("");
    if after_scheme.is_empty() || after_scheme.starts_with('/') || after_scheme.contains('@') {
        return Err(ConfigError::InvalidValue(
            name,
            "must include a valid host without embedded credentials",
        ));
    }
    Ok(val.to_owned())
}

fn parse_bool(raw: Option<&str>, default: bool, name: &'static str) -> Result<bool, ConfigError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(default),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err(ConfigError::InvalidValue(
                name,
                "must be a boolean ('true' or 'false')",
            )),
        },
    }
}

pub fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 64 {
        return false;
    }
    slug.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

pub fn is_valid_email(email: &str) -> bool {
    if email.len() < 3 || email.len() > 254 || email.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn parse_slug(raw: Option<&str>, default: &str, name: &'static str) -> Result<String, ConfigError> {
    let val = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => v,
        None => default,
    };
    if !is_valid_slug(val) {
        return Err(ConfigError::InvalidValue(
            name,
            "must be lowercase alphanumeric hyphen-separated slug (max 64 chars)",
        ));
    }
    Ok(val.to_owned())
}

fn parse_display_name(
    raw: Option<&str>,
    default: &str,
    name: &'static str,
) -> Result<String, ConfigError> {
    let val = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => v,
        None => default,
    };
    if val.is_empty() || val.len() > 128 {
        return Err(ConfigError::InvalidValue(
            name,
            "must be between 1 and 128 characters",
        ));
    }
    Ok(val.to_owned())
}

fn parse_u16_range(
    raw: Option<&str>,
    default: u16,
    min: u16,
    max: u16,
    name: &'static str,
) -> Result<u16, ConfigError> {
    let value = match raw {
        None | Some("") => default,
        Some(s) => s
            .parse::<u16>()
            .map_err(|_| ConfigError::InvalidValue(name, "must be a valid integer"))?,
    };
    if value < min || value > max {
        return Err(ConfigError::InvalidValue(
            name,
            "value out of allowed range",
        ));
    }
    Ok(value)
}

fn parse_u32_range(
    raw: Option<&str>,
    default: u32,
    min: u32,
    max: u32,
    name: &'static str,
) -> Result<u32, ConfigError> {
    let value = match raw {
        None | Some("") => default,
        Some(s) => s
            .parse::<u32>()
            .map_err(|_| ConfigError::InvalidValue(name, "must be a valid integer"))?,
    };
    if value < min || value > max {
        return Err(ConfigError::InvalidValue(
            name,
            "value out of allowed range",
        ));
    }
    Ok(value)
}

fn parse_f64_range(
    raw: Option<&str>,
    default: f64,
    min: f64,
    max: f64,
    name: &'static str,
) -> Result<f64, ConfigError> {
    let value = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => default,
        Some(s) => s
            .parse::<f64>()
            .map_err(|_| ConfigError::InvalidValue(name, "must be a valid number"))?,
    };
    if !value.is_finite() || value < min || value > max {
        return Err(ConfigError::InvalidValue(
            name,
            "value out of allowed range",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn map_lookup(entries: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn loads_defaults_when_only_database_url_set() {
        let cfg = Config::from_lookup(map_lookup(&[(
            "DATABASE_URL",
            "postgres://user:pass@127.0.0.1:55432/maxi_loop",
        )]))
        .expect("config should load");

        assert_eq!(cfg.app_host.to_string(), "127.0.0.1");
        assert_eq!(cfg.app_port, 8080);
        assert_eq!(cfg.db_max_connections, 10);
        assert_eq!(cfg.retention_default_days, 30);
        assert_eq!(cfg.max_full_events_per_issue, 50);
        assert_eq!(cfg.log_format, LogFormat::Pretty);
        assert_eq!(cfg.default_tenant_slug, "default");
        assert_eq!(cfg.default_workspace_slug, "default");
        assert_eq!(cfg.bootstrap_admin_email, None);
        assert_eq!(cfg.bootstrap_admin_password, None);
    }

    #[test]
    fn fails_when_database_url_missing() {
        let err = Config::from_lookup(map_lookup(&[])).unwrap_err();
        assert_eq!(err, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn fails_on_non_postgres_scheme() {
        let err = Config::from_lookup(map_lookup(&[(
            "DATABASE_URL",
            "mysql://user:pass@127.0.0.1/db",
        )]))
        .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue("DATABASE_URL", _)));
    }

    #[test]
    fn fails_on_out_of_range_port_and_retention() {
        let err = Config::from_lookup(map_lookup(&[
            ("DATABASE_URL", "postgres://user:pass@127.0.0.1/db"),
            ("APP_PORT", "0"),
        ]))
        .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue("APP_PORT", _)));

        let err = Config::from_lookup(map_lookup(&[
            ("DATABASE_URL", "postgres://user:pass@127.0.0.1/db"),
            ("RETENTION_DEFAULT_DAYS", "0"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidValue("RETENTION_DEFAULT_DAYS", _)
        ));
    }

    #[test]
    fn validates_bootstrap_admin_credentials_and_redacts_secrets_in_debug() {
        let err = Config::from_lookup(map_lookup(&[
            ("DATABASE_URL", "postgres://user:pass@127.0.0.1/db"),
            ("BOOTSTRAP_ADMIN_EMAIL", "admin@example.com"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidValue("BOOTSTRAP_ADMIN_EMAIL", _)
        ));

        let err = Config::from_lookup(map_lookup(&[
            ("DATABASE_URL", "postgres://user:pass@127.0.0.1/db"),
            ("BOOTSTRAP_ADMIN_EMAIL", "admin@example.com"),
            ("BOOTSTRAP_ADMIN_PASSWORD", "short"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidValue("BOOTSTRAP_ADMIN_PASSWORD", _)
        ));

        let cfg = Config::from_lookup(map_lookup(&[
            (
                "DATABASE_URL",
                "postgres://user:super_secret_db_pw@127.0.0.1:55432/maxi_loop",
            ),
            ("BOOTSTRAP_ADMIN_EMAIL", "Admin@Example.com"),
            ("BOOTSTRAP_ADMIN_PASSWORD", "admin_secret_password_123"),
        ]))
        .expect("valid bootstrap config");

        assert_eq!(
            cfg.bootstrap_admin_email.as_deref(),
            Some("admin@example.com")
        );
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("super_secret_db_pw"));
        assert!(!rendered.contains("admin_secret_password_123"));
        assert!(rendered.contains("<redacted>"));
    }
}
