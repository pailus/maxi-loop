//! Performance issue detectors (`F017`).
//!
//! Sentry SDKs only send transactions + spans; detecting N+1 queries and similar patterns is a
//! server-side job. These detectors run once per ingested transaction (before sampling), over at
//! most `MAX_SPANS_PER_TRANSACTION` spans, in O(n log n). Thresholds follow Sentry's published
//! defaults closely so results match what users expect from Sentry.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::model::{OffendingSpan, PerformanceProblem, PerformanceProblemType};
use crate::normalize::is_db_op;

/// Repeated identical (normalized) DB queries within one transaction.
const N_PLUS_ONE_DB_MIN_COUNT: usize = 5;
const N_PLUS_ONE_DB_MIN_TOTAL_MS: f64 = 50.0;
/// Repeated identical (normalized) outbound HTTP calls within one transaction.
const N_PLUS_ONE_API_MIN_COUNT: usize = 5;
const N_PLUS_ONE_API_MIN_TOTAL_MS: f64 = 100.0;
/// A single DB query slower than this is a "Slow DB Query".
const SLOW_DB_QUERY_MIN_MS: f64 = 1000.0;
/// Sequential DB queries that could have run in parallel.
const CONSECUTIVE_DB_MIN_SPAN_MS: f64 = 30.0;
const CONSECUTIVE_DB_MIN_SAVINGS_MS: f64 = 100.0;
/// Sequential outbound HTTP calls that could have run in parallel.
const CONSECUTIVE_HTTP_MIN_COUNT: usize = 3;
const CONSECUTIVE_HTTP_MIN_SPAN_MS: f64 = 500.0;
const CONSECUTIVE_HTTP_MIN_SAVINGS_MS: f64 = 1000.0;
/// Spans further apart than this are not considered "consecutive".
const CONSECUTIVE_MAX_GAP_MS: f64 = 1000.0;
/// Clock jitter allowed when deciding whether two spans overlap.
const OVERLAP_TOLERANCE_MS: f64 = 1.0;

pub const MAX_PROBLEMS_PER_TRANSACTION: usize = 10;
pub const MAX_OFFENDING_SPANS: usize = 20;

/// Minimal span view the detectors need (borrowed from the parsed transaction).
#[derive(Clone, Copy, Debug)]
pub struct DetectorSpan<'a> {
    pub span_id: &'a str,
    pub parent_span_id: Option<&'a str>,
    pub op: &'a str,
    /// Normalized description and its hash; `None` when the span has no description.
    pub normalized: Option<(&'a str, &'a str)>,
    pub start_ms: f64,
    pub end_ms: f64,
    pub duration_ms: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SpanKind {
    DbQuery,
    HttpClient,
    Other,
}

fn span_kind(span: &DetectorSpan<'_>) -> SpanKind {
    let Some((desc, _)) = span.normalized else {
        return SpanKind::Other;
    };
    let op = span.op.trim().to_ascii_lowercase();
    if is_db_op(&op) && !op.starts_with("db.redis") && !is_transaction_control(desc) {
        SpanKind::DbQuery
    } else if op.starts_with("http.client") {
        SpanKind::HttpClient
    } else {
        SpanKind::Other
    }
}

/// `BEGIN` / `COMMIT` / `SAVEPOINT` etc. are not queries a developer can batch or parallelize.
fn is_transaction_control(desc: &str) -> bool {
    let head: String = desc
        .trim_start()
        .chars()
        .take(20)
        .collect::<String>()
        .to_ascii_uppercase();
    [
        "BEGIN",
        "COMMIT",
        "ROLLBACK",
        "SAVEPOINT",
        "RELEASE",
        "SET ",
        "START TRANSACTION",
    ]
    .iter()
    .any(|kw| head.starts_with(kw))
}

