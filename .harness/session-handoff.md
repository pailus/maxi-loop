# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: F013 (Phase 4: React Source Maps upload per release and bounded LRU ingest-time resolution) — done

## Current state
- What works and is verified:
  - **Phase 1 (`F001`–`F009`), Phase 2 (`F010`), Phase 2b (`F011`), Phase 3 (`F012`), and Phase 4 (`F013`) are complete.**
  - Added migration `migrations/0006_phase4_sourcemaps.sql`: `source_maps` table (`content_zstd BYTEA`, `(tenant_id, project_id, release_version, filename)` unique key, partial `debug_id` index, RLS for `maxi_loop_app` + `maxi_loop_worker`).
  - Created vertical-slice crate `crates/sourcemaps` (`maxi-loop-sourcemaps`) using `sourcemap` v9.3.2 with content-validated zstd codec (`codec.rs`), byte- & entry-bounded LRU cache (`cache.rs`, default 64 MiB / 128 entries), ingest-time JS/React stack frame resolver (`resolver.rs` matching by `debug_id` or `release + filename`, populating `pre_context`/`context_line`/`post_context` & `in_app` before fingerprinting), and `/api/v1/sourcemaps` + raw CI upload `/api/v1/projects/{slug}/releases/{version}/sourcemaps` HTTP API (`http.rs`).
  - Wired `SourceMapResolver` into `crates/ingest` (`fingerprint.rs` + `worker.rs`) right after PII scrubbing and before fingerprinting, and into `crates/server`.
  - Updated MaxiBoard UI `web/src/modules/releases/ReleasesView.tsx` with the **Source Maps** tab (`.js.map` file upload, Debug ID auto-detection, zstd size display, delete), rebuilt embedded SPA.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (73/73 passed, incl. 7 `maxi-loop-sourcemaps` unit tests & `phase4_sourcemaps` integration test), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live smoke check against restarted `maxi-loop-dev.service` on port `8080`.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and `tests/e2e/python/.venv/`)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the updated `F013` release binary on `127.0.0.1:8080` for the user's Agentway tunnel preview.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Start **Phase 5 — `F014`** (final feature in `feature-list.json`): Go & Rust SDK verification (error + tracing), multi-user RBAC (`tenant_owner`, `tenant_admin`, `workspace_admin`, `member`, `viewer`), inherited settings, per-project API tokens, and aggregate tenant rate limits.
