# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F005 (Ingest worker pool: default PII scrubbing, fingerprinting, issue upsert/reopen, capped events, batch insert) — done

## Current state
- What works and is verified:
  - `crates/ingest/src/scrub.rs`: recursive depth-bounded baseline PII scrubbing (`[Filtered]`) for sensitive headers (`Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key`, etc., in both object and `[key, value]` tuple-array forms), cookies, sensitive fields (`password`, `token`, `secret`, `api_key`, `access_token`, `client_secret`, etc.), URL/query-string parameters, and `Bearer`/`Basic` tokens before persistence
  - `crates/ingest/src/fingerprint.rs`: Sentry event normalization and deterministic SHA-256 fingerprinting from exception type + in-app stack frames (`module`/`filename` + `function`, stripping Rust compiler hash suffixes), with normalized error message fallback (UUID/hex/number tokenization), custom `fingerprint` array support, `culprit` computation, and tag normalization
  - `crates/ingest/src/worker.rs`: asynchronous batch worker (`spawn_ingest_worker`, flushing every 100ms–1s or 256 events) and transactional `flush_processed_batch` (`begin_worker_tx`) that checks `to_regclass` before `ensure_events_partition`, upserts `issues`, auto-reopens `resolved` issues on regression (`status = 'unresolved'`, `is_regressed = TRUE`), caps full event payloads at `MAX_FULL_EVENTS_PER_ISSUE` (default 50) + sample (`is_sample = TRUE`), and batch-upserts `events` and `issue_hourly_counters`
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (34/34 passed), `cargo build --workspace`, and live HTTP envelope → worker → PostgreSQL smoke test
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` for local dev)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **F006**: Issues query, detail, hourly stats, and triage JSON API with tenant isolation tests (`crates/issues`):
   - Create `crates/issues` (`maxi-loop-issues`) vertical-slice crate (`model`, `service`, `http`) using `begin_tenant_tx` + `TenantContext`
   - List issues with pagination and filters by `project`, `environment`, `release`, and `status`
   - Issue detail returning summary, stack trace (`stack_frames`), tags, environment, release, sample event payload (`sample_event_id` / `is_sample`), and hourly/daily counter series from `issue_hourly_counters`
   - Issue status triage endpoint (`PATCH /api/v1/.../issues/{issue_id}` or `/status`) updating `unresolved` / `resolved` / `ignored` (clearing `is_regressed` when resolved) with server-side role authorization (`member`+ vs `viewer`)
   - Cross-tenant isolation tests verifying Tenant A cannot read or update Tenant B issues, events, or hourly stats.
