use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use sourcemap::DecodedMap;

use crate::model::SourceMapCacheStats;

/// Lookup key scoped strictly by `(tenant_id, project_id)` to guarantee zero cross-tenant sharing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceMapCacheKey {
    ByDebugId {
        tenant_id: i64,
        project_id: i64,
        debug_id: String,
    },
    ByReleaseFilename {
        tenant_id: i64,
        project_id: i64,
        release: String,
        filename: String,
    },
}

impl SourceMapCacheKey {
    pub fn tenant_project(&self) -> (i64, i64) {
        match self {
            Self::ByDebugId {
                tenant_id,
                project_id,
                ..
            }
            | Self::ByReleaseFilename {
                tenant_id,
                project_id,
                ..
            } => (*tenant_id, *project_id),
        }
    }
}

#[derive(Clone)]
pub enum CachedMapEntry {
    Found(Arc<DecodedMap>),
    NotFound,
}

struct StoredEntry {
    value: CachedMapEntry,
    weight_bytes: usize,
    generation: u64,
}

struct LruInner {
    max_bytes: usize,
    max_entries: usize,
    used_bytes: usize,
    next_gen: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    map: HashMap<SourceMapCacheKey, StoredEntry>,
    order: VecDeque<(SourceMapCacheKey, u64)>,
}

/// Thread-safe, memory-bounded (`max_bytes`) AND entry-bounded (`max_entries`) LRU cache
/// for lazily parsed `sourcemap::DecodedMap` instances (Design §5.7).
#[derive(Clone)]
pub struct SourceMapLruCache {
    inner: Arc<Mutex<LruInner>>,
}

impl SourceMapLruCache {
    pub fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LruInner {
                max_bytes: max_bytes.max(1024),
                max_entries: max_entries.max(1),
                used_bytes: 0,
                next_gen: 1,
                hits: 0,
                misses: 0,
                evictions: 0,
                map: HashMap::new(),
                order: VecDeque::new(),
            })),
        }
    }

    /// Look up a key, promoting it to most-recently-used on hit.
    pub fn get(&self, key: &SourceMapCacheKey) -> Option<CachedMapEntry> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let next_gen = guard.next_gen;
        if let Some(entry) = guard.map.get_mut(key) {
            entry.generation = next_gen;
            let cloned = entry.value.clone();
            guard.next_gen = next_gen.wrapping_add(1);
            guard.hits = guard.hits.saturating_add(1);
            guard.order.push_back((key.clone(), next_gen));
            compact_order_if_needed(&mut guard);
            Some(cloned)
        } else {
            guard.misses = guard.misses.saturating_add(1);
            None
        }
    }

    /// Insert a parsed source map (or negative lookup) with conservative memory weight,
    /// evicting least-recently-used entries to stay strictly within `max_bytes` and `max_entries`.
    pub fn insert(&self, key: SourceMapCacheKey, value: CachedMapEntry, raw_json_bytes: usize) {
        // Conservative memory accounting: parsed VLQ token tables + source lines ≈ 2x raw JSON size
        // (minimum 256 bytes for negative entries).
        let weight = match &value {
            CachedMapEntry::Found(_) => raw_json_bytes.saturating_mul(2).max(1024),
            CachedMapEntry::NotFound => 256,
        };

        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if weight > guard.max_bytes {
            // Single oversized source map still resolves for the caller, but is not retained in RAM.
            return;
        }

        if let Some(prev) = guard.map.remove(&key) {
            guard.used_bytes = guard.used_bytes.saturating_sub(prev.weight_bytes);
        }

        while (!guard.map.is_empty())
            && (guard.used_bytes.saturating_add(weight) > guard.max_bytes
                || guard.map.len() >= guard.max_entries)
        {
            if !evict_one_lru(&mut guard) {
                break;
            }
        }

        let gen_id = guard.next_gen;
        guard.next_gen = gen_id.wrapping_add(1);
        guard.used_bytes = guard.used_bytes.saturating_add(weight);
        guard.map.insert(
            key.clone(),
            StoredEntry {
                value,
                weight_bytes: weight,
                generation: gen_id,
            },
        );
        guard.order.push_back((key, gen_id));
        compact_order_if_needed(&mut guard);
    }

    /// Invalidate all cached entries (positive and negative) for a project after source map upload/delete.
    pub fn invalidate_project(&self, tenant_id: i64, project_id: i64) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut removed_bytes = 0usize;
        guard.map.retain(|k, v| {
            let keep = k.tenant_project() != (tenant_id, project_id);
            if !keep {
                removed_bytes = removed_bytes.saturating_add(v.weight_bytes);
            }
            keep
        });
        guard.used_bytes = guard.used_bytes.saturating_sub(removed_bytes);
        guard
            .order
            .retain(|(k, _)| k.tenant_project() != (tenant_id, project_id));
    }

    pub fn stats(&self) -> SourceMapCacheStats {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        SourceMapCacheStats {
            entries: guard.map.len(),
            used_bytes: guard.used_bytes,
            max_bytes: guard.max_bytes,
            max_entries: guard.max_entries,
            hits: guard.hits,
            misses: guard.misses,
            evictions: guard.evictions,
        }
    }
}

