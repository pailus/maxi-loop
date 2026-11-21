# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F003 (Session auth (Argon2id + CSRF) and Tenant/Workspace/Project/DSN management API) — done

## Current state
- What works and is verified:
  - `crates/auth` (`maxi-loop-auth`) provides `/api/v1/auth/{login,logout,me}` with Argon2id password verification (and dummy hash anti-enumeration), 256-bit CSPRNG session tokens stored only as SHA-256 hashes in PostgreSQL `sessions`, `HttpOnly; SameSite=Lax` cookies, constant-time `X-CSRF-Token` enforcement in `AuthSession` extractor for all state-changing methods, and bounded `LoginRateLimiter` (`429` + `Retry-After`)
  - `crates/org` (`maxi-loop-org`) provides tenant-isolated (`begin_tenant_tx` + `TenantContext`) management of workspaces, projects, and CSPRNG Sentry DSN generation/rotation via both slug-based routes (`/api/v1/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}`) and single-tenant convenience routes (`/api/v1/tenants/current`, `/api/v1/workspaces`, `/api/v1/projects`)
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (23/23 including Postgres 16 RLS and cross-tenant HTTP API isolation tests), `cargo build --workspace`, and live HTTP smoke test
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` for local dev)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **F004**: Sentry Envelope ingest endpoint (`POST /api/{project_id}/envelope/`) in `crates/ingest` with in-memory DSN cache (refreshed on project create/DSN rotate without per-envelope DB queries), gzip/deflate/brotli decompression, body/event size limits, early drop of unsupported items (`session`, `replay`, `profile`, `attachment`), per-project `governor` rate limiting, and bounded `tokio::sync::mpsc` channel backpressure returning `429` with `Retry-After` / `X-Sentry-Rate-Limits` headers.
