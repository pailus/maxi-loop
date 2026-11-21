# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: F014 (Phase 5: Go & Rust SDK verification, multi-user RBAC, inherited settings, API tokens, aggregate tenant limits) — done

## Current state
- What works and is verified:
  - **All roadmap phases (`F001`–`F014`, Phases 1–5) are 100% complete and verified.**
  - Added migration `migrations/0007_phase5_rbac_settings_tokens.sql`: inherited rate-limit columns on `tenants`/`workspaces`/`projects`, aggregate tenant rate-limit cap (`tenants.aggregate_rate_limit_per_minute`), and per-project `api_tokens` table (SHA-256 hash only, RLS for `maxi_loop_app` + `maxi_loop_worker`).
  - Implemented multi-user RBAC (`crates/auth/src/members.rs`, workspace-scoped `AuthSession` roles, privilege-hierarchy & last-owner guards, immediate session revocation on removal), per-project Bearer API tokens (`crates/auth/src/api_tokens.rs`, `mlapi_...`, strict single-project scoping + fail-closed admin denial), inherited settings PATCH endpoints (`crates/org`), and live per-project + aggregate per-tenant `governor` rate limiting (`crates/ingest/src/cache.rs`, `X-Sentry-Rate-Limits: organization`).
  - Added MaxiBoard UI `web/src/modules/admin/TeamAndSettingsView.tsx` (**Team & settings** tab) and inline inherited-override editing + Python/Go/Rust SDK quickstarts in `ProjectsView.tsx`, rebuilt embedded SPA.
  - Verified live E2E compatibility with official **Go (`github.com/getsentry/sentry-go` v0.31.1)** and **Rust (`sentry` v0.34.0 + `sentry-tracing`)** SDKs (`tests/e2e/go/`, `tests/e2e/rust/`, `tests/e2e/verify_go_rust_sdks.py`) for error grouping, stack frames, PII scrubbing (`[Filtered]`), 1-minute performance histograms, and 3-span sampled trace waterfalls.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (76/76 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, `cargo tree --depth 1 -p maxi-loop-mcp` (0 direct `sqlx`), and live smoke/E2E checks on `maxi-loop-dev.service` (port `8080`).
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, `tests/e2e/python/.venv/`, and `tests/e2e/rust/target/`)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the updated `F014` release binary on `127.0.0.1:8080` for the user's Agentway tunnel preview.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. All Phase 1–5 features (`F001`–`F014`) are complete. Next work (if requested by the user) would be P2 enhancements from PRD §7 (e.g., manual issue merge/split `F-ISS-9`, event breadcrumbs viewer `F-ISS-10`, release comparison `F-PRF-6`, slow query aggregation `F-PRF-7`, MCP `list_slow_queries`/`compare_releases` `F-MCP-14`, or production Docker/deployment packaging).
