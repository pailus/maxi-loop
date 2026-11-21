use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub_notify::{
    PiiScrubRuleEntry, PiiScrubRuleType, PiiScrubRulesObserver,
};
use maxi_loop_platform::tenant::begin_worker_tx;
use sqlx::PgPool;

/// Thread-safe in-memory cache of configurable PII scrub rules grouped by `tenant_id`.
/// Keeps the ingest worker hot path free of per-event DB queries while immediately reflecting
/// rule changes made through the API.
#[derive(Clone, Debug, Default)]
pub struct ScrubRulesCache {
    by_tenant: Arc<RwLock<HashMap<i64, Vec<PiiScrubRuleEntry>>>>,
}

#[derive(sqlx::FromRow)]
struct PiiRuleDbRow {
    id: i64,
    tenant_id: i64,
    workspace_id: Option<i64>,
    project_id: Option<i64>,
    rule_type: String,
    pattern: String,
    replacement: String,
    is_enabled: bool,
}

impl ScrubRulesCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Preload all configurable PII scrub rules from PostgreSQL into memory.
    pub async fn reload_from_db(&self, pool: &PgPool) -> Result<usize, AppError> {
        let mut tx = begin_worker_tx(pool).await?;
        let rows: Vec<PiiRuleDbRow> = sqlx::query_as(
            "SELECT id, tenant_id, workspace_id, project_id, rule_type, pattern, replacement, is_enabled
             FROM pii_scrub_rules
             ORDER BY tenant_id ASC, id ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("reload pii scrub rules cache: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit pii scrub rules reload: {err}"))
        })?;

        let mut next: HashMap<i64, Vec<PiiScrubRuleEntry>> = HashMap::new();
        let mut count = 0usize;
        for row in rows {
            let Some(rule_type) = PiiScrubRuleType::parse(&row.rule_type) else {
                continue;
            };
            next.entry(row.tenant_id)
                .or_default()
                .push(PiiScrubRuleEntry {
                    id: row.id,
                    tenant_id: row.tenant_id,
                    workspace_id: row.workspace_id,
                    project_id: row.project_id,
                    rule_type,
                    pattern: row.pattern,
                    replacement: row.replacement,
                    is_enabled: row.is_enabled,
                });
            count += 1;
        }

        let mut guard = match self.by_tenant.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = next;
        Ok(count)
    }

    pub fn upsert(&self, rule: PiiScrubRuleEntry) {
        let mut guard = match self.by_tenant.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let list = guard.entry(rule.tenant_id).or_default();
        if let Some(existing) = list.iter_mut().find(|r| r.id == rule.id) {
            *existing = rule;
        } else {
            list.push(rule);
        }
    }

    pub fn remove(&self, tenant_id: i64, rule_id: i64) {
        let mut guard = match self.by_tenant.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(list) = guard.get_mut(&tenant_id) {
            list.retain(|r| r.id != rule_id);
        }
    }

    /// Resolve all enabled inherited rules matching `(tenant_id, workspace_id, project_id)`:
    /// union of tenant-wide rules, workspace-scoped rules, and project-scoped rules.
    pub fn effective_rules(
        &self,
        tenant_id: i64,
        workspace_id: i64,
        project_id: i64,
    ) -> Vec<PiiScrubRuleEntry> {
        let guard = match self.by_tenant.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(list) = guard.get(&tenant_id) else {
            return Vec::new();
        };

        list.iter()
            .filter(|rule| {
                if !rule.is_enabled {
                    return false;
                }
                match (rule.workspace_id, rule.project_id) {
                    (None, None) => true,
                    (Some(ws), None) => ws == workspace_id,
                    (Some(ws), Some(proj)) => ws == workspace_id && proj == project_id,
                    (None, Some(proj)) => proj == project_id,
                }
            })
            .cloned()
            .collect()
    }
}

impl PiiScrubRulesObserver for ScrubRulesCache {
    fn on_pii_rule_upserted(&self, rule: PiiScrubRuleEntry) {
        self.upsert(rule);
    }

    fn on_pii_rule_deleted(&self, tenant_id: i64, rule_id: i64) {
        self.remove(tenant_id, rule_id);
    }
}
