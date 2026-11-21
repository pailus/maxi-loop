# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F002 (PostgreSQL schema, daily-partitioned events, RLS tenant isolation, and bootstrap CLI) — done

## Current state
- What works and is verified:
  - `migrations/0001_initial_schema.sql` creates `maxi_loop_app` & `maxi_loop_worker` DB roles, `app_current_tenant_id()`, `tenants`, `workspaces`, `projects`, `users`, `memberships`, `sessions`, `issues`, daily range-partitioned `events`, `issue_hourly_counters`, `FORCE ROW LEVEL SECURITY` policies, and `ensure_events_partition(DATE)`
  - `crates/platform` provides `TenantContext`, `begin_tenant_tx` (transaction-local `SET LOCAL ROLE maxi_loop_app` + `app.tenant_id`), `begin_worker_tx` (`SET LOCAL ROLE maxi_loop_worker`), migration & partition helpers, and Argon2id password hashing + idempotent `bootstrap_defaults`
  - `crates/server` runs migrations, pre-creates 7 days of `events` partitions, runs idempotent first-boot bootstrap, and supports `maxi-loop bootstrap` CLI subcommand
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (16/16 including Postgres 16 RLS integration test), `cargo build --workspace`, and live smoke test
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` for local dev)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **F003**: Session auth (Argon2id login/logout/me, CSPRNG session tokens, `HttpOnly`/`SameSite` cookies, CSRF header/token validation, login rate limiting) and Tenant/Workspace/Project/DSN management API (`crates/auth` and `crates/org`, mounted in `crates/server`), plus cross-tenant API isolation tests.
