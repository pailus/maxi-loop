# ARCHITECTURE.md — Project Map

> A short map of this project so each session does not have to re-explore the
> whole repo. Keep it under ~150 lines and update it in the same commit
> whenever structure, entry points, services, or conventions change.

## Overview
**Maxi Loop** is a lightweight, single-binary Rust + embedded React + PostgreSQL error tracking (**Issues**) and **Performance** monitoring platform compatible with official Sentry SDKs (Django, FastAPI, Go, Rust, React). It supports strict multi-tenant isolation (`tenant → workspace → project` with automatic single-tenant default mode) and will expose an embedded MCP server (`/mcp`) over the internal service layer.

## Directory Map
```
docs/              — PRD (sentry-lite-prd.md) and technical design (sentry-lite-design.md)
src/               — Rust backend crate (`maxi-loop`)
  main.rs          — Binary entry point: env config, tracing, DB pool, axum server, graceful shutdown
  lib.rs           — Shared `AppState` and PostgreSQL pool setup
  config.rs        — Centralized env config parsing/validation (redacted Debug)
  error.rs         — Typed application/config errors mapped to safe HTTP responses
  http/            — Axum HTTP layer
    mod.rs         — Router assembly, security headers, API 404 fallback
    health.rs      — `/healthz` and `/api/v1/health` (bounded DB `SELECT 1` check)
    assets.rs      — Embedded React SPA handler (`rust-embed` from `web/dist`)
web/               — React + TypeScript + Vite + Tailwind CSS v4 frontend (built into `web/dist`)
docker-compose.yml — Isolated PostgreSQL 16 service for local dev & integration tests
.harness/          — AI working rules, feature list, progress log, decisions, handoff
```

## Entry Points
- `src/main.rs` — Runs the single-binary HTTP server (`APP_HOST:APP_PORT`, default `127.0.0.1:8080`).
- `build.rs` — Ensures `web/dist/` exists at compile time for `rust-embed`.
- `web/src/main.tsx` — React SPA entry point (dev server proxies `/api` and `/healthz` to `:8080`).

## Main Data Flow
- Current (F001): HTTP request → Axum router + security headers + tracing → `/healthz` or `/api/v1/health` (PostgreSQL `SELECT 1` with 2s timeout) or embedded React SPA (`web/dist`).
- Planned (Phase 1): Sentry SDK → `/api/{project_id}/envelope/` (DSN cache auth, decompression, rate limit, drop unsupported items) → bounded `tokio::mpsc` → worker pool (PII scrub, fingerprint, batch insert) → PostgreSQL (daily partitioned events, RLS tenant isolation) → JSON API & React UI.

## External Services & Config
- **PostgreSQL 16** (via `docker-compose.yml`, host port `55432` by default).
- Required/optional env vars (see `.env.example`, never commit `.env`):
  `DATABASE_URL` (required), `APP_HOST`, `APP_PORT`, `DB_MAX_CONNECTIONS`, `RETENTION_DEFAULT_DAYS`, `MAX_FULL_EVENTS_PER_ISSUE`, `LOG_FORMAT`, `RUST_LOG`, plus compose vars `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB`, `POSTGRES_PORT`.

## Conventions Found in the Code
- Rust 2024 edition (`cargo fmt`, `cargo clippy --all-targets -- -D warnings`, no `unwrap()`/`expect()` on production paths).
- Security headers applied globally via `tower-http`; unknown `/api/v1/*` routes return JSON `404` (never HTML).
- `Config` implements custom `Debug` redacting `database_url`; health/error responses never leak raw DB errors or secrets.
- Frontend uses `npm` (`web/package-lock.json`), TypeScript strict mode, Tailwind CSS v4, English UI.

## Known Pitfalls
- `rust-embed` embeds `web/dist` at compile time: run `npm --prefix web run build` before `cargo build` when shipping UI changes. `build.rs` creates an empty `web/dist` if absent so `cargo check`/`cargo test` still succeed on a clean checkout.
- `docker-compose.yml` requires `POSTGRES_PASSWORD` in `.env` (fails fast if missing; never commit `.env`).
