use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec};
use maxi_loop_platform::config::Config;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Serve,
    Bootstrap(BootstrapOverrides),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BootstrapOverrides {
    pub tenant_slug: Option<String>,
    pub tenant_name: Option<String>,
    pub workspace_slug: Option<String>,
    pub workspace_name: Option<String>,
    pub admin_email: Option<String>,
    pub admin_password: Option<String>,
    pub admin_name: Option<String>,
}

impl BootstrapOverrides {
    pub fn apply_to(&self, config: &Config) -> Result<BootstrapSpec, String> {
        let mut spec = BootstrapSpec::from_config(config);
        if let Some(slug) = &self.tenant_slug {
            spec.tenant_slug = slug.clone();
        }
        if let Some(name) = &self.tenant_name {
            spec.tenant_name = name.clone();
        }
        if let Some(slug) = &self.workspace_slug {
            spec.workspace_slug = slug.clone();
        }
        if let Some(name) = &self.workspace_name {
            spec.workspace_name = name.clone();
        }

        let email = self
            .admin_email
            .clone()
            .or_else(|| config.bootstrap_admin_email.clone());
        let password = self
            .admin_password
            .clone()
            .or_else(|| config.bootstrap_admin_password.clone());
        let name = self
            .admin_name
            .clone()
            .unwrap_or_else(|| config.bootstrap_admin_name.clone());

        match (email, password) {
            (Some(email), Some(password)) => {
                spec.admin_user = Some(AdminUserSpec {
                    email: email.trim().to_ascii_lowercase(),
                    password,
                    name,
                });
            }
            (None, None) => {}
            _ => {
                return Err(
                    "both --admin-email and --admin-password (or BOOTSTRAP_ADMIN_EMAIL and BOOTSTRAP_ADMIN_PASSWORD) must be provided together".to_owned(),
                );
            }
        }

        spec.validate().map_err(|err| err.to_string())?;
        Ok(spec)
    }
}

pub fn parse_args<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut iter = args.into_iter().map(|s| s.as_ref().to_owned());
    let _bin = iter.next();

    let Some(subcmd) = iter.next() else {
        return Ok(Command::Serve);
    };

    match subcmd.as_str() {
        "serve" => {
            if let Some(extra) = iter.next() {
                return Err(format!("unexpected argument for 'serve': {extra}"));
            }
            Ok(Command::Serve)
        }
        "bootstrap" => {
            let mut overrides = BootstrapOverrides::default();
            while let Some(flag) = iter.next() {
                let take_val = |iter: &mut dyn Iterator<Item = String>, flag_name: &str| {
                    iter.next()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| format!("missing value for {flag_name}"))
                };
                match flag.as_str() {
                    "--tenant-slug" => {
                        overrides.tenant_slug = Some(take_val(&mut iter, "--tenant-slug")?);
                    }
                    "--tenant-name" => {
                        overrides.tenant_name = Some(take_val(&mut iter, "--tenant-name")?);
                    }
                    "--workspace-slug" => {
                        overrides.workspace_slug = Some(take_val(&mut iter, "--workspace-slug")?);
                    }
                    "--workspace-name" => {
                        overrides.workspace_name = Some(take_val(&mut iter, "--workspace-name")?);
                    }
                    "--admin-email" => {
                        overrides.admin_email = Some(take_val(&mut iter, "--admin-email")?);
                    }
                    "--admin-password" => {
                        overrides.admin_password = Some(take_val(&mut iter, "--admin-password")?);
                    }
                    "--admin-name" => {
                        overrides.admin_name = Some(take_val(&mut iter, "--admin-name")?);
                    }
                    _ => return Err(format!("unknown bootstrap flag: {flag}")),
                }
            }
            Ok(Command::Bootstrap(overrides))
        }
        other => Err(format!(
            "unknown subcommand '{other}' (expected 'serve' or 'bootstrap')"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_serve_when_no_subcommand() {
        assert_eq!(parse_args(["maxi-loop"]).unwrap(), Command::Serve);
        assert_eq!(parse_args(["maxi-loop", "serve"]).unwrap(), Command::Serve);
    }

    #[test]
    fn parses_bootstrap_flags() {
        let cmd = parse_args([
            "maxi-loop",
            "bootstrap",
            "--tenant-slug",
            "acme",
            "--workspace-slug",
            "core",
            "--admin-email",
            "owner@acme.example",
            "--admin-password",
            "very_secure_password_123",
        ])
        .expect("valid bootstrap args");

        assert_eq!(
            cmd,
            Command::Bootstrap(BootstrapOverrides {
                tenant_slug: Some("acme".to_owned()),
                tenant_name: None,
                workspace_slug: Some("core".to_owned()),
                workspace_name: None,
                admin_email: Some("owner@acme.example".to_owned()),
                admin_password: Some("very_secure_password_123".to_owned()),
                admin_name: None,
            })
        );
    }

    #[test]
    fn rejects_unknown_subcommand_or_missing_flag_value() {
        assert!(parse_args(["maxi-loop", "unknown"]).is_err());
        assert!(parse_args(["maxi-loop", "bootstrap", "--admin-email"]).is_err());
    }
}
