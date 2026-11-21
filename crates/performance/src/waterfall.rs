use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub::{scrub_event_payload_with_rules, scrub_text_with_rules};
use maxi_loop_platform::scrub_notify::PiiScrubRuleEntry;
use serde_json::{Value, json};

use crate::codec::decompress_trace_blob;
use crate::histogram::round_two_decimals;
use crate::ingest_proc::timestamp_to_ms;
use crate::model::{SlowSpanHighlight, TraceWaterfallView, WaterfallSpanNode};

#[derive(Clone, Debug)]
pub struct StoredTraceSegment {
    pub event_id: String,
    pub project_id: i64,
    pub project_slug: String,
    pub workspace_slug: String,
    pub trace_id: String,
    pub root_span_id: String,
    pub parent_span_id: Option<String>,
    pub transaction_name: String,
    pub op: String,
    pub status: String,
    pub is_error: bool,
    pub sample_reason: String,
    pub environment: String,
    pub release: Option<String>,
    pub platform: String,
    pub start_timestamp: DateTime<Utc>,
    pub end_timestamp: DateTime<Utc>,
    pub duration_ms: f64,
    pub tags: Value,
    pub spans_zstd: Vec<u8>,
}

struct RawNode {
    span_id: String,
    parent_span_id: Option<String>,
    project_id: i64,
    project_slug: String,
    platform: String,
    environment: String,
    release: Option<String>,
    op: String,
    description: String,
    status: String,
    is_error: bool,
    is_transaction_root: bool,
    start_timestamp_ms: f64,
    end_timestamp_ms: f64,
    duration_ms: f64,
    tags: Value,
    data: Value,
}

