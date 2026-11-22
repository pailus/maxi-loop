# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: `F016` — P2 Performance & MCP: Compare releases (`F-PRF-6`), slow DB queries/operations aggregation (`F-PRF-7`), MCP `list_slow_queries` / `compare_releases` (`F-MCP-14`), & MCP prompt templates (`F-MCP-15`) — done

## Current state
- What works and is verified:
  - **All 16 roadmap features (`F001`–`F016`, Phases 1–5 + all PRD P2 features) are 100% implemented, tested, and live-verified.**
  - Migration `migrations/0009_p2_span_operations.sql` adds RLS-protected `span_minute_metrics` aggregating normalized DB queries & operations across all ingested transactions (with 76-bucket duration histograms and `sample_trace_id`), cleaned up by `crates/retention`.
  - Performance HTTP APIs (`GET /api/v1/performance/slow-operations`, `GET /api/v1/performance/compare-releases` + project & `/t/{tenant}/w/{workspace}/p/{project}/...` slug routes) enforce session/API-token scoping and Postgres RLS isolation.
  - Embedded `/mcp` server (zero direct `sqlx` dependency, `D006`) now exposes 11 tools (9 read incl. `list_slow_queries` & `compare_releases`, + 2 triage) and 4 prompt templates via `prompts/list` & `prompts/get` (`triage_issue`, `fix_issue`, `triage_release_regression`, `investigate_slow_queries`).
  - MaxiBoard React UI (`SlowOperationsView.tsx`, `CompareReleasesView.tsx`, `PerformanceView.tsx`) adds **Slow Queries & Ops** and **Compare Releases** sub-tabs with `?view=performance&tab=slow-queries|compare` deep links.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace`, `cargo test --workspace` (89/89 passed across 39 suites), `cargo build --workspace`, `cargo build --release -p maxi-loop`, `cargo tree --depth 1 -p maxi-loop-mcp` (0 direct `sqlx`), and live smoke check on `maxi-loop-dev.service` (port `8080`).
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and E2E build/venv caches)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the latest release binary (with `F016` embedded SPA, `span_minute_metrics`, compare-releases, and MCP prompts) on `127.0.0.1:8080`.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Production packaging & documentation: multi-stage `Dockerfile`, `docker-compose.prod.yml`, reverse-proxy TLS guide, and comprehensive `README.md`.
2. Optional: `maxi-loop mcp` stdio bridge subcommand (mentioned as optional in Design §5.10) or any custom operational enhancements the user requests.
