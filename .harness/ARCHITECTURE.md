# ARCHITECTURE.md — Project Map

> A short map of this project so each session does not have to re-explore the
> whole repo. Keep it under ~150 lines and update it in the same commit
> whenever structure, entry points, services, or conventions change.

## Overview
**Maxi Loop** is a lightweight, single-binary Rust + embedded React + PostgreSQL error tracking (**Issues**) and **Performance** monitoring platform compatible with official Sentry SDKs (Django, FastAPI, Go, Rust, React). It is organized as a **Cargo workspace with vertical-slice feature crates** (`crates/<module>/`) and a feature-modular React frontend (`web/src/modules/<module>/`).

## Directory Map
```
Cargo.toml               — Cargo workspace root + shared `[workspace.dependencies]`
docs/                    — PRD (sentry-lite-prd.md) and technical design (sentry-lite-design.md)
crates/                  — Rust workspace crates (vertical-slice modular layout)
  platform/              — `maxi-loop-platform`: config, typed errors, DB pool, (later: TenantContext & RLS tx helper)
  server/                — `maxi-loop` binary: composition root, router, security headers, `/healthz`, embedded SPA
  # Feature crates added one-by-one as each feature is built (no empty speculative crates):
  # auth/, org/, ingest/, issues/, retention/, alerts/, mcp/, performance/, sourcemaps/
web/                     — React + TypeScript + Vite + Tailwind CSS v4 frontend (built into `web/dist`)
  src/app/               — App shell & routing
  src/lib/               — Shared HTTP client & types
  src/modules/<feature>/ — Vertical-slice UI modules (currently: `health/`)
docker-compose.yml       — Isolated PostgreSQL 16 service for local dev & integration tests
.harness/                — AI working rules, feature list, progress log, decisions, handoff
```

## Module Dependency Rules
- `crates/platform` has **zero** dependencies on feature crates or `crates/server`.
- Feature crates (`auth`, `org`, `issues`, `ingest`, `retention`, `performance`, etc.) depend on `maxi-loop-platform` (and explicit upstream service crates only — no cycles). Each feature crate is a vertical slice (`model`, `repo`, `service`, `http` as needed).
- `crates/mcp` (Phase 2b) depends **only** on service crates (`org`, `issues`, `performance`), **never** on `sqlx` directly — compiler-enforced single service layer.
- `crates/server` is the thin composition root: wires config, DB pool, workers, schedulers, mounts each feature crate's `Router`, and embeds `web/dist`.

## Entry Points
- `crates/server/src/main.rs` — Single-binary entrypoint (`maxi-loop`), listening on `APP_HOST:APP_PORT` (default `127.0.0.1:8080`).
- `crates/server/build.rs` — Ensures `../../web/dist` exists at compile time for `rust-embed`.
- `web/src/main.tsx` — React SPA entry point (Vite dev server proxies `/api` and `/healthz` to `:8080`).

## Main Data Flow
- Current (F001): HTTP request → `crates/server` Axum router + security headers + tracing → `/healthz` or `/api/v1/health` (PostgreSQL `SELECT 1` with 2s timeout) or embedded React SPA (`web/dist`).
- Planned (Phase 1): Sentry SDK → `ingest` crate (`/api/{project_id}/envelope/`, DSN cache auth, decompression, rate limit, bounded channel) → worker pool (PII scrub, fingerprint, batch insert) → PostgreSQL (daily partitioned events, RLS tenant isolation) → `issues`/`org`/`auth` services → JSON API & React UI (and later `mcp`).

## External Services & Config
- **PostgreSQL 16** (via `docker-compose.yml`, host port `55432` by default).
- Required/optional env vars (see `.env.example`, never commit `.env`):
  `DATABASE_URL` (required), `APP_HOST`, `APP_PORT`, `DB_MAX_CONNECTIONS`, `RETENTION_DEFAULT_DAYS`, `MAX_FULL_EVENTS_PER_ISSUE`, `LOG_FORMAT`, `RUST_LOG`, plus compose vars `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB`, `POSTGRES_PORT`.

## Conventions Found in the Code
- Rust 2024 edition (`cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, no `unwrap()`/`expect()` on production paths).
- All shared crate versions are declared in root `[workspace.dependencies]`.
- Security headers applied globally via `tower-http`; unknown `/api/v1/*` routes return JSON `404` (never HTML).
- `Config` implements custom `Debug` redacting `database_url`; health/error responses never leak raw DB errors or secrets.
- Frontend uses `npm` (`web/package-lock.json`), TypeScript strict mode, Tailwind CSS v4, English UI.

## Known Pitfalls
- `rust-embed` inside `crates/server` references `../../web/dist`: run `npm --prefix web run build` before `cargo build` when shipping UI changes. `crates/server/build.rs` creates an empty `../../web/dist` if absent so `cargo check`/`cargo test` still succeed on a clean checkout.
- `docker-compose.yml` requires `POSTGRES_PASSWORD` in `.env` (fails fast if missing; never commit `.env`).
