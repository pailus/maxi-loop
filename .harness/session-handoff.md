# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity (finishing Claude Code's interrupted F021 session)
- Work (user-agreed order: security checks → spool → load test → production packaging LAST):
  - `F021` — Phase 2 load test (`maxi-loop-loadgen --kind error|transaction|mixed`, `tests/e2e/python/bench_phase2.py`, `docs/benchmarks-phase2.md`) — done.

## Current state
- What works and is verified: `F001`–`F021` all `done`. Release binaries `maxi-loop` (22.7 MiB) and `maxi-loop-loadgen` (2.97 MiB) built and benchmarked across 8 scenarios (without & with `INGEST_SPOOL_DIR`): 240,600 accepted envelopes, 0 lost, 0 5xx, 1,831,261 clean `429`s, idle RSS 12.0 MiB, peak RSS 115.3 / 106.0 MiB, sustained 3,674 err/s, 3,220 tx/s, 2,222 mixed/s (spool overhead 0.5%–3.2%). `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check/build --workspace`, `cargo build --release -p maxi-loop -p maxi-loop-loadgen`, `cargo test -p maxi-loop-loadgen` (9/9) PASS; `cargo test --workspace --no-fail-fast` 132 passed / 1 pre-existing failure.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, E2E caches)

## What the next session MUST watch out for
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
1. **Production packaging (LAST item in user-agreed order):** `Dockerfile` (multi-stage build: React `web/dist` + Rust release binary, non-root user, `0700` spool dir support), `docker-compose.prod.yml` (with PostgreSQL + `INGEST_SPOOL_DIR` volume), and reverse-proxy TLS deployment guide (Nginx/Caddy).
2. Optionally ask user whether to fix the pre-existing `phase2_pii_releases_alerts.rs:777` shared-DB test flakiness.
