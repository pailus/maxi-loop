use std::collections::HashMap;
use std::sync::Arc;

use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::begin_worker_tx;
use serde_json::{Map, Value, json};
use sourcemap::DecodedMap;
use sqlx::PgPool;
use tracing::warn;

use crate::cache::{CachedMapEntry, SourceMapCacheKey, SourceMapLruCache};
use crate::codec::{canonicalize_js_path, decompress_and_parse_sourcemap, normalize_debug_id};
use crate::model::SourceMapCacheStats;

const CONTEXT_LINES_RADIUS: usize = 5;

/// Ingest-time JavaScript / React stack frame resolver backed by PostgreSQL + a bounded LRU cache.
#[derive(Clone)]
pub struct SourceMapResolver {
    pool: PgPool,
    cache: SourceMapLruCache,
    max_raw_bytes: usize,
}

impl SourceMapResolver {
    pub fn new(
        pool: PgPool,
        max_raw_bytes: usize,
        cache_max_bytes: usize,
        cache_max_entries: usize,
    ) -> Self {
        Self {
            pool,
            cache: SourceMapLruCache::new(cache_max_bytes, cache_max_entries),
            max_raw_bytes,
        }
    }

    pub fn invalidate_project(&self, tenant_id: i64, project_id: i64) {
        self.cache.invalidate_project(tenant_id, project_id);
    }

    pub fn cache_stats(&self) -> SourceMapCacheStats {
        self.cache.stats()
    }

    /// Resolve minified JS/React stack frames in `payload` in-place BEFORE fingerprinting (Design §5.3).
    /// Returns the number of stack frames resolved via source maps.
    pub async fn resolve_event_payload(
        &self,
        tenant_id: i64,
        project_id: i64,
        platform: &str,
        release: Option<&str>,
        payload: &mut Value,
    ) -> usize {
        if !is_js_candidate_event(platform, payload) {
            return 0;
        }

        let debug_meta_map = extract_debug_meta_images(payload);
        let release_trimmed = release.map(str::trim).filter(|s| !s.is_empty());

        let mut resolved_count = 0usize;

        // 1. Resolve frames inside `exception.values[].stacktrace.frames` (or `exception[].stacktrace.frames`)
        if let Some(exc) = payload.get_mut("exception") {
            let values_arr = if let Some(arr) = exc.get_mut("values").and_then(Value::as_array_mut)
            {
                Some(arr)
            } else {
                exc.as_array_mut()
            };

            if let Some(items) = values_arr {
                for item in items {
                    if let Some(st) = item.get_mut("stacktrace") {
                        resolved_count += self
                            .resolve_stacktrace_node(
                                tenant_id,
                                project_id,
                                release_trimmed,
                                &debug_meta_map,
                                st,
                            )
                            .await;
                    }
                }
            }
        }

        // 2. Resolve frames inside top-level `stacktrace.frames` if present
        if let Some(st) = payload.get_mut("stacktrace") {
            resolved_count += self
                .resolve_stacktrace_node(
                    tenant_id,
                    project_id,
                    release_trimmed,
                    &debug_meta_map,
                    st,
                )
                .await;
        }

        resolved_count
    }

    async fn resolve_stacktrace_node(
        &self,
        tenant_id: i64,
        project_id: i64,
        release: Option<&str>,
        debug_meta_map: &HashMap<String, String>,
        stacktrace: &mut Value,
    ) -> usize {
        let frames = if let Some(arr) = stacktrace.get_mut("frames").and_then(Value::as_array_mut) {
            arr
        } else if let Some(arr) = stacktrace.as_array_mut() {
            arr
        } else {
            return 0;
        };

        let mut resolved = 0usize;
        for frame_val in frames.iter_mut() {
            let Some(frame) = frame_val.as_object_mut() else {
                continue;
            };
            if self
                .resolve_single_frame(tenant_id, project_id, release, debug_meta_map, frame)
                .await
            {
                resolved += 1;
            }
        }
        resolved
    }

