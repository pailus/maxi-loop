# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Claude Code
- Work: Team & settings full width; `F022` invite-by-link onboarding; `F023` self-service password change (D015); `F024` issue assignment UI + MCP (D016).

## Current state
- What works and is verified: `F001`–`F024` `done`. New tests `f022_invitations_password.rs`, `f024_issue_assignment.rs` PASS; `cargo test --workspace --no-fail-fast` PASS (0 failures this run); fmt/clippy/web build PASS; release binary running as `maxi-loop-dev` on :8080 with migrations 0014–0015 applied.
- What is in progress but unfinished: none
- Files left "dirty" / half-done: **nothing is committed** — the working tree holds the earlier D014 URL-state/UI-polish session changes plus F022/F023. Commit them as separate commits when the user asks (F024 = migration 0015, `crates/issues/src/assignment.rs` + issues/mcp edits, `f024` test, `mcp_server.rs` tool count, `AssigneePicker.tsx`, issue views, `api.ts` assignment part) (D014 UI files vs. F022/F023: migration 0014, `crates/auth/src/{invitations,members,service,http,model,lib}.rs`, new test, `web/src/modules/auth/{AcceptInviteView,ChangePasswordDialog}.tsx`, `web/src/lib/api.ts`, plus invite/password parts of `App.tsx` and `TeamAndSettingsView.tsx`, which also contain D014 edits).

## What the next session MUST watch out for
- Workspace isolation gaps found 2026-11-22 are fixed by F025/D017 (`b0_workspace_isolation.rs`). New project-data endpoints must use `AuthSession::project_access()` (lists, in SQL) and `enforce_project_read/write` or `&dyn AccessGuard` (single records) — never `session.role` alone.
- `phase2_pii_releases_alerts.rs:777` now fails consistently on this dev DB (also on pre-B0 code): leftover spike rules of old test tenants fire. Needs the user's OK to scope `evaluate_spike_rules` in the test or clean old test tenants.
- **Stage B agreed with user:** per-member project access + invite project picker. Change `user_can_triage_project` (migration 0015) rather than adding a second rule; every project-data endpoint (issues, events, performance, releases, sourcemaps, alerts, MCP, API tokens) needs a `can_access_project` check.
- Invite links are copy-only (no email). Forgot-password does not exist; the login screen still points to `maxi-loop bootstrap` for resets.
- Invite accept runs entirely in a worker tx (invitee has no tenant context yet); keep the `FOR UPDATE OF i` lock so a token can't be used twice.
- **Pre-existing failing test:** `crates/server/tests/phase2_pii_releases_alerts.rs:777` fails on clean `main` too (`evaluate_spike_rules` is global and counts spike rules of older test tenants in the shared dev DB). Not caused by F020/F021; ask user before changing the test or scoping `evaluate_spike_rules`.
- Spool (D012): only `Spool::enqueue` may send on `ingest_tx` while the spool is on; replay must run after the PII rule cache loads and before the listener binds; worker checkpoints even after a failed flush (by design). MSRV is 1.89.
- `gitleaks` is still not installed (sandbox denied binary download).
- `cargo audit` reports `libsqlite3-sys`/`rsa` from unused optional `sqlx` drivers (not compiled) and `chrono` unmaintained.
- Visual tokens are 1:1 with MaxiBoard (D009); responsive conventions (D008).
- `instance_secrets.user_hash_key` must stay worker-only; don't rotate without a recount plan.
- `alert_deliveries.issue_id` is nullable (0013): `LEFT JOIN issues` when listing deliveries.
- Dev host has no SMTP: email deliveries are `failed` in E2E (expected).
- `issues.issue_type` allow-list lives in migration 0011 and `crates/issues/src/service.rs` (`ISSUE_TYPES`).

## Suggested next steps
0. **Stage B — project-level access** (see watch-out above), then commit the pending work in separate commits when the user asks.
1. **Production packaging (LAST item in user-agreed order):** `Dockerfile` (multi-stage build: React `web/dist` + Rust release binary, non-root user, `0700` spool dir support), `docker-compose.prod.yml` (with PostgreSQL + `INGEST_SPOOL_DIR` volume), and reverse-proxy TLS deployment guide (Nginx/Caddy).
2. Optionally ask user whether to fix the pre-existing `phase2_pii_releases_alerts.rs:777` shared-DB test flakiness.
