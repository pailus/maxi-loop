# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F007 (Retention scheduler: daily partition maintenance, DROP PARTITION, and override batch delete) — done

## Current state
- What works and is verified:
  - `migrations/0002_drop_events_partition.sql` adds `drop_expired_events_partitions(cutoff_date DATE) RETURNS TEXT[]` (`SECURITY DEFINER`, executable by `maxi_loop_worker`) and `crates/platform/build.rs` tracks `../../migrations`
  - `crates/retention` (`maxi-loop-retention`) implements `run_retention_cycle` and `spawn_retention_scheduler`: pre-creates upcoming daily `events_YYYY_MM_DD` partitions, drops daily partitions older than global retention (`RETENTION_DEFAULT_DAYS`, default 30 days), and runs bounded batch deletes for shorter workspace/project retention overrides (`<= global`) as well as expired hourly counters, stale issues, and expired sessions
  - Wired into `crates/server/src/main.rs` on `maxi-loop serve`
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (39/39 passed), `cargo build --workspace`, and live smoke test verifying automatic drop of expired partition `events_2025_01_15`
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` for local dev)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **F008**: Embedded React UI: Login, Projects & DSN, Issues list/detail, and hourly chart (`web/src/`):
   - Organize vertical-slice frontend modules under `web/src/modules/` (`auth/`, `projects/`, `issues/`, `health/`) and shared API client (`web/src/lib/api.ts` handling credentials + `X-CSRF-Token`)
   - Login view authenticating via `/api/v1/auth/login` and hiding tenant/workspace chrome when `single_tenant_mode` is true (while showing workspace switcher/creator when `single_tenant_mode` is false)
   - Projects view listing & creating projects (`/api/v1/projects`) with copyable Sentry DSN, ingest endpoint, platform badge, retention/rate-limit settings, and DSN rotation action
   - Issues list view supporting filters (`project`, `environment`, `release`, `status`, search `query`, `sort`) and pagination
   - Issue detail view displaying summary, regression badge, in-app/all stack trace frames with source context (`pre_context`, `context_line`, `post_context`), tags, sample event JSON payload, hourly/daily occurrence chart, and `Resolve` / `Ignore` / `Reopen` triage actions
   - Build with `npm --prefix web run build` and verify the embedded SPA is served from the single Rust binary.
