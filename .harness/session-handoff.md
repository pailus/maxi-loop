# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: F011 (Phase 2b: Embedded MCP server `/mcp` Streamable HTTP for AI issue analysis and triage) — done

## Current state
- What works and is verified:
  - **Phase 1 (`F001`–`F009`), Phase 2 (`F010`), and Phase 2b (`F011`) are complete.**
  - Added migration `migrations/0004_phase2b_mcp.sql` (`mcp_tokens`, `mcp_audit_logs`, `issue_notes` + RLS).
  - Moved pure PII scrubbing to `crates/platform/src/scrub.rs` (re-exported by `crates/ingest/src/scrub.rs`) for ingest-parity MCP output scrubbing without coupling `crates/mcp` to `crates/ingest` or `sqlx`.
  - Extended `crates/auth/src/mcp.rs` (`McpAuthService`, `/api/v1/mcp/tokens`, `/api/v1/mcp/audit-logs`), `crates/org/src/service.rs` (`OrgService`), and `crates/issues` (`notes.rs`, `IssuesService`, `/api/v1/issues/{id}/notes`, `since`/`until`/`sort=regressed`, `resolve_release_commit_sha`).
  - Created `crates/mcp` (`maxi-loop-mcp`, **zero `sqlx` dependency** per `D006`) serving Streamable HTTP JSON-RPC 2.0 on `POST /mcp` with all 7 tools (`list_projects`, `list_issues`, `get_issue`, `get_event`, `get_issue_stats`, `update_issue_status`, `add_issue_note`), per-token `governor` rate limiting (`429`), output PII scrubbing, untrusted-data wrapping, truncation markers, release-to-`commit_sha` mapping, and full audit logging.
  - Added MaxiBoard UI `web/src/modules/mcp/McpView.tsx` (**MCP & AI** sidebar tab) and **Notes & AI Triage** tab in `IssueDetailFullView.tsx`, rebuilt embedded SPA.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (55/55 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live smoke check against restarted `maxi-loop-dev.service` on port `8080`.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and `tests/e2e/python/.venv/`)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running on `127.0.0.1:8080` for the user's Agentway tunnel preview.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Start **Phase 3 — `F012`**: Performance monitoring (transactions, spans, 1-minute duration histograms for p50/p95/p99, throughput & error rate, sampled zstd-compressed full traces, distributed trace waterfall UI, and MCP `list_transactions` + `get_trace` tools).
