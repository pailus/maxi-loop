# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: F010 (Phase 2: Configurable PII scrubbing, Release tracking, and Alerts — Webhook, Telegram, Email) — done

## Current state
- What works and is verified:
  - **Phase 1 (`F001`–`F009`) and Phase 2 (`F010`) are complete.**
  - Added migration `migrations/0003_phase2_pii_releases_alerts.sql` (`issues.first_release`, `pii_scrub_rules`, `releases`, `issue_releases`, `alert_channels`, `alert_rules`, `alert_deliveries` + RLS).
  - Implemented inherited configurable PII scrubbing (`crates/org/src/pii.rs`, `crates/ingest/src/scrub_cache.rs`, `scrub.rs`) applied additively after mandatory baseline scrubbing (`D002`), live-synced in memory.
  - Implemented Release tracking (`crates/ingest/src/worker.rs`, `crates/issues/src/releases.rs`) auto-tracking releases & inferred `commit_sha`, linking issues via `issue_releases`, and exposing `/api/v1/releases` + `/api/v1/releases/{id}/issues`.
  - Created `crates/alerts` (`maxi-loop-alerts`) with SSRF-safe IP-pinned delivery (`ssrf.rs`, `delivery.rs`), HMAC-SHA256 Webhook (priority #1), Telegram Bot API, server-configured SMTP/TLS Email, real-time `new_issue`/`regression` consumer + 60s `spike` evaluator (`engine.rs`), cooldown deduplication, masked secrets, and `/api/v1/alerts/{channels,rules,deliveries}` routes.
  - Added MaxiBoard UI views `ReleasesView.tsx` and `AlertsView.tsx` in `web/src/modules/` and rebuilt embedded SPA.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (48/48 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live smoke check against restarted `maxi-loop-dev.service` on port `8080`.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and `tests/e2e/python/.venv/`)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running on `127.0.0.1:8080` for the user's Agentway tunnel preview.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Start **Phase 2b — `F011`**: Embedded MCP server (`/mcp` Streamable HTTP) for AI issue analysis and triage:
   - Add migration for revocable scoped MCP Bearer tokens (`mcp_tokens`: SHA-256 hash, tenant/workspace/project scope, `read`/`triage` action scopes, rate limit) + `mcp_audit_logs` + `issue_notes` (all with RLS).
   - Create `crates/mcp` (`maxi-loop-mcp`) depending **only** on service crates (`org`, `issues`, `platform`) — **never** on `sqlx` directly (`D006` compiler-enforced single service layer).
   - Implement `list_projects`, `list_issues`, `get_issue` (with `commit_sha` release mapping & code context), `get_event`, `get_issue_stats`, `update_issue_status`, `add_issue_note` with untrusted-content wrapping, output PII scrubbing, per-token rate limiting, audit logging, and cross-tenant isolation tests.
