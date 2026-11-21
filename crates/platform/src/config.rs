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
}

// Redact database_url so credentials can never be logged via Debug.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("app_host", &self.app_host)
            .field("app_port", &self.app_port)
            .field("database_url", &"<redacted>")
            .field("db_max_connections", &self.db_max_connections)
            .field("retention_default_days", &self.retention_default_days)
            .field("max_full_events_per_issue", &self.max_full_events_per_issue)
            .field("log_format", &self.log_format)
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

        Ok(Self {
            app_host,
            app_port,
            database_url,
            db_max_connections,
            retention_default_days,
            max_full_events_per_issue,
            log_format,
        })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, ConfigError> {
        Ok(SocketAddr::new(self.app_host, self.app_port))
    }
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
    fn debug_output_redacts_database_url() {
        let cfg = Config::from_lookup(map_lookup(&[(
            "DATABASE_URL",
            "postgres://user:super_secret_pw@127.0.0.1:55432/maxi_loop",
        )]))
        .expect("config should load");

        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("super_secret_pw"));
        assert!(rendered.contains("<redacted>"));
    }
}
