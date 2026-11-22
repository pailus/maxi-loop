# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Claude Code
- Work (user-agreed order: security checks → spool → load test → production packaging LAST):
  - Security tooling check (no commit of code): `cargo-audit` installed and run (no compiled vulnerable crate; see progress), `npm audit` clean; `gitleaks` install denied by sandbox → `rg` secret-pattern scan instead (clean).
  - `F020` — Optional ingest write-ahead spool (`INGEST_SPOOL_DIR`, D012) — done.

## Current state
- What works and is verified: `F001`–`F020` done. After F020: 129 cargo tests pass; `verify_ingest_spool.py` (SIGKILL mid-burst, 0 lost), `verify_sentry_sdk.py`, `verify_users_affected.py` PASS on the new release binary; `maxi-loop-dev.service` (port 8080) restarted on it (spool NOT enabled on the dev service).
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, E2E caches)

## What the next session MUST watch out for
- **Pre-existing failing test:** `crates/server/tests/phase2_pii_releases_alerts.rs:777` fails on clean `main` too (`evaluate_spike_rules` is global and counts spike rules of older test tenants in the shared dev DB: got 5–8, expected 1). Not caused by F020; user informed. Fix needs scoping the assertion/evaluator call per tenant or cleaning old test tenants — ask before changing the test.
- Spool (D012): only `Spool::enqueue` may send on `ingest_tx` while the spool is on; replay must run after the PII rule cache loads and before the listener binds; worker checkpoints even after a failed flush (by design). MSRV is now 1.89.
- `gitleaks` is still not installed (sandbox denied the binary download) — user can install it manually if wanted.
- `cargo audit` reports `libsqlite3-sys`/`rsa` from unused optional `sqlx` drivers (not compiled) and `chrono` unmaintained — no action taken, no ignore file added.
- Visual tokens are 1:1 with MaxiBoard (D009); responsive conventions (D008).
- `instance_secrets.user_hash_key` must stay worker-only; don't rotate without a recount plan.
- `alert_deliveries.issue_id` is nullable (0013): `LEFT JOIN issues` when listing deliveries.
- Dev host has no SMTP: email deliveries are `failed` in E2E (expected).
- `issues.issue_type` allow-list lives in migration 0011 and `crates/issues/src/service.rs` (`ISSUE_TYPES`).

## Suggested next steps
1. Load test (user-agreed next): rerun `maxi-loop-loadgen` against the current release (errors + transactions/spans, with and without `INGEST_SPOOL_DIR`), measure RSS idle/under load, throughput, ingest latency, 429 behaviour; write `docs/benchmarks-phase2.md`.
2. Production packaging LAST (user decision): `Dockerfile`, `docker-compose.prod.yml` (spool volume with 0700), reverse-proxy TLS guide.
