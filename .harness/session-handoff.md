# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F004 (Sentry Envelope ingest endpoint with DSN cache, decompression, rate limiting, and bounded channel) — done

## Current state
- What works and isVerified:
  - `crates/ingest` (`maxi-loop-ingest`) implements `POST /api/{project_id}/envelope/` (and `/api/{project_id}/envelope`), authenticating DSN keys from in-memory `DsnCache` (preloaded at startup and updated live via `ProjectDsnObserver` on project creation/DSN rotation with 0 per-envelope DB queries)
  - Supports `gzip`, `deflate`, and `br` (brotli) request decompression with bounded `Read::take` protection against decompression bombs (`MAX_ENVELOPE_COMPRESSED_BYTES`, `MAX_ENVELOPE_DECOMPRESSED_BYTES`, `MAX_EVENT_PAYLOAD_BYTES`)
  - Drops unsupported item types (`session`, `replay_*`, `profile`, `attachment`, etc.) early without parsing their JSON payloads
  - Enforces per-project `governor` rate limits and bounded `tokio::sync::mpsc` channel backpressure, returning `429 Too Many Requests` with `Retry-After` and `X-Sentry-Rate-Limits` headers
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (29/29 passed), `cargo build --workspace`, and live gzipped envelope HTTP smoke test
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` for local dev)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **F005**: Ingest worker pool (`crates/ingest` worker + batch writer):
   - Default baseline PII scrubbing (`Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key` headers; `password`, `token`, `secret`, `api_key`, `access_token`, `refresh_token`, `client_secret` fields recursively in event payload) before persistence
   - Fingerprinting / automatic grouping: hash of exception type + in-app stack frames (`module`/`filename` + `function`), with fallback to normalized error message
   - Batch writer (flushing every 100ms–1s or max batch size) using `begin_worker_tx`: upserts `issues`, auto-reopens `resolved` issues on regression (`is_regressed = TRUE`, `status = 'unresolved'`), stores full event payloads in daily-partitioned `events` up to `MAX_FULL_EVENTS_PER_ISSUE` (default 50) + sample, and increments `issue_hourly_counters`.
