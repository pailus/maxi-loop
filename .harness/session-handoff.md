# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: Browser Ingest CORS fix & Official React SDK (`@sentry/react`) Headless Chrome E2E (Error + Vite Source Maps + Browser→Backend Distributed Trace) — done

## Current state
- What works and is verified:
  - **All roadmap phases (`F001`–`F014`, Phases 1–5) AND live E2E verification across all 5 PRD SDK stacks (Python FastAPI, Python Django, Go, Rust, React/Browser) are 100% complete and verified.**
  - Added non-credentialed CORS support (`OPTIONS` 204 preflight + `Access-Control-Allow-Origin: *`, `Access-Control-Expose-Headers: X-Sentry-Rate-Limits, Retry-After`, explicitly no `Allow-Credentials`) strictly on `/api/{project_id}/envelope/` in `crates/ingest/src/http.rs`, keeping `/api/v1/*` strictly same-origin; verified in `crates/server/tests/envelope_ingest.rs`.
  - Created isolated Vite + React + `@sentry/react` E2E suite (`tests/e2e/react/`, `tests/e2e/verify_react_sdk.py`) verified live in real Headless Google Chrome (`google-chrome --headless=new`): cross-origin CORS, CI `.js.map` upload via `mlapi_...` write token, minified error resolution to `src/checkout.ts` with `context_line`/`pre_context`/`post_context` & `node_modules/` marked `in_app=False`, PII scrubbing (`[Filtered]`), real 2-service **Browser (React) → Backend (Django)** distributed trace waterfall (`6 spans`, `p50=605.0ms`), and tampered-DSN rejection (`0` issues & `0` transactions).
  - All checks pass: `npm --prefix tests/e2e/react run build`, `npm --prefix tests/e2e/react audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (77/77 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live Chrome E2E against `maxi-loop-dev.service` on port `8080`.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, `tests/e2e/python/.venv/`, `tests/e2e/rust/target/`, `tests/e2e/react/node_modules/`, and `tests/e2e/react/dist/`)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the latest release binary (with ingest CORS) on `127.0.0.1:8080` for the user's Agentway tunnel preview.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Wait for user direction among:
   - Production deployment packaging (`Dockerfile` multi-stage, `docker-compose.prod.yml`, `README.md`, operational guide)
   - P2 Issues (`F-ISS-9` manual merge/split, `F-ISS-10` breadcrumbs viewer)
   - P2 Performance & MCP (`F-PRF-6` compare releases, `F-PRF-7` slow queries, `F-MCP-14`/`F-MCP-15` tools & prompt templates)
