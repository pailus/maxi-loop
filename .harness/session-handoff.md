# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: `F015` — P2 Issues: Manual issue merge/split (with fingerprint alias routing) and event breadcrumbs viewer — done

## Current state
- What works and is verified:
  - **`F015` (`F-ISS-9` manual merge/split + `F-ISS-10` event breadcrumbs viewer) is 100% implemented, tested, and live-verified.**
  - Migration `migrations/0008_p2_issue_merge_split.sql` adds `events.fingerprint` and RLS-protected `issue_fingerprints` so merged fingerprints route future incoming events into the target issue without duplicating issues, and splitting restores independent routing.
  - Endpoints `POST /api/v1/issues/merge` and `POST /api/v1/issues/{id}/split` (plus `/t/{tenant}/w/{workspace}/p/{project}/...` slug routes) enforce RBAC, API token write/project scope, same-project merge bounds, and Postgres RLS tenant isolation.
  - Normalized event breadcrumbs (`crates/issues/src/breadcrumbs.rs`) are returned on `IssueDetailView` and `IssueEventView` (already PII-scrubbed) and rendered in MaxiBoard UI (`BreadcrumbsView.tsx`, `IssueDetailDrawer.tsx`, `IssueDetailFullView.tsx`), alongside multi-select Merge bar in `IssuesView.tsx` and fingerprint/event Split controls in `IssueDetailFullView.tsx`.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (82/82 passed, incl. `p2_merge_split_breadcrumbs`), `cargo build --workspace`, `cargo build --release -p maxi-loop`, `cargo tree --depth 1 -p maxi-loop-mcp` (0 direct `sqlx`), and live smoke check on `maxi-loop-dev.service` (port `8080`).
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and E2E build/venv caches)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the latest release binary (with `F015` embedded SPA & merge/split/breadcrumbs) on `127.0.0.1:8080`.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Implement `F016` (P2 Performance & MCP):
   - `F-PRF-7`: Slow DB queries & operations aggregation (`span_minute_metrics` or `operation_minute_metrics` normalized by `op` + normalized SQL/description)
   - `F-PRF-6`: Compare releases API & UI (latency p50/p95/p99, throughput, error rate, and issue counts delta)
   - `F-MCP-14`: MCP tools `list_slow_queries` & `compare_releases`
   - `F-MCP-15`: MCP prompt templates (`prompts/list`, `prompts/get`) for issue triage and fixing
2. Or proceed to Production Deployment & Docs (`Dockerfile` multi-stage, `docker-compose.prod.yml`, `README.md`) if user prefers.
