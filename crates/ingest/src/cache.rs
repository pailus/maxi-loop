use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use maxi_loop_platform::dsn_notify::{ProjectDsnInfo, ProjectDsnObserver, TenantRateLimitUpdate};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::begin_worker_tx;
use sqlx::PgPool;
use subtle::ConstantTimeEq;

const DEFAULT_TENANT_AGGREGATE_RPM: u32 = 6000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitScope {
    Project,
    Tenant,
}

impl RateLimitScope {
    /// Sentry rate-limit scope identifier (`project` or `organization` for tenant-wide cap).
    pub fn sentry_scope(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Tenant => "organization",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitHit {
    pub scope: RateLimitScope,
    pub retry_after_secs: u64,
}

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

#[derive(Clone, Debug)]
struct CachedTenantLimiter {
    aggregate_rate_limit_per_minute: u32,
    limiter: Arc<DefaultDirectRateLimiter>,
}

impl CachedTenantLimiter {
    fn new(aggregate_rpm: u32, existing: Option<&Self>) -> Self {
        let rpm = aggregate_rpm.max(1);
        let limiter = existing
            .filter(|prev| prev.aggregate_rate_limit_per_minute == rpm)
            .map(|prev| Arc::clone(&prev.limiter))
            .unwrap_or_else(|| Arc::new(build_rate_limiter(rpm)));
        Self {
            aggregate_rate_limit_per_minute: rpm,
            limiter,
        }
    }
}

fn build_rate_limiter(rate_limit_per_minute: u32) -> DefaultDirectRateLimiter {
    let nz = NonZeroU32::new(rate_limit_per_minute.max(1)).expect("rate_limit_per_minute >= 1");
    RateLimiter::direct(Quota::per_minute(nz))
}

/// Thread-safe in-memory DSN cache with BOTH per-project and aggregate per-tenant `governor` rate limiters.
/// Authenticates envelopes and enforces limits in O(1) without querying PostgreSQL on the ingest path.
#[derive(Clone, Debug, Default)]
pub struct DsnCache {
    projects: Arc<RwLock<HashMap<i64, CachedProject>>>,
    tenants: Arc<RwLock<HashMap<i64, CachedTenantLimiter>>>,
    db_reload_count: Arc<AtomicU64>,
    cache_auth_count: Arc<AtomicU64>,
}

impl DsnCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Preload or refresh all tenants (aggregate caps) and projects (effective inherited limits)
    /// from PostgreSQL into the in-memory cache.
    pub async fn reload_from_db(&self, pool: &PgPool) -> Result<usize, AppError> {
        let mut tx = begin_worker_tx(pool).await?;

        let tenant_rows: Vec<(i64, i32)> = sqlx::query_as(
            "SELECT id, aggregate_rate_limit_per_minute
             FROM tenants",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("reload tenant limits from db: {err}")))?;

        let project_rows: Vec<(i64, i64, i64, String, i32)> = sqlx::query_as(
            "SELECT p.id, p.tenant_id, p.workspace_id, p.dsn_public_key,
                    COALESCE(
                        p.rate_limit_per_minute_override,
                        w.default_rate_limit_per_minute_override,
                        t.default_rate_limit_per_minute
                    ) AS effective_rpm
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             JOIN tenants t ON t.id = p.tenant_id",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("reload dsn cache from db: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::DatabaseQuery(format!("commit reload dsn cache: {err}")))?;

        self.db_reload_count.fetch_add(1, Ordering::Relaxed);

        {
            let mut t_guard = match self.tenants.write() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut next_tenants = HashMap::with_capacity(tenant_rows.len());
            for (tenant_id, agg_i32) in tenant_rows {
                let agg_rpm = u32::try_from(agg_i32.max(1)).unwrap_or(DEFAULT_TENANT_AGGREGATE_RPM);
                let entry = CachedTenantLimiter::new(agg_rpm, t_guard.get(&tenant_id));
                next_tenants.insert(tenant_id, entry);
            }
            *t_guard = next_tenants;
        }

        let mut p_guard = match self.projects.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        let mut next_map = HashMap::with_capacity(project_rows.len());
        for (project_id, tenant_id, workspace_id, dsn_public_key, rpm_i32) in project_rows {
            let rpm = u32::try_from(rpm_i32.max(1)).unwrap_or(1200);
            let existing_limiter = p_guard
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
        *p_guard = next_map;
        Ok(count)
    }

    /// Upsert a single project entry in memory (called when a project is created, updated, or its DSN is rotated).
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

    /// Set or update the aggregate rate limit for a tenant directly in memory.
    pub fn set_tenant_aggregate_limit(&self, tenant_id: i64, aggregate_rpm: u32) {
        let mut guard = match self.tenants.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = CachedTenantLimiter::new(aggregate_rpm, guard.get(&tenant_id));
        guard.insert(tenant_id, entry);
    }

    /// Apply a live tenant/workspace rate-limit inheritance update without querying PostgreSQL.
    pub fn apply_rate_limit_update(&self, update: TenantRateLimitUpdate) {
        if let Some(agg_rpm) = update.aggregate_rate_limit_per_minute {
            self.set_tenant_aggregate_limit(update.tenant_id, agg_rpm);
        }

        if !update.project_effective_limits.is_empty() {
            let mut p_guard = match self.projects.write() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            for (project_id, new_rpm) in update.project_effective_limits {
                if let Some(existing) = p_guard.get_mut(&project_id)
                    && existing.tenant_id == update.tenant_id
                {
                    let clamped = new_rpm.max(1);
                    if existing.rate_limit_per_minute != clamped {
                        existing.rate_limit_per_minute = clamped;
                        existing.limiter = Arc::new(build_rate_limiter(clamped));
                    }
                }
            }
        }
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

    /// Check BOTH the per-project `governor` token-bucket AND the aggregate per-tenant token-bucket
    /// for `item_count` events. Checking per-project first ensures a single noisy project exceeding its own
    /// limit does not consume sibling projects' shared tenant aggregate quota.
    pub fn check_rate_limit(
        &self,
        project: &CachedProject,
        item_count: u32,
    ) -> Result<(), RateLimitHit> {
        let count = item_count.max(1);
        for _ in 0..count {
            if let Err(not_until) = project.limiter.check() {
                let now = DefaultClock::default().now();
                let secs = not_until.wait_time_from(now).as_secs().max(1);
                return Err(RateLimitHit {
                    scope: RateLimitScope::Project,
                    retry_after_secs: secs,
                });
            }
        }

        if let Some(tenant_limiter) = self.tenant_limiter(project.tenant_id) {
            for _ in 0..count {
                if let Err(not_until) = tenant_limiter.check() {
                    let now = DefaultClock::default().now();
                    let secs = not_until.wait_time_from(now).as_secs().max(1);
                    return Err(RateLimitHit {
                        scope: RateLimitScope::Tenant,
                        retry_after_secs: secs,
                    });
                }
            }
        }

        Ok(())
    }

    fn tenant_limiter(&self, tenant_id: i64) -> Option<Arc<DefaultDirectRateLimiter>> {
        let guard = match self.tenants.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.get(&tenant_id).map(|t| Arc::clone(&t.limiter))
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

    fn on_tenant_rate_limits_updated(&self, update: TenantRateLimitUpdate) {
        self.apply_rate_limit_update(update);
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

        // Quota is 2 events/minute: first 2 succeed, 3rd returns Project rate limit.
        assert_eq!(cache.check_rate_limit(&proj, 1), Ok(()));
        assert_eq!(cache.check_rate_limit(&proj, 1), Ok(()));
        let hit = cache.check_rate_limit(&proj, 1).unwrap_err();
        assert_eq!(hit.scope, RateLimitScope::Project);
        assert!(hit.retry_after_secs >= 1);
    }

    #[test]
    fn enforces_aggregate_tenant_limit_across_sibling_projects() {
        let cache = DsnCache::new();
        cache.set_tenant_aggregate_limit(7, 3);
        cache.upsert(ProjectDsnInfo {
            project_id: 101,
            tenant_id: 7,
            workspace_id: 1,
            dsn_public_key: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            rate_limit_per_minute: 10,
        });
        cache.upsert(ProjectDsnInfo {
            project_id: 102,
            tenant_id: 7,
            workspace_id: 1,
            dsn_public_key: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
            rate_limit_per_minute: 10,
        });

        let p1 = cache
            .authenticate(101, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap();
        let p2 = cache
            .authenticate(102, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .unwrap();

        // 2 events on P1 + 1 event on P2 exhaust the shared tenant aggregate quota of 3/min.
        assert_eq!(cache.check_rate_limit(&p1, 2), Ok(()));
        assert_eq!(cache.check_rate_limit(&p2, 1), Ok(()));

        // 4th event on P2 is still under P2's limit (10/min) but hits Tenant aggregate cap!
        let hit = cache.check_rate_limit(&p2, 1).unwrap_err();
        assert_eq!(hit.scope, RateLimitScope::Tenant);
        assert_eq!(hit.scope.sentry_scope(), "organization");
        assert!(hit.retry_after_secs >= 1);
    }
}