/// Assemble a distributed trace waterfall across one or more service/project segments (`F-PRF-4`, `F-PRF-5`).
///
/// Decompresses each segment's zstd span blob, links child spans and downstream service transactions
/// via `parent_span_id`, orders spans in DFS tree order, computes offsets & self durations, and
/// flags the slowest operations for both UI highlighting and MCP `get_trace`.
pub fn assemble_trace_waterfall(
    trace_id: String,
    segments: Vec<StoredTraceSegment>,
    pii_rules: &[PiiScrubRuleEntry],
) -> Result<TraceWaterfallView, AppError> {
    if segments.is_empty() {
        return Err(AppError::NotFound);
    }

    let mut nodes: Vec<RawNode> = Vec::new();
    let mut seen_span_ids: HashSet<String> = HashSet::new();
    let mut project_set: BTreeSet<String> = BTreeSet::new();
    let mut reason_set: BTreeSet<String> = BTreeSet::new();
    let mut any_error = false;

    for seg in segments {
        project_set.insert(seg.project_slug.clone());
        reason_set.insert(seg.sample_reason.clone());
        any_error |= seg.is_error;

        let blob = decompress_trace_blob(&seg.spans_zstd)?;
        let root_start_ms = timestamp_to_ms(seg.start_timestamp);
        let root_end_ms = timestamp_to_ms(seg.end_timestamp);
        let root_dur = seg.duration_ms.max((root_end_ms - root_start_ms).max(0.0));

        let mut root_tags = seg.tags;
        let mut root_data = json!({
            "request": blob.request,
            "measurements": blob.measurements,
            "contexts": blob.contexts,
        });
        if !pii_rules.is_empty() {
            scrub_event_payload_with_rules(&mut root_tags, pii_rules);
            scrub_event_payload_with_rules(&mut root_data, pii_rules);
        }
        let root_desc = if pii_rules.is_empty() {
            seg.transaction_name
        } else {
            scrub_text_with_rules(&seg.transaction_name, pii_rules)
        };

        if seen_span_ids.insert(seg.root_span_id.clone()) {
            nodes.push(RawNode {
                span_id: seg.root_span_id.clone(),
                parent_span_id: seg.parent_span_id.clone(),
                project_id: seg.project_id,
                project_slug: seg.project_slug.clone(),
                platform: seg.platform.clone(),
                environment: seg.environment.clone(),
                release: seg.release.clone(),
                op: seg.op.clone(),
                description: root_desc,
                status: seg.status.clone(),
                is_error: seg.is_error,
                is_transaction_root: true,
                start_timestamp_ms: root_start_ms,
                end_timestamp_ms: root_end_ms.max(root_start_ms + root_dur),
                duration_ms: root_dur,
                tags: root_tags,
                data: root_data,
            });
        }

        for mut span in blob.spans {
            if !seen_span_ids.insert(span.span_id.clone()) {
                continue;
            }
            if !pii_rules.is_empty() {
                scrub_event_payload_with_rules(&mut span.tags, pii_rules);
                scrub_event_payload_with_rules(&mut span.data, pii_rules);
                span.description = scrub_text_with_rules(&span.description, pii_rules);
            }
            any_error |= span.is_error;
            nodes.push(RawNode {
                span_id: span.span_id,
                parent_span_id: span
                    .parent_span_id
                    .or_else(|| Some(seg.root_span_id.clone())),
                project_id: seg.project_id,
                project_slug: seg.project_slug.clone(),
                platform: seg.platform.clone(),
                environment: seg.environment.clone(),
                release: seg.release.clone(),
                op: span.op,
                description: span.description,
                status: span.status,
                is_error: span.is_error,
                is_transaction_root: false,
                start_timestamp_ms: span.start_timestamp_ms,
                end_timestamp_ms: span.end_timestamp_ms,
                duration_ms: span.duration_ms,
                tags: span.tags,
                data: span.data,
            });
        }
    }

    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.span_id.clone(), i))
        .collect();

    let mut children_map: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (idx, node) in nodes.iter().enumerate() {
        let parent_idx = node
            .parent_span_id
            .as_deref()
            .and_then(|pid| id_to_idx.get(pid).copied())
            .filter(|&p| p != idx);
        match parent_idx {
            Some(p) => children_map.entry(p).or_default().push(idx),
            None => roots.push(idx),
        }
    }

    // Sort roots and each children list chronologically by start timestamp (tx roots first on tie).
    let sort_by_time = |list: &mut Vec<usize>| {
        list.sort_by(|&a, &b| {
            nodes[a]
                .start_timestamp_ms
                .total_cmp(&nodes[b].start_timestamp_ms)
                .then_with(|| {
                    nodes[b]
                        .is_transaction_root
                        .cmp(&nodes[a].is_transaction_root)
                })
                .then_with(|| nodes[a].span_id.cmp(&nodes[b].span_id))
        });
    };
    sort_by_time(&mut roots);
    for children in children_map.values_mut() {
        sort_by_time(children);
    }

    let trace_start_ms = nodes
        .iter()
        .map(|n| n.start_timestamp_ms)
        .fold(f64::INFINITY, f64::min);
    let trace_end_ms = nodes
        .iter()
        .map(|n| n.end_timestamp_ms)
        .fold(f64::NEG_INFINITY, f64::max);
    let total_duration_ms = (trace_end_ms - trace_start_ms).max(0.0);

    // Precompute self-duration for every node by subtracting merged child time intervals.
    let mut self_durations = vec![0.0f64; nodes.len()];
    for (idx, node) in nodes.iter().enumerate() {
        let child_covered = children_map
            .get(&idx)
            .map(|kids| {
                let mut intervals: Vec<(f64, f64)> = kids
                    .iter()
                    .map(|&k| {
                        let s = nodes[k].start_timestamp_ms.max(node.start_timestamp_ms);
                        let e = nodes[k].end_timestamp_ms.min(node.end_timestamp_ms);
                        (s, e.max(s))
                    })
                    .collect();
                merged_interval_length(&mut intervals)
            })
            .unwrap_or(0.0);
        self_durations[idx] = (node.duration_ms - child_covered).max(0.0);
    }

    // Flatten in DFS order with cycle protection.
    let mut ordered: Vec<(usize, u32)> = Vec::with_capacity(nodes.len());
    let mut visited = vec![false; nodes.len()];

    fn dfs(
        idx: usize,
        depth: u32,
        children_map: &BTreeMap<usize, Vec<usize>>,
        visited: &mut [bool],
        ordered: &mut Vec<(usize, u32)>,
    ) {
        if visited[idx] {
            return;
        }
        visited[idx] = true;
        ordered.push((idx, depth));
        if let Some(kids) = children_map.get(&idx) {
            for &child in kids {
                dfs(
                    child,
                    depth.saturating_add(1),
                    children_map,
                    visited,
                    ordered,
                );
            }
        }
    }

    for &root in &roots {
        dfs(root, 0, &children_map, &mut visited, &mut ordered);
    }
    // Include any cyclic orphans so no span is ever silently dropped.
    for idx in 0..nodes.len() {
        if !visited[idx] {
            dfs(idx, 0, &children_map, &mut visited, &mut ordered);
        }
    }

    // Identify slow spans: prioritize leaf / non-root spans (or self-time bottlenecks).
    let mut slow_candidates: Vec<usize> = ordered
        .iter()
        .map(|&(idx, _)| idx)
        .filter(|&idx| !nodes[idx].is_transaction_root || nodes.len() == 1)
        .collect();
    if slow_candidates.is_empty() {
        slow_candidates = ordered.iter().map(|&(idx, _)| idx).collect();
    }
    slow_candidates.sort_by(|&a, &b| {
        nodes[b]
            .duration_ms
            .total_cmp(&nodes[a].duration_ms)
            .then_with(|| self_durations[b].total_cmp(&self_durations[a]))
    });

    let mut slow_ids: HashSet<String> = HashSet::new();
    let mut slowest_spans: Vec<SlowSpanHighlight> = Vec::new();
    for (rank, &idx) in slow_candidates.iter().take(5).enumerate() {
        let node = &nodes[idx];
        let pct = if total_duration_ms > 0.0 {
            (node.duration_ms / total_duration_ms) * 100.0
        } else {
            0.0
        };
        if rank == 0 || pct >= 20.0 || node.duration_ms >= 100.0 {
            if rank == 0 || pct >= 25.0 {
                slow_ids.insert(node.span_id.clone());
            }
            slowest_spans.push(SlowSpanHighlight {
                span_id: node.span_id.clone(),
                project_slug: node.project_slug.clone(),
                op: node.op.clone(),
                description: node.description.clone(),
                duration_ms: round_two_decimals(node.duration_ms),
                self_duration_ms: round_two_decimals(self_durations[idx]),
                pct_of_trace: round_two_decimals(pct),
                is_error: node.is_error,
            });
        }
    }

    let root_node = ordered.first().map(|&(idx, _)| &nodes[idx]);
    let root_transaction = root_node
        .map(|n| n.description.clone())
        .unwrap_or_else(|| "<trace>".to_owned());
    let root_status = if any_error {
        ordered
            .iter()
            .find_map(|&(idx, _)| nodes[idx].is_error.then(|| nodes[idx].status.clone()))
            .unwrap_or_else(|| "internal_error".to_owned())
    } else {
        root_node
            .map(|n| n.status.clone())
            .unwrap_or_else(|| "ok".to_owned())
    };

    let spans: Vec<WaterfallSpanNode> = ordered
        .into_iter()
        .map(|(idx, depth)| {
            let n = &nodes[idx];
            let offset_ms = (n.start_timestamp_ms - trace_start_ms).max(0.0);
            let pct = if total_duration_ms > 0.0 {
                ((n.duration_ms / total_duration_ms) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
            WaterfallSpanNode {
                span_id: n.span_id.clone(),
                parent_span_id: n.parent_span_id.clone(),
                project_id: n.project_id,
                project_slug: n.project_slug.clone(),
                platform: n.platform.clone(),
                environment: n.environment.clone(),
                release: n.release.clone(),
                op: n.op.clone(),
                description: n.description.clone(),
                status: n.status.clone(),
                is_error: n.is_error,
                is_transaction_root: n.is_transaction_root,
                depth,
                start_timestamp_ms: round_two_decimals(n.start_timestamp_ms),
                end_timestamp_ms: round_two_decimals(n.end_timestamp_ms),
                offset_ms: round_two_decimals(offset_ms),
                duration_ms: round_two_decimals(n.duration_ms),
                self_duration_ms: round_two_decimals(self_durations[idx]),
                pct_of_trace: round_two_decimals(pct),
                is_slow: slow_ids.contains(&n.span_id),
                tags: n.tags.clone(),
                data: n.data.clone(),
            }
        })
        .collect();

    let projects: Vec<String> = project_set.into_iter().collect();
    let services_count = projects.len();

    Ok(TraceWaterfallView {
        trace_id,
        root_transaction,
        status: root_status,
        is_error: any_error,
        projects,
        services_count,
        sample_reasons: reason_set.into_iter().collect(),
        start_timestamp: ms_to_datetime(trace_start_ms),
        end_timestamp: ms_to_datetime(trace_end_ms),
        total_duration_ms: round_two_decimals(total_duration_ms),
        span_count: spans.len(),
        slowest_spans,
        spans,
    })
}

fn merged_interval_length(intervals: &mut [(f64, f64)]) -> f64 {
    if intervals.is_empty() {
        return 0.0;
    }
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    let (mut cur_s, mut cur_e) = intervals[0];
    for &(s, e) in &intervals[1..] {
        if s <= cur_e {
            cur_e = cur_e.max(e);
        } else {
            total += (cur_e - cur_s).max(0.0);
            cur_s = s;
            cur_e = e;
        }
    }
    total += (cur_e - cur_s).max(0.0);
    total
}

fn ms_to_datetime(ms: f64) -> DateTime<Utc> {
    if !ms.is_finite() {
        return Utc::now();
    }
    let micros = (ms * 1000.0).round() as i64;
    DateTime::<Utc>::from_timestamp_micros(micros).unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::codec::compress_trace_blob;
    use crate::model::{TraceBlobPayload, TraceSpanRecord};

    #[test]
    fn assembles_distributed_cross_service_waterfall_and_marks_slowest_span() {
        let t0 = Utc.with_ymd_and_hms(2026, 11, 22, 10, 0, 0).unwrap();
        let t0_ms = timestamp_to_ms(t0);

        // Segment 1: React frontend transaction (0..600ms) with http.client span (50..550ms)
        let frontend_blob = compress_trace_blob(&TraceBlobPayload {
            spans: vec![TraceSpanRecord {
                span_id: "front_http_span0".to_owned(),
                parent_span_id: Some("front_root_span0".to_owned()),
                op: "http.client".to_owned(),
                description: "GET /api/checkout".to_owned(),
                status: "ok".to_owned(),
                is_error: false,
                start_timestamp_ms: t0_ms + 50.0,
                end_timestamp_ms: t0_ms + 550.0,
                duration_ms: 500.0,
                tags: json!({}),
                data: json!({}),
            }],
            request: None,
            measurements: None,
            contexts: None,
        })
        .unwrap();

        // Segment 2: FastAPI backend transaction (70..530ms), parent_span_id = front_http_span0,
        // with a slow PostgreSQL span (100..490ms = 390ms)
        let backend_blob = compress_trace_blob(&TraceBlobPayload {
            spans: vec![TraceSpanRecord {
                span_id: "db_query_span000".to_owned(),
                parent_span_id: Some("api_root_span000".to_owned()),
                op: "db.sql.query".to_owned(),
                description: "SELECT * FROM invoices FOR UPDATE".to_owned(),
                status: "ok".to_owned(),
                is_error: false,
                start_timestamp_ms: t0_ms + 100.0,
                end_timestamp_ms: t0_ms + 490.0,
                duration_ms: 390.0,
                tags: json!({}),
                data: json!({}),
            }],
            request: None,
            measurements: None,
            contexts: None,
        })
        .unwrap();

        let seg_front = StoredTraceSegment {
            event_id: "ev1".to_owned(),
            project_id: 1,
            project_slug: "react-web".to_owned(),
            workspace_slug: "default".to_owned(),
            trace_id: "aabbccddeeff00112233445566778899".to_owned(),
            root_span_id: "front_root_span0".to_owned(),
            parent_span_id: None,
            transaction_name: "/checkout".to_owned(),
            op: "pageload".to_owned(),
            status: "ok".to_owned(),
            is_error: false,
            sample_reason: "slow".to_owned(),
            environment: "production".to_owned(),
            release: Some("web@1.0".to_owned()),
            platform: "javascript".to_owned(),
            start_timestamp: t0,
            end_timestamp: t0 + chrono::Duration::milliseconds(600),
            duration_ms: 600.0,
            tags: json!({}),
            spans_zstd: frontend_blob,
        };

        let seg_api = StoredTraceSegment {
            event_id: "ev2".to_owned(),
            project_id: 2,
            project_slug: "fastapi-core".to_owned(),
            workspace_slug: "default".to_owned(),
            trace_id: "aabbccddeeff00112233445566778899".to_owned(),
            root_span_id: "api_root_span000".to_owned(),
            parent_span_id: Some("front_http_span0".to_owned()),
            transaction_name: "GET /api/checkout".to_owned(),
            op: "http.server".to_owned(),
            status: "ok".to_owned(),
            is_error: false,
            sample_reason: "slow".to_owned(),
            environment: "production".to_owned(),
            release: Some("api@2.1".to_owned()),
            platform: "python".to_owned(),
            start_timestamp: t0 + chrono::Duration::milliseconds(70),
            end_timestamp: t0 + chrono::Duration::milliseconds(530),
            duration_ms: 460.0,
            tags: json!({}),
            spans_zstd: backend_blob,
        };

        let wf = assemble_trace_waterfall(
            "aabbccddeeff00112233445566778899".to_owned(),
            vec![seg_api, seg_front],
            &[],
        )
        .expect("waterfall ok");

        assert_eq!(wf.services_count, 2);
        assert_eq!(wf.span_count, 4);
        assert!((wf.total_duration_ms - 600.0).abs() < 1.0);
        // Hierarchy: react root (0) -> front_http_span0 (1) -> api_root_span000 (2) -> db_query_span000 (3)
        assert_eq!(wf.spans[0].span_id, "front_root_span0");
        assert_eq!(wf.spans[0].depth, 0);
        assert_eq!(wf.spans[1].span_id, "front_http_span0");
        assert_eq!(wf.spans[1].depth, 1);
        assert_eq!(wf.spans[2].span_id, "api_root_span000");
        assert_eq!(wf.spans[2].depth, 2);
        assert_eq!(wf.spans[3].span_id, "db_query_span000");
        assert_eq!(wf.spans[3].depth, 3);
        assert!(wf.spans[3].is_slow);
        assert!(!wf.slowest_spans.is_empty());
    }
}
