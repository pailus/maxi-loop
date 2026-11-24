//! Turn performance problems detected in a transaction (`F017`) into issue events so they reuse
//! the regular issue pipeline: grouping, regression, releases, hourly counters and alerts.

use maxi_loop_performance::model::{PerformanceProblem, ProcessedTransaction};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::fingerprint::ProcessedEvent;
use crate::user_identity::UserDigest;

const MAX_TITLE_DESC_CHARS: usize = 200;
const MAX_EXCEPTION_VALUE_CHARS: usize = 1024;
const MAX_CULPRIT_CHARS: usize = 256;
/// Performance problems are not crashes; Sentry also reports them below `error`.
const PERFORMANCE_ISSUE_LEVEL: &str = "warning";

/// Build one issue event per detected problem. The transaction payload was already PII-scrubbed,
/// and problems only carry normalized (parameter-stripped) span descriptions.
/// `user_digest` is the transaction's user (derived from the raw payload before scrubbing) so
/// performance issues also count affected users (`F018`).
pub fn performance_problem_events(
    tx: &ProcessedTransaction,
    user_digest: Option<UserDigest>,
) -> Vec<ProcessedEvent> {
    tx.performance_problems
        .iter()
        .map(|problem| problem_to_event(tx, problem, user_digest))
        .collect()
}

fn problem_to_event(
    tx: &ProcessedTransaction,
    problem: &PerformanceProblem,
    user_digest: Option<UserDigest>,
) -> ProcessedEvent {
    let kind = problem.problem_type;
    // Same pattern in the same endpoint = same issue, independent of environment or release.
    let fingerprint = sha256_hex(&format!(
        "perf:{}:{}:{}",
        kind.issue_type(),
        tx.transaction_name,
        problem.group_hash
    ));
    let event_id = sha256_hex(&format!("{}:{fingerprint}", tx.event_id))[..32].to_owned();
    let title = format!(
        "{}: {}",
        kind.title(),
        truncate_chars(
            &collapse_select_columns(&problem.description),
            MAX_TITLE_DESC_CHARS
        )
    );

    let payload = json!({
        "event_id": event_id,
        "type": "performance",
        "level": PERFORMANCE_ISSUE_LEVEL,
        "platform": tx.platform,
        "environment": tx.environment,
        "release": tx.release,
        "transaction": tx.transaction_name,
        "tags": tx.tags,
        "contexts": {
            "trace": {
                "trace_id": tx.trace_id,
                "span_id": tx.root_span_id,
                "op": tx.op,
            }
        },
        "performance_problem": {
            "issue_type": kind.issue_type(),
            "title": kind.title(),
            "transaction_event_id": tx.event_id,
            "transaction_duration_ms": tx.duration_ms,
            "trace_id": tx.trace_id,
            "problem": problem,
        },
    });

    ProcessedEvent {
        tenant_id: tx.tenant_id,
        workspace_id: tx.workspace_id,
        project_id: tx.project_id,
        event_id,
        received_at: tx.received_at,
        fingerprint,
        title,
        exception_type: kind.title().to_owned(),
        exception_value: truncate_chars(&problem.description, MAX_EXCEPTION_VALUE_CHARS),
        culprit: truncate_chars(&tx.transaction_name, MAX_CULPRIT_CHARS),
        level: PERFORMANCE_ISSUE_LEVEL.to_owned(),
        environment: tx.environment.clone(),
        release: tx.release.clone(),
        platform: tx.platform.clone(),
        stack_frames: Value::Array(Vec::new()),
        tags: tx.tags.clone(),
        payload,
        issue_type: kind.issue_type().to_owned(),
        user_digest,
    }
}

fn sha256_hex(seed: &str) -> String {
    hex::encode(Sha256::digest(seed.as_bytes()))
}

