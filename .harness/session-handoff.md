# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F006 (Issues query, detail, hourly stats, and triage JSON API with tenant isolation tests) — done

## Current state
- What works and is verified:
  - `crates/issues` (`maxi-loop-issues`) implements RLS-enforced (`begin_tenant_tx` + `TenantContext`) issue list pagination & filtering (`project_id`, `project_slug`, `workspace_slug`, `environment`, `release`, `status`, `is_regressed`, `query`, `sort`), issue detail (`stack_frames`, `tags`, `sample_event`, `hourly_series`, `daily_series`), hourly/daily stats (`/stats?interval=1h|1d`), stored events list/detail (`/events`), and status triage (`PATCH /issues/{id}/status` for `unresolved`/`resolved`/`ignored` with `Role::can_triage_issues` RBAC and CSRF validation)
  - Supports both single-tenant convenience routes (`/api/v1/issues/...`, `/api/v1/projects/{project_slug}/issues`) and explicit slug-scoped hierarchy routes (`/api/v1/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/...`)
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (36/36 passed), `cargo build --workspace`, and live HTTP smoke test
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` for local dev)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **F007**: Retention scheduler: daily partition maintenance, `DROP PARTITION`, and override batch delete (`crates/retention`):
   - Background scheduler pre-creating upcoming daily `events_YYYY_MM_DD` partitions and dropping partitions older than global retention (`RETENTION_DEFAULT_DAYS`, default 30 days) via `DROP TABLE` / `DROP PARTITION`
   - Bounded scheduled batch deletes for tenants/workspaces/projects configured with shorter retention overrides (`< global`) across `events`, `issue_hourly_counters`, and expired `issues` (plus expired `sessions` cleanup)
   - Integration tests against PostgreSQL 16 verifying partition creation, dropping expired partitions, and shorter override batch cleanup.
