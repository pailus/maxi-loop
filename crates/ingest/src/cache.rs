use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use maxi_loop_platform::dsn_notify::{ProjectDsnInfo, ProjectDsnObserver};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::begin_worker_tx;
use sqlx::PgPool;
use subtle::ConstantTimeEq;

#[derive(Clone, Debug)]
pub struct CachedProject {
    pub project_id: i64,
    pub tenant_id: i64,
    pub workspace_id: i64,
    pub dsn_public_key: String,
    pub rate_limit_per_minute: u32,
    limiter: Arc<DefaultDirectRateLimiter>,
}

impl CachedProject {
    fn from_info(
        info: ProjectDsnInfo,
        existing_limiter: Option<Arc<DefaultDirectRateLimiter>>,
    ) -> Self {
        let rpm = info.rate_limit_per_minute.max(1);
        let limiter = existing_limiter.unwrap_or_else(|| Arc::new(build_rate_limiter(rpm)));
        Self {
            project_id: info.project_id,
            tenant_id: info.tenant_id,
            workspace_id: info.workspace_id,
            dsn_public_key: info.dsn_public_key,
            rate_limit_per_minute: rpm,
            limiter,
        }
    }
}

fn build_rate_limiter(rate_limit_per_minute: u32) -> DefaultDirectRateLimiter {
    let nz = NonZeroU32::new(rate_limit_per_minute.max(1)).expect("rate_limit_per_minute >= 1");
    RateLimiter::direct(Quota::per_minute(nz))
}

/// Thread-safe in-memory DSN cache and per-project `governor` rate limiter.
/// Authenticates envelopes in O(1) without querying PostgreSQL on the ingest path.
#[derive(Clone, Debug, Default)]
pub struct DsnCache {
    projects: Arc<RwLock<HashMap<i64, CachedProject>>>,
    db_reload_count: Arc<AtomicU64>,
    cache_auth_count: Arc<AtomicU64>,
}

impl DsnCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Preload or refresh all projects from PostgreSQL into the in-memory cache.
    pub async fn reload_from_db(&self, pool: &PgPool) -> Result<usize, AppError> {
        let mut tx = begin_worker_tx(pool).await?;
        let rows: Vec<(i64, i64, i64, String, i32)> = sqlx::query_as(
            "SELECT id, tenant_id, workspace_id, dsn_public_key, rate_limit_per_minute
             FROM projects",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("reload dsn cache from db: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("commit reload dsn cache: {err}")))?;

        self.db_reload_count.fetch_add(1, Ordering::Relaxed);

        let mut guard = match self.projects.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        let mut next_map = HashMap::with_capacity(rows.len());
        for (project_id, tenant_id, workspace_id, dsn_public_key, rpm_i32) in rows {
            let rpm = u32::try_from(rpm_i32.max(1)).unwrap_or(1200);
            let existing_limiter = guard
                .get(&project_id)
                .filter(|prev| prev.rate_limit_per_minute == rpm)
                .map(|prev| Arc::clone(&prev.limiter));

            let entry = CachedProject::from_info(
                ProjectDsnInfo {
                    project_id,
                    tenant_id,
                    workspace_id,
                    dsn_public_key,
                    rate_limit_per_minute: rpm,
                },
                existing_limiter,
            );
            next_map.insert(project_id, entry);
        }

        let count = next_map.len();
        *guard = next_map;
        Ok(count)
    }

    /// Upsert a single project entry in memory (called when a project is created or its DSN is rotated).
    pub fn upsert(&self, info: ProjectDsnInfo) {
        let mut guard = match self.projects.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        let rpm = info.rate_limit_per_minute.max(1);
        let existing_limiter = guard
            .get(&info.project_id)
            .filter(|prev| prev.rate_limit_per_minute == rpm)
            .map(|prev| Arc::clone(&prev.limiter));

        let entry = CachedProject::from_info(info, existing_limiter);
        guard.insert(entry.project_id, entry);
    }

    /// Authenticate `(project_id, provided_key)` strictly from the in-memory cache with constant-time key comparison.
    /// Never queries the database.
    pub fn authenticate(
        &self,
        project_id: i64,
        provided_key: &str,
    ) -> Result<CachedProject, AppError> {
        self.cache_auth_count.fetch_add(1, Ordering::Relaxed);

        let trimmed = provided_key.trim();
        if project_id <= 0 || trimmed.is_empty() {
            return Err(AppError::Unauthorized("invalid dsn credentials"));
        }

        let guard = match self.projects.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        let Some(project) = guard.get(&project_id) else {
            return Err(AppError::Unauthorized("invalid dsn credentials"));
        };

        let expected = project.dsn_public_key.as_bytes();
        let candidate = trimmed.as_bytes();
        if expected.len() != candidate.len() || !bool::from(expected.ct_eq(candidate)) {
            return Err(AppError::Unauthorized("invalid dsn credentials"));
        }

        Ok(project.clone())
    }

    /// Check the per-project `governor` token-bucket rate limiter for `item_count` events.
    /// Returns `Err(retry_after_secs)` if the project's quota is exhausted.
    pub fn check_rate_limit(&self, project: &CachedProject, item_count: u32) -> Result<(), u64> {
        for _ in 0..item_count.max(1) {
            if let Err(not_until) = project.limiter.check() {
                let now = DefaultClock::default().now();
                let secs = not_until.wait_time_from(now).as_secs().max(1);
                return Err(secs);
            }
        }
        Ok(())
    }

    pub fn db_reload_count(&self) -> u64 {
        self.db_reload_count.load(Ordering::Relaxed)
    }

    pub fn cache_auth_count(&self) -> u64 {
        self.cache_auth_count.load(Ordering::Relaxed)
    }
}

impl ProjectDsnObserver for DsnCache {
    fn on_project_dsn_upserted(&self, info: ProjectDsnInfo) {
        self.upsert(info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticates_from_memory_and_enforces_per_project_governor_quota() {
        let cache = DsnCache::new();
        cache.upsert(ProjectDsnInfo {
            project_id: 10,
            tenant_id: 1,
            workspace_id: 2,
            dsn_public_key: "0123456789abcdef0123456789abcdef".to_owned(),
            rate_limit_per_minute: 2,
        });

        assert_eq!(cache.db_reload_count(), 0);
        assert!(
            cache
                .authenticate(999, "0123456789abcdef0123456789abcdef")
                .is_err()
        );
        assert!(cache.authenticate(10, "wrong_key").is_err());

        let proj = cache
            .authenticate(10, "0123456789abcdef0123456789abcdef")
            .expect("valid cached dsn");
        assert_eq!(proj.tenant_id, 1);
        assert_eq!(proj.workspace_id, 2);
        assert_eq!(cache.cache_auth_count(), 3);
        assert_eq!(cache.db_reload_count(), 0);

        // Quota is 2 events/minute: first 2 succeed, 3rd returns Err(retry_after_secs >= 1).
        assert_eq!(cache.check_rate_limit(&proj, 1), Ok(()));
        assert_eq!(cache.check_rate_limit(&proj, 1), Ok(()));
        let retry_after = cache.check_rate_limit(&proj, 1).unwrap_err();
        assert!(retry_after >= 1);
    }
}