    async fn resolve_single_frame(
        &self,
        tenant_id: i64,
        project_id: i64,
        release: Option<&str>,
        debug_meta_map: &HashMap<String, String>,
        frame: &mut Map<String, Value>,
    ) -> bool {
        let Some(lineno_1based) = frame.get("lineno").and_then(Value::as_u64) else {
            return false;
        };
        if lineno_1based == 0 {
            return false;
        }
        let colno_raw = frame.get("colno").and_then(Value::as_u64).unwrap_or(1);

        let raw_abs = frame
            .get("abs_path")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let raw_file = frame
            .get("filename")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);

        // Determine candidate debug_id: explicit `frame.debug_id` or `debug_meta.images` match.
        let frame_debug_id = frame
            .get("debug_id")
            .and_then(Value::as_str)
            .and_then(|s| normalize_debug_id(Some(s)).ok().flatten())
            .or_else(|| {
                lookup_debug_id_for_frame(debug_meta_map, raw_abs.as_deref(), raw_file.as_deref())
            });

        // 1. Try matching by `debug_id` first (exact, release-independent per Sentry Debug IDs spec).
        let mut decoded_map: Option<Arc<DecodedMap>> = None;
        if let Some(debug_id) = &frame_debug_id {
            decoded_map = self.load_by_debug_id(tenant_id, project_id, debug_id).await;
        }

        // 2. Fallback to `release + filename` matching (canonical path, then basename).
        if decoded_map.is_none()
            && let Some(rel) = release
        {
            let candidates = build_filename_candidates(raw_abs.as_deref(), raw_file.as_deref());
            for candidate in candidates {
                if let Some(found) = self
                    .load_by_release_filename(tenant_id, project_id, rel, &candidate)
                    .await
                {
                    decoded_map = Some(found);
                    break;
                }
            }
        }

        let Some(sm) = decoded_map else {
            return false;
        };

        apply_sourcemap_to_frame(
            &sm,
            lineno_1based,
            colno_raw,
            raw_abs.as_deref().or(raw_file.as_deref()),
            frame,
        )
    }

    async fn load_by_debug_id(
        &self,
        tenant_id: i64,
        project_id: i64,
        debug_id: &str,
    ) -> Option<Arc<DecodedMap>> {
        let key = SourceMapCacheKey::ByDebugId {
            tenant_id,
            project_id,
            debug_id: debug_id.to_owned(),
        };
        if let Some(cached) = self.cache.get(&key) {
            return match cached {
                CachedMapEntry::Found(sm) => Some(sm),
                CachedMapEntry::NotFound => None,
            };
        }

        let fetched = self
            .fetch_blob_by_debug_id(tenant_id, project_id, debug_id)
            .await;
        self.decode_and_cache(key, fetched)
    }

    async fn load_by_release_filename(
        &self,
        tenant_id: i64,
        project_id: i64,
        release: &str,
        filename: &str,
    ) -> Option<Arc<DecodedMap>> {
        let key = SourceMapCacheKey::ByReleaseFilename {
            tenant_id,
            project_id,
            release: release.to_owned(),
            filename: filename.to_owned(),
        };
        if let Some(cached) = self.cache.get(&key) {
            return match cached {
                CachedMapEntry::Found(sm) => Some(sm),
                CachedMapEntry::NotFound => None,
            };
        }

        let fetched = self
            .fetch_blob_by_release_filename(tenant_id, project_id, release, filename)
            .await;
        self.decode_and_cache(key, fetched)
    }

    fn decode_and_cache(
        &self,
        key: SourceMapCacheKey,
        fetched: Result<Option<Vec<u8>>, AppError>,
    ) -> Option<Arc<DecodedMap>> {
        match fetched {
            Ok(Some(zstd_bytes)) => {
                match decompress_and_parse_sourcemap(&zstd_bytes, self.max_raw_bytes) {
                    Ok((decoded, raw_len)) => {
                        let arc = Arc::new(decoded);
                        self.cache
                            .insert(key, CachedMapEntry::Found(Arc::clone(&arc)), raw_len);
                        Some(arc)
                    }
                    Err(err) => {
                        warn!(error = %err, "failed to parse cached/stored source map");
                        None
                    }
                }
            }
            Ok(None) => {
                self.cache.insert(key, CachedMapEntry::NotFound, 0);
                None
            }
            Err(err) => {
                warn!(error = %err, "failed to query source map from PostgreSQL");
                None
            }
        }
    }

    async fn fetch_blob_by_debug_id(
        &self,
        tenant_id: i64,
        project_id: i64,
        debug_id: &str,
    ) -> Result<Option<Vec<u8>>, AppError> {
        let mut tx = begin_worker_tx(&self.pool).await?;
        let blob: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT content_zstd
             FROM source_maps
             WHERE tenant_id = $1 AND project_id = $2 AND debug_id = $3
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(debug_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup sourcemap by debug_id: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit sourcemap debug_id lookup: {err}"))
        })?;
        Ok(blob)
    }

    async fn fetch_blob_by_release_filename(
        &self,
        tenant_id: i64,
        project_id: i64,
        release: &str,
        filename: &str,
    ) -> Result<Option<Vec<u8>>, AppError> {
        let mut tx = begin_worker_tx(&self.pool).await?;
        let blob: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT content_zstd
             FROM source_maps
             WHERE tenant_id = $1 AND project_id = $2 AND release_version = $3 AND filename = $4
             LIMIT 1",
        )
        .bind(tenant_id)
        .bind(project_id)
        .bind(release)
        .bind(filename)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("lookup sourcemap by release+filename: {err}"))
        })?;
        tx.commit().await.map_err(|err| {
            AppError::DatabaseQuery(format!("commit sourcemap filename lookup: {err}"))
        })?;
        Ok(blob)
    }
}

