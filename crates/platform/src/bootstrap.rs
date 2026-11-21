use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use password_hash::SaltString;
use password_hash::rand_core::OsRng;
use sqlx::PgPool;

use crate::config::{Config, is_valid_email, is_valid_slug};
use crate::error::AppError;
use crate::tenant::begin_worker_tx;

#[derive(Clone, Debug)]
pub struct AdminUserSpec {
    pub email: String,
    pub password: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct BootstrapSpec {
    pub tenant_slug: String,
    pub tenant_name: String,
    pub retention_days: u16,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub admin_user: Option<AdminUserSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapOutcome {
    pub tenant_id: i64,
    pub tenant_slug: String,
    pub workspace_id: i64,
    pub workspace_slug: String,
    pub admin_user_id: Option<i64>,
    pub admin_email: Option<String>,
}

impl BootstrapSpec {
    pub fn from_config(config: &Config) -> Self {
        let admin_user = match (
            config.bootstrap_admin_email.as_ref(),
            config.bootstrap_admin_password.as_ref(),
        ) {
            (Some(email), Some(password)) => Some(AdminUserSpec {
                email: email.clone(),
                password: password.clone(),
                name: config.bootstrap_admin_name.clone(),
            }),
            _ => None,
        };

        Self {
            tenant_slug: config.default_tenant_slug.clone(),
            tenant_name: config.default_tenant_name.clone(),
            retention_days: config.retention_default_days,
            workspace_slug: config.default_workspace_slug.clone(),
            workspace_name: config.default_workspace_name.clone(),
            admin_user,
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if !is_valid_slug(&self.tenant_slug) {
            return Err(AppError::Validation(
                "tenant_slug must be a lowercase alphanumeric hyphen-separated slug".to_owned(),
            ));
        }
        if self.tenant_name.trim().is_empty() || self.tenant_name.len() > 128 {
            return Err(AppError::Validation(
                "tenant_name must be between 1 and 128 characters".to_owned(),
            ));
        }
        if self.retention_days < 1 || self.retention_days > 3650 {
            return Err(AppError::Validation(
                "retention_days must be between 1 and 3650".to_owned(),
            ));
        }
        if !is_valid_slug(&self.workspace_slug) {
            return Err(AppError::Validation(
                "workspace_slug must be a lowercase alphanumeric hyphen-separated slug".to_owned(),
            ));
        }
        if self.workspace_name.trim().is_empty() || self.workspace_name.len() > 128 {
            return Err(AppError::Validation(
                "workspace_name must be between 1 and 128 characters".to_owned(),
            ));
        }
        if let Some(admin) = &self.admin_user {
            if !is_valid_email(&admin.email) {
                return Err(AppError::Validation(
                    "admin email must be a valid email address".to_owned(),
                ));
            }
            if admin.password.len() < 12 || admin.password.len() > 256 {
                return Err(AppError::Validation(
                    "admin password must be between 12 and 256 characters".to_owned(),
                ));
            }
            if admin.name.trim().is_empty() || admin.name.len() > 128 {
                return Err(AppError::Validation(
                    "admin name must be between 1 and 128 characters".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

/// Hash a password with Argon2id using OWASP-recommended parameters and a CSPRNG salt.
pub fn hash_password_argon2id(password: &str) -> Result<String, AppError> {
    if password.len() < 12 || password.len() > 256 {
        return Err(AppError::Validation(
            "password must be between 12 and 256 characters".to_owned(),
        ));
    }
    let salt = SaltString::generate(&mut OsRng);
    let params = Params::new(19_456, 2, 1, None)
        .map_err(|err| AppError::Crypto(format!("invalid argon2id params: {err}")))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::Crypto(format!("failed to hash password: {err}")))
}

/// Verify a plaintext password against a stored Argon2id PHC string.
pub fn verify_password_argon2id(password: &str, password_hash: &str) -> Result<bool, AppError> {
    let parsed_hash = PasswordHash::new(password_hash)
        .map_err(|err| AppError::Crypto(format!("invalid password hash format: {err}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

/// Idempotently ensure the default tenant, default workspace, and optional initial admin user exist.
pub async fn bootstrap_defaults(
    pool: &PgPool,
    spec: &BootstrapSpec,
) -> Result<BootstrapOutcome, AppError> {
    spec.validate()?;

    let prehashed_admin = match &spec.admin_user {
        Some(admin) => {
            let normalized_email = admin.email.trim().to_ascii_lowercase();
            let password_hash = hash_password_argon2id(&admin.password)?;
            Some((
                normalized_email,
                admin.name.trim().to_owned(),
                password_hash,
            ))
        }
        None => None,
    };

    let mut tx = begin_worker_tx(pool).await?;

    // 1. Ensure target tenant exists by slug (marking it as the installation default if no default tenant exists yet).
    let existing_tenant: Option<(i64, String)> =
        sqlx::query_as("SELECT id, slug FROM tenants WHERE slug = $1 LIMIT 1")
            .bind(&spec.tenant_slug)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("lookup tenant by slug: {err}")))?;

    let (tenant_id, tenant_slug) = match existing_tenant {
        Some((id, slug)) => (id, slug),
        None => {
            let has_default: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenants WHERE is_default = TRUE)")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|err| {
                        AppError::DatabaseQuery(format!("check default tenant: {err}"))
                    })?;

            let retention_i16 = i16::try_from(spec.retention_days)
                .map_err(|_| AppError::Validation("retention_days out of range".to_owned()))?;
            let row: (i64, String) = sqlx::query_as(
                "INSERT INTO tenants (slug, name, retention_days, is_default)
                 VALUES ($1, $2, $3, $4)
                 RETURNING id, slug",
            )
            .bind(&spec.tenant_slug)
            .bind(spec.tenant_name.trim())
            .bind(retention_i16)
            .bind(!has_default)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("insert tenant: {err}")))?;
            row
        }
    };

    // 2. Ensure default workspace exists inside the tenant (idempotent).
    let existing_default_workspace: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, slug FROM workspaces
         WHERE tenant_id = $1 AND (is_default = TRUE OR slug = $2)
         ORDER BY is_default DESC LIMIT 1",
    )
    .bind(tenant_id)
    .bind(&spec.workspace_slug)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("lookup default workspace: {err}")))?;

    let (workspace_id, workspace_slug) = match existing_default_workspace {
        Some((id, slug)) => (id, slug),
        None => {
            let row: (i64, String) = sqlx::query_as(
                "INSERT INTO workspaces (tenant_id, slug, name, is_default)
                 VALUES ($1, $2, $3, TRUE)
                 RETURNING id, slug",
            )
            .bind(tenant_id)
            .bind(&spec.workspace_slug)
            .bind(spec.workspace_name.trim())
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("insert default workspace: {err}")))?;
            row
        }
    };