fn evict_one_lru(guard: &mut LruInner) -> bool {
    while let Some((cand_key, cand_gen)) = guard.order.pop_front() {
        let is_live = guard
            .map
            .get(&cand_key)
            .is_some_and(|entry| entry.generation == cand_gen);
        if is_live && let Some(removed) = guard.map.remove(&cand_key) {
            guard.used_bytes = guard.used_bytes.saturating_sub(removed.weight_bytes);
            guard.evictions = guard.evictions.saturating_add(1);
            return true;
        }
    }
    false
}

fn compact_order_if_needed(guard: &mut LruInner) {
    if guard.order.len() > guard.max_entries.saturating_mul(4).max(64) {
        let map = &guard.map;
        guard
            .order
            .retain(|(k, g)| map.get(k).is_some_and(|e| e.generation == *g));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI_MAP: &str = r#"{"version":3,"sources":["src/a.ts"],"names":[],"mappings":"AAAA"}"#;

    #[test]
    fn enforces_max_entries_and_lru_order() {
        let cache = SourceMapLruCache::new(10 * 1024 * 1024, 2);
        let decoded = Arc::new(sourcemap::decode_slice(MINI_MAP.as_bytes()).unwrap());

        let k1 = SourceMapCacheKey::ByDebugId {
            tenant_id: 1,
            project_id: 1,
            debug_id: "id-1".to_owned(),
        };
        let k2 = SourceMapCacheKey::ByDebugId {
            tenant_id: 1,
            project_id: 1,
            debug_id: "id-2".to_owned(),
        };
        let k3 = SourceMapCacheKey::ByDebugId {
            tenant_id: 1,
            project_id: 1,
            debug_id: "id-3".to_owned(),
        };

        cache.insert(k1.clone(), CachedMapEntry::Found(Arc::clone(&decoded)), 100);
        cache.insert(k2.clone(), CachedMapEntry::Found(Arc::clone(&decoded)), 100);
        // Touch k1 so k2 becomes least recently used.
        assert!(cache.get(&k1).is_some());

        cache.insert(k3.clone(), CachedMapEntry::Found(Arc::clone(&decoded)), 100);
        assert!(cache.get(&k1).is_some());
        assert!(cache.get(&k2).is_none(), "k2 should have been evicted");
        assert!(cache.get(&k3).is_some());
        assert_eq!(cache.stats().entries, 2);
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn enforces_max_bytes_memory_budget_and_project_invalidation() {
        // Budget = 5000 bytes; each 2000-byte raw map weighs 4000 bytes -> only 1 fits!
        let cache = SourceMapLruCache::new(5000, 10);
        let decoded = Arc::new(sourcemap::decode_slice(MINI_MAP.as_bytes()).unwrap());

        let k1 = SourceMapCacheKey::ByReleaseFilename {
            tenant_id: 1,
            project_id: 10,
            release: "v1".to_owned(),
            filename: "assets/a.js".to_owned(),
        };
        let k2 = SourceMapCacheKey::ByReleaseFilename {
            tenant_id: 1,
            project_id: 10,
            release: "v1".to_owned(),
            filename: "assets/b.js".to_owned(),
        };

        cache.insert(
            k1.clone(),
            CachedMapEntry::Found(Arc::clone(&decoded)),
            2000,
        );
        assert_eq!(cache.stats().used_bytes, 4000);

        cache.insert(
            k2.clone(),
            CachedMapEntry::Found(Arc::clone(&decoded)),
            2000,
        );
        assert!(cache.get(&k1).is_none(), "k1 evicted by byte budget");
        assert!(cache.get(&k2).is_some());
        assert!(cache.stats().used_bytes <= 5000);

        cache.invalidate_project(1, 10);
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().used_bytes, 0);
    }
}
