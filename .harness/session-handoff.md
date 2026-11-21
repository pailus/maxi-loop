# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: Official Python `sentry-sdk` (FastAPI & Django) E2E upgrade for Error + Performance + Distributed Tracing & OTel status-code parsing — done

## Current state
- What works and is verified:
  - **All roadmap phases (`F001`–`F014`, Phases 1–5) are 100% complete and verified.**
  - Hardened `crates/performance/src/ingest_proc.rs` (`extract_http_status_code`) to also inspect nested `contexts.trace.data["http.response.status_code"]` / `contexts.trace.data["http.status_code"]` (standard in `sentry-sdk` 2.x / OpenTelemetry semantic conventions), covered by unit test `detects_5xx_error_from_nested_otel_trace_data_status_code`.
  - Upgraded `tests/e2e/python/verify_sentry_sdk.py` & `tests/e2e/python/README.md` to verify official Python `sentry-sdk==2.75.0` (`fastapi==0.165.0`, `django==5.2.19` WSGI) across **both** Error Tracking AND Performance Monitoring (`traces_sample_rate=1.0`, histograms, error rates, span PII scrubbing, per-project `mlapi_...` API tokens, and a live 2-service **FastAPI → Django** distributed trace waterfall via `sentry-trace` + `baggage` headers).
  - All checks pass: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (77/77 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live FastAPI + Django E2E against restarted `maxi-loop-dev.service` on port `8080`.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, `tests/e2e/python/.venv/`, and `tests/e2e/rust/target/`)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the latest release binary on `127.0.0.1:8080` for the user's Agentway tunnel preview.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Wait for user direction among:
   - E2E React SDK (`@sentry/react`) + source map upload + browser→backend distributed trace
   - Production deployment packaging (`Dockerfile`, `docker-compose.prod.yml`, `README.md`)
   - P2 Issues (`F-ISS-9` manual merge/split, `F-ISS-10` breadcrumbs viewer)
   - P2 Performance & MCP (`F-PRF-6` compare releases, `F-PRF-7` slow queries, `F-MCP-14`/`F-MCP-15` tools & prompt templates)