/// Run every detector over one transaction's spans. `root_start_ms` is used for span offsets.
pub fn detect_performance_problems(
    spans: &[DetectorSpan<'_>],
    root_start_ms: f64,
) -> Vec<PerformanceProblem> {
    let mut problems = Vec::new();
    let mut claimed: BTreeSet<usize> = BTreeSet::new();

    detect_repeated(
        spans,
        root_start_ms,
        SpanKind::DbQuery,
        PerformanceProblemType::NPlusOneDb,
        N_PLUS_ONE_DB_MIN_COUNT,
        N_PLUS_ONE_DB_MIN_TOTAL_MS,
        &mut claimed,
        &mut problems,
    );
    detect_repeated(
        spans,
        root_start_ms,
        SpanKind::HttpClient,
        PerformanceProblemType::NPlusOneApi,
        N_PLUS_ONE_API_MIN_COUNT,
        N_PLUS_ONE_API_MIN_TOTAL_MS,
        &mut claimed,
        &mut problems,
    );
    detect_slow_db(spans, root_start_ms, &mut problems);
    detect_consecutive(
        spans,
        root_start_ms,
        &claimed,
        ConsecutiveRule {
            kind: SpanKind::DbQuery,
            problem_type: PerformanceProblemType::ConsecutiveDb,
            min_count: 2,
            min_span_ms: CONSECUTIVE_DB_MIN_SPAN_MS,
            min_savings_ms: CONSECUTIVE_DB_MIN_SAVINGS_MS,
        },
        &mut problems,
    );
    detect_consecutive(
        spans,
        root_start_ms,
        &claimed,
        ConsecutiveRule {
            kind: SpanKind::HttpClient,
            problem_type: PerformanceProblemType::ConsecutiveHttp,
            min_count: CONSECUTIVE_HTTP_MIN_COUNT,
            min_span_ms: CONSECUTIVE_HTTP_MIN_SPAN_MS,
            min_savings_ms: CONSECUTIVE_HTTP_MIN_SAVINGS_MS,
        },
        &mut problems,
    );

    problems.truncate(MAX_PROBLEMS_PER_TRANSACTION);
    problems
}

/// N+1: the same normalized query / request executed many times in one transaction.
/// Indices of matched spans are added to `claimed` so consecutive detectors skip them.
#[allow(clippy::too_many_arguments)] // private helper; a params struct would only add noise
fn detect_repeated(
    spans: &[DetectorSpan<'_>],
    root_start_ms: f64,
    kind: SpanKind,
    problem_type: PerformanceProblemType,
    min_count: usize,
    min_total_ms: f64,
    claimed: &mut BTreeSet<usize>,
    out: &mut Vec<PerformanceProblem>,
) {
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (idx, span) in spans.iter().enumerate() {
        if span_kind(span) == kind
            && let Some((_, hash)) = span.normalized
        {
            groups.entry(hash).or_default().push(idx);
        }
    }

    for (hash, idxs) in groups {
        let total_ms: f64 = idxs.iter().map(|&i| spans[i].duration_ms).sum();
        if idxs.len() < min_count || total_ms < min_total_ms {
            continue;
        }
        let description = spans[idxs[0]].normalized.map(|(d, _)| d).unwrap_or("");
        out.push(build_problem(
            problem_type,
            hash.to_owned(),
            description.to_owned(),
            &idxs,
            spans,
            root_start_ms,
            None,
        ));
        claimed.extend(idxs);
    }
}

fn detect_slow_db(
    spans: &[DetectorSpan<'_>],
    root_start_ms: f64,
    out: &mut Vec<PerformanceProblem>,
) {
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (idx, span) in spans.iter().enumerate() {
        if span_kind(span) == SpanKind::DbQuery
            && span.duration_ms >= SLOW_DB_QUERY_MIN_MS
            && let Some((_, hash)) = span.normalized
        {
            groups.entry(hash).or_default().push(idx);
        }
    }
    for (hash, idxs) in groups {
        let description = spans[idxs[0]].normalized.map(|(d, _)| d).unwrap_or("");
        out.push(build_problem(
            PerformanceProblemType::SlowDbQuery,
            hash.to_owned(),
            description.to_owned(),
            &idxs,
            spans,
            root_start_ms,
            None,
        ));
    }
}

struct ConsecutiveRule {
    kind: SpanKind,
    problem_type: PerformanceProblemType,
    min_count: usize,
    min_span_ms: f64,
    min_savings_ms: f64,
}

/// Runs of non-overlapping sibling spans (same parent) that each take long enough that running
/// them in parallel would save at least `min_savings_ms` (`sum - max`).
fn detect_consecutive(
    spans: &[DetectorSpan<'_>],
    root_start_ms: f64,
    claimed: &BTreeSet<usize>,
    rule: ConsecutiveRule,
    out: &mut Vec<PerformanceProblem>,
) {
    let mut by_parent: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (idx, span) in spans.iter().enumerate() {
        if span_kind(span) == rule.kind && !claimed.contains(&idx) {
            by_parent
                .entry(span.parent_span_id.unwrap_or(""))
                .or_default()
                .push(idx);
        }
    }

    for mut idxs in by_parent.into_values() {
        idxs.sort_by(|&a, &b| spans[a].start_ms.total_cmp(&spans[b].start_ms));
        let mut run: Vec<usize> = Vec::new();
        for idx in idxs {
            let span = &spans[idx];
            let continues_run = run.last().is_some_and(|&prev| {
                let prev_end = spans[prev].end_ms;
                span.start_ms >= prev_end - OVERLAP_TOLERANCE_MS
                    && span.start_ms - prev_end <= CONSECUTIVE_MAX_GAP_MS
            });
            if span.duration_ms < rule.min_span_ms {
                flush_consecutive_run(spans, root_start_ms, &rule, &mut run, out);
                continue;
            }
            if !continues_run {
                flush_consecutive_run(spans, root_start_ms, &rule, &mut run, out);
            }
            run.push(idx);
        }
        flush_consecutive_run(spans, root_start_ms, &rule, &mut run, out);
    }
}

fn flush_consecutive_run(
    spans: &[DetectorSpan<'_>],
    root_start_ms: f64,
    rule: &ConsecutiveRule,
    run: &mut Vec<usize>,
    out: &mut Vec<PerformanceProblem>,
) {
    let taken = std::mem::take(run);
    if taken.len() < rule.min_count {
        return;
    }
    let total: f64 = taken.iter().map(|&i| spans[i].duration_ms).sum();
    let longest = taken
        .iter()
        .map(|&i| spans[i].duration_ms)
        .fold(0.0_f64, f64::max);
    let savings = total - longest;
    if savings < rule.min_savings_ms {
        return;
    }

    // Group by the ordered set of distinct query shapes so the same code path always maps to
    // the same issue, regardless of how many times a query repeats inside the run.
    let mut shapes: Vec<(&str, &str)> = Vec::new();
    for &i in &taken {
        if let Some(norm) = spans[i].normalized
            && !shapes.iter().any(|(_, h)| *h == norm.1)
        {
            shapes.push(norm);
        }
    }
    let mut hasher = Sha256::new();
    for (_, hash) in &shapes {
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    let group_hash = hex::encode(hasher.finalize());
    let description = shapes
        .iter()
        .map(|(d, _)| *d)
        .collect::<Vec<_>>()
        .join(" ; ");

    out.push(build_problem(
        rule.problem_type,
        group_hash,
        description,
        &taken,
        spans,
        root_start_ms,
        Some(round_ms(savings)),
    ));
}

fn build_problem(
    problem_type: PerformanceProblemType,
    group_hash: String,
    description: String,
    idxs: &[usize],
    spans: &[DetectorSpan<'_>],
    root_start_ms: f64,
    potential_savings_ms: Option<f64>,
) -> PerformanceProblem {
    let total_ms: f64 = idxs.iter().map(|&i| spans[i].duration_ms).sum();
    let offending_spans = idxs
        .iter()
        .take(MAX_OFFENDING_SPANS)
        .map(|&i| {
            let s = &spans[i];
            OffendingSpan {
                span_id: s.span_id.to_owned(),
                op: s.op.to_owned(),
                description: s.normalized.map(|(d, _)| d).unwrap_or("").to_owned(),
                start_offset_ms: round_ms((s.start_ms - root_start_ms).max(0.0)),
                duration_ms: round_ms(s.duration_ms),
            }
        })
        .collect();
    PerformanceProblem {
        problem_type,
        group_hash,
        description,
        offending_span_count: idxs.len(),
        offending_spans_total_ms: round_ms(total_ms),
        potential_savings_ms,
        offending_spans,
    }
}

fn round_ms(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        id: String,
        parent: Option<String>,
        op: String,
        desc: String,
        hash: String,
        start: f64,
        dur: f64,
    }

    fn span(id: &str, op: &str, desc: &str, start: f64, dur: f64) -> Fixture {
        Fixture {
            id: id.to_owned(),
            parent: Some("root".to_owned()),
            op: op.to_owned(),
            desc: desc.to_owned(),
            hash: format!("h:{desc}"),
            start,
            dur,
        }
    }

    fn detect(fixtures: &[Fixture]) -> Vec<PerformanceProblem> {
        let spans: Vec<DetectorSpan<'_>> = fixtures
            .iter()
            .map(|f| DetectorSpan {
                span_id: &f.id,
                parent_span_id: f.parent.as_deref(),
                op: &f.op,
                normalized: if f.desc.is_empty() {
                    None
                } else {
                    Some((f.desc.as_str(), f.hash.as_str()))
                },
                start_ms: f.start,
                end_ms: f.start + f.dur,
                duration_ms: f.dur,
            })
            .collect();
        detect_performance_problems(&spans, 0.0)
    }

    fn types(problems: &[PerformanceProblem]) -> Vec<PerformanceProblemType> {
        problems.iter().map(|p| p.problem_type).collect()
    }

    fn n_queries(op: &str, desc: &str, n: usize, dur: f64) -> Vec<Fixture> {
        (0..n)
            .map(|i| span(&format!("s{i}"), op, desc, i as f64 * (dur + 1.0), dur))
            .collect()
    }

    #[test]
    fn detects_n_plus_one_db_and_claims_spans_from_consecutive_detector() {
        let mut spans = vec![span("src", "db", "SELECT * FROM books", 0.0, 5.0)];
        spans.extend(n_queries(
            "db",
            "SELECT * FROM authors WHERE id = ?",
            20,
            40.0,
        ));
        let problems = detect(&spans);
        assert_eq!(types(&problems), vec![PerformanceProblemType::NPlusOneDb]);
        assert_eq!(problems[0].offending_span_count, 20);
        assert_eq!(problems[0].offending_spans.len(), MAX_OFFENDING_SPANS);
        assert_eq!(
            problems[0].description,
            "SELECT * FROM authors WHERE id = ?"
        );
    }

    #[test]
    fn ignores_repeated_queries_below_count_or_duration_threshold() {
        // 4 repeats is below the N+1 count (it is still reported as consecutive queries).
        let four = detect(&n_queries("db", "SELECT 1", 4, 50.0));
        assert!(!types(&four).contains(&PerformanceProblemType::NPlusOneDb));
        // 20 queries but only 20ms in total: cheap enough that Sentry would not flag it either.
        assert!(detect(&n_queries("db", "SELECT 1", 20, 1.0)).is_empty());
    }

    #[test]
    fn ignores_transaction_control_statements() {
        assert!(detect(&n_queries("db", "SAVEPOINT ?", 10, 20.0)).is_empty());
    }

    #[test]
    fn detects_n_plus_one_api_calls() {
        let problems = detect(&n_queries("http.client", "GET /items/:id", 8, 50.0));
        assert_eq!(types(&problems), vec![PerformanceProblemType::NPlusOneApi]);
    }

    #[test]
    fn detects_slow_db_query_grouped_by_query_shape() {
        let problems = detect(&[
            span("a", "db.sql.query", "SELECT * FROM big", 0.0, 1500.0),
            span("b", "db.sql.query", "SELECT * FROM big", 2000.0, 1200.0),
            span("c", "db.sql.query", "SELECT 1", 3300.0, 10.0),
        ]);
        let slow: Vec<_> = problems
            .iter()
            .filter(|p| p.problem_type == PerformanceProblemType::SlowDbQuery)
            .collect();
        assert_eq!(slow.len(), 1);
        assert_eq!(slow[0].offending_span_count, 2);
    }

    #[test]
    fn detects_consecutive_db_queries_with_savings() {
        let problems = detect(&[
            span("a", "db", "SELECT * FROM users", 0.0, 80.0),
            span("b", "db", "SELECT * FROM orders", 81.0, 90.0),
            span("c", "db", "SELECT * FROM carts", 172.0, 70.0),
        ]);
        assert_eq!(
            types(&problems),
            vec![PerformanceProblemType::ConsecutiveDb]
        );
        // sum 240 - longest 90 = 150ms that could be saved by running in parallel.
        assert_eq!(problems[0].potential_savings_ms, Some(150.0));
        assert_eq!(problems[0].offending_span_count, 3);
    }

    #[test]
    fn overlapping_db_queries_are_not_consecutive() {
        let problems = detect(&[
            span("a", "db", "SELECT * FROM users", 0.0, 80.0),
            span("b", "db", "SELECT * FROM orders", 10.0, 90.0),
            span("c", "db", "SELECT * FROM carts", 20.0, 70.0),
        ]);
        assert!(problems.is_empty());
    }

    #[test]
    fn consecutive_db_grouping_is_stable_across_repeat_counts() {
        let three = detect(&n_queries("db", "SELECT * FROM t", 3, 60.0));
        let four = detect(&n_queries("db", "SELECT * FROM t", 4, 60.0));
        assert_eq!(three[0].group_hash, four[0].group_hash);
    }

    #[test]
    fn detects_consecutive_http_calls() {
        let problems = detect(&[
            span("a", "http.client", "GET /a", 0.0, 600.0),
            span("b", "http.client", "GET /b", 601.0, 700.0),
            span("c", "http.client", "GET /c", 1302.0, 650.0),
        ]);
        assert_eq!(
            types(&problems),
            vec![PerformanceProblemType::ConsecutiveHttp]
        );
    }

    #[test]
    fn spans_without_description_and_other_ops_are_ignored() {
        let mut spans = n_queries("db", "", 10, 100.0);
        spans.extend(n_queries("template.render", "index.html", 10, 100.0));
        assert!(detect(&spans).is_empty());
    }
}
