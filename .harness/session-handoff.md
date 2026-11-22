# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Claude Code
- Work: Team & settings full width; `F022` invite-by-link onboarding; `F023` self-service password change (D015); `F024` issue assignment UI + MCP (D016); `F025` workspace isolation hardening (D017); `F026` per-member project access (D018).

## Current state
- What works and is verified: `F001`–`F026` `done`. New tests `f022_invitations_password.rs`, `f024_issue_assignment.rs` PASS; `cargo test --workspace --no-fail-fast` → 145 passed, 0 failed (after the phase2 spike-test fix); fmt/clippy/web build PASS; release binary running as `maxi-loop-dev` on :8080 with migrations 0014–0016 applied.
- What is in progress but unfinished: none
- Files left "dirty" / half-done: none — everything is committed (not pushed).

## What the next session MUST watch out for
- Workspace isolation gaps found 2026-11-22 are fixed by F025/D017 (`b0_workspace_isolation.rs`). New project-data endpoints must use `AuthSession::project_access()` (lists, in SQL) and `enforce_project_read/write` or `&dyn AccessGuard` (single records) — never `session.role` alone.
- `phase2_pii_releases_alerts.rs` spike flake fixed (user OK 2026-11-23): the test disables stale `p2-alpha-*` spike rules before evaluating; `evaluate_spike_rules` itself is unchanged (instance-wide by design).
- Project limits (D018): keep using `project_access()` / `AccessGuard`; never treat "no membership_projects rows" as "all projects" — check `project_limited`.
- Invite links are copy-only (no email). Forgot-password does not exist; the login screen still points to `maxi-loop bootstrap` for resets.
- Invite accept runs entirely in a worker tx (invitee has no tenant context yet); keep the `FOR UPDATE OF i` lock so a token can't be used twice.
- Spool (D012): only `Spool::enqueue` may send on `ingest_tx` while the spool is on; replay must run after the PII rule cache loads and before the listener binds; worker checkpoints even after a failed flush (by design). MSRV is 1.89.
- `gitleaks` is still not installed (sandbox denied binary download).
- `cargo audit` reports `libsqlite3-sys`/`rsa` from unused optional `sqlx` drivers (not compiled) and `chrono` unmaintained.
- Visual tokens are 1:1 with MaxiBoard (D009); responsive conventions (D008).
- `instance_secrets.user_hash_key` must stay worker-only; don't rotate without a recount plan.
- `alert_deliveries.issue_id` is nullable (0013): `LEFT JOIN issues` when listing deliveries.
- Dev host has no SMTP: email deliveries are `failed` in E2E (expected).
- `issues.issue_type` allow-list lives in migration 0011 and `crates/issues/src/service.rs` (`ISSUE_TYPES`).

## Suggested next steps
0. Optional follow-ups: e-mail delivery for invites + forgot-password + assignee notifications (needs an e-mail sender outside `alerts`).
1. **Production packaging (LAST item in user-agreed order):** `Dockerfile` (multi-stage build: React `web/dist` + Rust release binary, non-root user, `0700` spool dir support), `docker-compose.prod.yml` (with PostgreSQL + `INGEST_SPOOL_DIR` volume), and reverse-proxy TLS deployment guide (Nginx/Caddy).