    // 3. Optionally ensure the initial admin user and `tenant_owner` membership exist.
    let (admin_user_id, admin_email) = if let Some((email, name, password_hash)) = prehashed_admin {
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email, name, password_hash, is_active)
             VALUES ($1, $2, $3, TRUE)
             ON CONFLICT (email) DO UPDATE
                 SET name = EXCLUDED.name,
                     password_hash = EXCLUDED.password_hash,
                     is_active = TRUE,
                     updated_at = NOW()
             RETURNING id",
        )
        .bind(&email)
        .bind(&name)
        .bind(&password_hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("upsert bootstrap admin user: {err}")))?;

        sqlx::query(
            "INSERT INTO memberships (tenant_id, workspace_id, user_id, role)
             VALUES ($1, NULL, $2, 'tenant_owner')
             ON CONFLICT (tenant_id, user_id) WHERE workspace_id IS NULL
             DO UPDATE SET role = 'tenant_owner'",
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("ensure admin tenant_owner membership: {err}"))
        })?;

        (Some(user_id), Some(email))
    } else {
        (None, None)
    };

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit bootstrap transaction: {err}")))?;

    Ok(BootstrapOutcome {
        tenant_id,
        tenant_slug,
        workspace_id,
        workspace_slug,
        admin_user_id,
        admin_email,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_verifies_argon2id_password() {
        let hash =
            hash_password_argon2id("correct-horse-battery-staple").expect("hash should succeed");
        assert!(hash.starts_with("$argon2id$"));
        assert!(
            verify_password_argon2id("correct-horse-battery-staple", &hash)
                .expect("verify should succeed")
        );
        assert!(
            !verify_password_argon2id("wrong-password-attempt", &hash)
                .expect("verify should succeed")
        );
    }

    #[test]
    fn rejects_short_password() {
        let err = hash_password_argon2id("too-short").unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }
}
