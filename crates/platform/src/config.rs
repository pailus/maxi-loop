use std::fmt;
use std::net::{IpAddr, SocketAddr};

use crate::error::ConfigError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Clone)]
pub struct Config {
    pub app_host: IpAddr,
    pub app_port: u16,
    pub database_url: String,
    pub db_max_connections: u32,
    pub retention_default_days: u16,
    pub max_full_events_per_issue: u32,
    pub log_format: LogFormat,
    pub default_tenant_slug: String,
    pub default_tenant_name: String,
    pub default_workspace_slug: String,
    pub default_workspace_name: String,
    pub bootstrap_admin_email: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    pub bootstrap_admin_name: String,
}

// Redact sensitive fields so credentials can never be logged via Debug.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_admin_pw = self.bootstrap_admin_password.as_ref().map(|_| "<redacted>");
        f.debug_struct("Config")
            .field("app_host", &self.app_host)
            .field("app_port", &self.app_port)
            .field("database_url", &"<redacted>")
            .field("db_max_connections", &self.db_max_connections)
            .field("retention_default_days", &self.retention_default_days)
            .field("max_full_events_per_issue", &self.max_full_events_per_issue)
            .field("log_format", &self.log_format)
            .field("default_tenant_slug", &self.default_tenant_slug)
            .field("default_tenant_name", &self.default_tenant_name)
            .field("default_workspace_slug", &self.default_workspace_slug)
            .field("default_workspace_name", &self.default_workspace_name)
            .field("bootstrap_admin_email", &self.bootstrap_admin_email)
            .field("bootstrap_admin_password", &redacted_admin_pw)
            .field("bootstrap_admin_name", &self.bootstrap_admin_name)
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

        Ok(Self {
            app_host,
            app_port,
            database_url,
            db_max_connections,
            retention_default_days,
            max_full_events_per_issue,
            log_format,
            default_tenant_slug,
            default_tenant_name,
            default_workspace_slug,
            default_workspace_name,
            bootstrap_admin_email,
            bootstrap_admin_password,
            bootstrap_admin_name,
        })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, ConfigError> {
        Ok(SocketAddr::new(self.app_host, self.app_port))
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
