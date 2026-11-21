# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F009 (Envelope load-test generator and real Python Django/FastAPI Sentry SDK E2E verification) — done (**completes Phase 1 MVP**)

## Current state
- What works and is verified:
  - **Phase 1 (MVP) `F001`–`F009` is 100% complete.**
  - Added `crates/loadgen` (`maxi-loop-loadgen` binary): synthetic Sentry Envelope HTTP/1.1 keep-alive load-test generator supporting `none`/`gzip`/`deflate`/`br`, optional unsupported `session` early-drop item, total-request or duration modes, latency percentiles (`min`/`p50`/`p95`/`p99`/`max`), and `429` header verification (`Retry-After`, `X-Sentry-Rate-Limits`).
  - Added official Python `sentry-sdk` E2E verification suite in `tests/e2e/python/` (`verify_sentry_sdk.py`, `requirements.txt`, `README.md`) verified live against `maxi-loop` with `sentry-sdk==2.75.0`, `fastapi==0.165.0`, and `django==5.2.19` by changing only the DSN (grouping, `in_app` frames + `context_line`, hourly counters, `[Filtered]` PII scrubbing, and invalid-DSN negative test).
  - Documented Phase 1 baseline measurements in `docs/benchmarks-phase1.md`: `13 MB` server binary, **`8.4 MiB` idle RSS**, `27.6k req/s` gzip burst (`63.6k req/s` uncompressed), exact `120/min` per-project `429` (`1:error:project`), and `23,040` events persisted in 5s under saturation (`5:error:organization` backpressure, 0 lost accepted events, `100.6 MiB` peak RSS).
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (45/45 passed), `cargo build --workspace`, live release benchmarks, and Python FastAPI + Django E2E suite.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` and `tests/e2e/python/.venv/`, both gitignored)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **Phase 2 — `F010`**: Configurable PII scrubbing, Release tracking, and Alerts (Webhook first per `D002`, then Telegram and Email):
   - Add migration & schema for configurable PII scrub rules (inherited tenant → workspace → project), releases metadata (commit SHA mapping for future Phase 2b MCP), alert rules, and alert delivery channels/logs.
   - Apply configurable scrub rules in `crates/ingest` worker alongside mandatory baseline scrubbing (`D002`).
   - Implement `crates/alerts` (with SSRF protections on outbound webhooks per `.harness/SECURITY.md` §3, timeouts, and cooldown/dedup) + UI management views in `web/`.