/// Sentry-style short form for titles: ORM queries list every column, which pushes the table
/// and `WHERE` clause past the title length. `SELECT a, b, c FROM t` becomes `SELECT .. FROM t`;
/// single-column / `*` selects are kept as is.
fn collapse_select_columns(desc: &str) -> String {
    const SELECT: &str = "SELECT ";
    const FROM: &str = " FROM ";
    let has_select = desc
        .get(..SELECT.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(SELECT));
    if !has_select {
        return desc.to_owned();
    }
    // ASCII uppercasing keeps byte offsets identical, so indexes map back onto `desc`.
    let Some(from_idx) = desc.to_ascii_uppercase().find(FROM) else {
        return desc.to_owned();
    };
    if from_idx < SELECT.len() || !desc[SELECT.len()..from_idx].contains(',') {
        return desc.to_owned();
    }
    format!(
        "{}.. FROM {}",
        &desc[..SELECT.len()],
        &desc[from_idx + FROM.len()..]
    )
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use maxi_loop_performance::model::{OffendingSpan, PerformanceProblemType};

    use super::*;

    fn tx_with(
        name: &str,
        event_id: &str,
        problems: Vec<PerformanceProblem>,
    ) -> ProcessedTransaction {
        ProcessedTransaction {
            tenant_id: 1,
            workspace_id: 2,
            project_id: 3,
            event_id: event_id.to_owned(),
            received_at: Utc::now(),
            trace_id: "a".repeat(32),
            root_span_id: "b".repeat(16),
            parent_span_id: None,
            transaction_name: name.to_owned(),
            op: "http.server".to_owned(),
            status: "ok".to_owned(),
            is_error: false,
            environment: "production".to_owned(),
            release: Some("1.0".to_owned()),
            platform: "python".to_owned(),
            start_timestamp: Utc::now(),
            end_timestamp: Utc::now(),
            duration_ms: 120.0,
            span_count: 21,
            tags: json!({}),
            sample_reason: None,
            spans_zstd: None,
            span_ops: Vec::new(),
            performance_problems: problems,
        }
    }

    fn n_plus_one() -> PerformanceProblem {
        PerformanceProblem {
            problem_type: PerformanceProblemType::NPlusOneDb,
            group_hash: "h1".to_owned(),
            description: "SELECT * FROM authors WHERE id = ?".to_owned(),
            offending_span_count: 20,
            offending_spans_total_ms: 80.0,
            potential_savings_ms: None,
            offending_spans: vec![OffendingSpan {
                span_id: "c".repeat(16),
                op: "db".to_owned(),
                description: "SELECT * FROM authors WHERE id = ?".to_owned(),
                start_offset_ms: 5.0,
                duration_ms: 4.0,
            }],
        }
    }

    #[test]
    fn builds_performance_issue_event_with_type_title_and_trace_link() {
        let events =
            performance_problem_events(&tx_with("/books/", "e1", vec![n_plus_one()]), None);
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.issue_type, "performance_n_plus_one_db");
        assert_eq!(ev.title, "N+1 Query: SELECT * FROM authors WHERE id = ?");
        assert_eq!(ev.culprit, "/books/");
        assert_eq!(ev.level, "warning");
        assert_eq!(ev.event_id.len(), 32);
        assert_eq!(
            ev.payload["performance_problem"]["trace_id"],
            json!("a".repeat(32))
        );
        assert_eq!(
            ev.payload["performance_problem"]["problem"]["offending_span_count"],
            json!(20)
        );
    }

    #[test]
    fn same_pattern_and_endpoint_groups_together_but_other_endpoints_do_not() {
        let a = &performance_problem_events(&tx_with("/books/", "e1", vec![n_plus_one()]), None)[0];
        let b = &performance_problem_events(&tx_with("/books/", "e2", vec![n_plus_one()]), None)[0];
        let c =
            &performance_problem_events(&tx_with("/authors/", "e3", vec![n_plus_one()]), None)[0];
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_ne!(a.event_id, b.event_id);
        assert_ne!(a.fingerprint, c.fingerprint);
    }

    #[test]
    fn title_collapses_orm_column_list_so_table_and_where_stay_visible() {
        let columns: Vec<String> = (0..40)
            .map(|i| format!("\"pegawai_pegawai\".\"column_{i:02}\""))
            .collect();
        let mut problem = n_plus_one();
        problem.description = format!(
            "SELECT {} FROM \"pegawai_pegawai\" WHERE \"pegawai_pegawai\".\"id\" = ? LIMIT ?",
            columns.join(", ")
        );
        problem.group_hash = "h2".to_owned();
        let with_columns =
            &performance_problem_events(&tx_with("/keluarga/", "e1", vec![problem.clone()]), None)
                [0];
        assert_eq!(
            with_columns.title,
            "N+1 Query: SELECT .. FROM \"pegawai_pegawai\" WHERE \"pegawai_pegawai\".\"id\" = ? LIMIT ?"
        );

        // The title is display-only: grouping still uses the full pattern hash.
        let plain =
            &performance_problem_events(&tx_with("/keluarga/", "e2", vec![n_plus_one()]), None)[0];
        assert_ne!(with_columns.fingerprint, plain.fingerprint);
    }
}