/// Apply a parsed `DecodedMap` to a single mutable JSON stack frame.
pub fn apply_sourcemap_to_frame(
    sm: &DecodedMap,
    lineno_1based: u64,
    colno_raw: u64,
    orig_path: Option<&str>,
    frame: &mut Map<String, Value>,
) -> bool {
    let line_0 = u32::try_from(lineno_1based.saturating_sub(1)).unwrap_or(u32::MAX);
    // Browser / Sentry JS colno is usually 1-based; try `col - 1` first, then `col` as fallback.
    let col_0 = u32::try_from(colno_raw.saturating_sub(1)).unwrap_or(0);
    let col_alt = u32::try_from(colno_raw).unwrap_or(0);

    let token = sm
        .lookup_token(line_0, col_0)
        .or_else(|| sm.lookup_token(line_0, col_alt));
    let Some(token) = token else {
        return false;
    };

    let Some(raw_source) = token.get_source() else {
        return false;
    };
    let clean_source = clean_resolved_source_path(raw_source);
    if clean_source.is_empty() {
        return false;
    }

    let resolved_line = u64::from(token.get_src_line()) + 1;
    let resolved_col = u64::from(token.get_src_col()) + 1;

    frame.insert("filename".to_owned(), Value::String(clean_source.clone()));
    frame.insert("abs_path".to_owned(), Value::String(clean_source.clone()));
    frame.insert("lineno".to_owned(), json!(resolved_line));
    frame.insert("colno".to_owned(), json!(resolved_col));

    if let Some(name) = token.get_name().map(str::trim).filter(|s| !s.is_empty()) {
        frame.insert("function".to_owned(), Value::String(name.to_owned()));
    }

    // Mark third-party vendor frames (`node_modules/`) as `in_app = false` and app sources as `in_app = true`.
    let is_vendor = clean_source.contains("node_modules/");
    frame.insert("in_app".to_owned(), json!(!is_vendor));

    // Populate `pre_context`, `context_line`, and `post_context` from embedded `sourcesContent`.
    if let Some(sv) = token.get_source_view() {
        let src_line_idx = token.get_src_line() as usize;
        let raw_line_count = sv.line_count();
        let line_count =
            if raw_line_count > 1 && sv.get_line((raw_line_count - 1) as u32) == Some("") {
                raw_line_count - 1
            } else {
                raw_line_count
            };
        if src_line_idx < line_count {
            if let Some(ctx) = sv.get_line(src_line_idx as u32) {
                frame.insert("context_line".to_owned(), Value::String(ctx.to_owned()));
            }
            let pre_start = src_line_idx.saturating_sub(CONTEXT_LINES_RADIUS);
            let pre: Vec<Value> = (pre_start..src_line_idx)
                .filter_map(|i| sv.get_line(i as u32).map(|l| Value::String(l.to_owned())))
                .collect();
            let post_end = (src_line_idx + 1 + CONTEXT_LINES_RADIUS).min(line_count);
            let post: Vec<Value> = ((src_line_idx + 1)..post_end)
                .filter_map(|i| sv.get_line(i as u32).map(|l| Value::String(l.to_owned())))
                .collect();
            frame.insert("pre_context".to_owned(), Value::Array(pre));
            frame.insert("post_context".to_owned(), Value::Array(post));
        }
    }

    // Record non-sensitive resolution provenance inside `frame.data`.
    let mut data_obj = frame
        .get("data")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    data_obj.insert("sourcemap_resolved".to_owned(), json!(true));
    if let Some(orig) = orig_path {
        data_obj.insert("orig_filename".to_owned(), Value::String(orig.to_owned()));
    }
    data_obj.insert("orig_lineno".to_owned(), json!(lineno_1based));
    data_obj.insert("orig_colno".to_owned(), json!(colno_raw));
    frame.insert("data".to_owned(), Value::Object(data_obj));

    true
}

fn is_js_candidate_event(platform: &str, payload: &Value) -> bool {
    let p = platform.trim().to_ascii_lowercase();
    if matches!(
        p.as_str(),
        "javascript" | "node" | "react" | "browser" | "typescript"
    ) {
        return true;
    }

    if payload
        .get("debug_meta")
        .and_then(|d| d.get("images"))
        .and_then(Value::as_array)
        .is_some_and(|arr| {
            arr.iter()
                .any(|img| img.get("type").and_then(Value::as_str) == Some("sourcemap"))
        })
    {
        return true;
    }

    // Fallback: check whether any exception frame filename looks like a JS/TS asset.
    let exc_values = payload.get("exception").and_then(|e| {
        e.get("values")
            .and_then(Value::as_array)
            .or_else(|| e.as_array())
    });
    if let Some(items) = exc_values {
        for item in items {
            if let Some(frames) = item
                .get("stacktrace")
                .and_then(|st| st.get("frames"))
                .and_then(Value::as_array)
            {
                for f in frames {
                    for key in ["abs_path", "filename"] {
                        if let Some(path) = f.get(key).and_then(Value::as_str) {
                            let canonical = canonicalize_js_path(path).to_ascii_lowercase();
                            if canonical.ends_with(".js")
                                || canonical.ends_with(".mjs")
                                || canonical.ends_with(".cjs")
                                || canonical.ends_with(".jsx")
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }

    false
}

fn extract_debug_meta_images(payload: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(images) = payload
        .get("debug_meta")
        .and_then(|d| d.get("images"))
        .and_then(Value::as_array)
    else {
        return out;
    };

    for img in images {
        let Some(debug_id) = img
            .get("debug_id")
            .and_then(Value::as_str)
            .and_then(|s| normalize_debug_id(Some(s)).ok().flatten())
        else {
            continue;
        };
        if let Some(code_file) = img.get("code_file").and_then(Value::as_str) {
            let trimmed = code_file.trim();
            if !trimmed.is_empty() {
                out.insert(trimmed.to_owned(), debug_id.clone());
                let canonical = canonicalize_js_path(trimmed);
                if !canonical.is_empty() {
                    out.insert(canonical.clone(), debug_id.clone());
                    if let Some((_, base)) = canonical.rsplit_once('/') {
                        out.entry(base.to_owned())
                            .or_insert_with(|| debug_id.clone());
                    }
                }
            }
        }
    }
    out
}

fn lookup_debug_id_for_frame(
    map: &HashMap<String, String>,
    abs_path: Option<&str>,
    filename: Option<&str>,
) -> Option<String> {
    if map.is_empty() {
        return None;
    }
    for raw in [abs_path, filename].into_iter().flatten() {
        let trimmed = raw.trim();
        if let Some(id) = map.get(trimmed) {
            return Some(id.clone());
        }
        let canonical = canonicalize_js_path(trimmed);
        if let Some(id) = map.get(&canonical) {
            return Some(id.clone());
        }
        if let Some((_, base)) = canonical.rsplit_once('/')
            && let Some(id) = map.get(base)
        {
            return Some(id.clone());
        }
    }
    None
}

fn build_filename_candidates(abs_path: Option<&str>, filename: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut push_unique = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };

    for raw in [abs_path, filename].into_iter().flatten() {
        let canonical = canonicalize_js_path(raw);
        if !canonical.is_empty() {
            push_unique(canonical.clone());
            if let Some((_, base)) = canonical.rsplit_once('/') {
                push_unique(base.to_owned());
            }
        }
    }
    out
}

fn clean_resolved_source_path(raw: &str) -> String {
    let mut s = raw.trim().replace('\\', "/");
    for prefix in [
        "webpack:///",
        "webpack://",
        "vite:///",
        "file:///",
        "app:///",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_owned();
            break;
        }
    }
    while let Some(rest) = s.strip_prefix("../") {
        s = rest.to_owned();
    }
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_owned();
    }
    s.trim_start_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_minified_react_frame_with_source_context_and_original_name() {
        // Source map mapping minified (line 1, col 1) -> original `src/Checkout.tsx` (line 3, col 3), name `submitOrder`
        let sm_json = json!({
            "version": 3,
            "file": "bundle-a1b2.js",
            "sources": ["webpack:///./src/Checkout.tsx"],
            "sourcesContent": [
                "import React from 'react';\nexport function submitOrder(cart: any) {\n  if (!cart) throw new TypeError('empty cart');\n  return cart.total;\n}\n"
            ],
            "names": ["submitOrder"],
            "mappings": "AAECA"
        });
        let sm = sourcemap::decode_slice(sm_json.to_string().as_bytes()).unwrap();

        let mut frame = json!({
            "filename": "~/assets/bundle-a1b2.js",
            "abs_path": "https://app.example.com/assets/bundle-a1b2.js",
            "function": "e",
            "lineno": 1,
            "colno": 1,
            "in_app": true
        })
        .as_object()
        .unwrap()
        .clone();

        let changed = apply_sourcemap_to_frame(
            &sm,
            1,
            1,
            Some("https://app.example.com/assets/bundle-a1b2.js"),
            &mut frame,
        );
        assert!(changed);
        assert_eq!(frame["filename"], "src/Checkout.tsx");
        assert_eq!(frame["function"], "submitOrder");
        assert_eq!(frame["lineno"], 3);
        assert_eq!(frame["in_app"], true);
        assert_eq!(
            frame["context_line"],
            "  if (!cart) throw new TypeError('empty cart');"
        );
        assert_eq!(frame["pre_context"].as_array().unwrap().len(), 2);
        assert_eq!(frame["post_context"].as_array().unwrap().len(), 2);
        assert_eq!(frame["data"]["sourcemap_resolved"], true);
    }

    #[test]
    fn marks_node_modules_sources_as_not_in_app() {
        let sm_json = json!({
            "version": 3,
            "file": "vendor.js",
            "sources": ["../node_modules/react-dom/cjs/react-dom.production.min.js"],
            "names": ["renderRoot"],
            "mappings": "AAAAA"
        });
        let sm = sourcemap::decode_slice(sm_json.to_string().as_bytes()).unwrap();
        let mut frame = json!({
            "filename": "assets/vendor.js",
            "lineno": 1,
            "colno": 1,
            "in_app": true
        })
        .as_object()
        .unwrap()
        .clone();

        assert!(apply_sourcemap_to_frame(&sm, 1, 1, None, &mut frame));
        assert_eq!(frame["in_app"], false);
        assert_eq!(
            frame["filename"],
            "node_modules/react-dom/cjs/react-dom.production.min.js"
        );
    }
}
