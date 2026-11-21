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
migrations/              — SQLx PostgreSQL migrations (`0001_initial_schema.sql`: schema, daily partitions, RLS)
crates/                  — Rust workspace crates (vertical-slice modular layout)
  platform/              — `maxi-loop-platform`: config, typed errors, DB pool/migrations/partitions, TenantContext & RLS tx helpers, Argon2id bootstrap
  auth/                  — `maxi-loop-auth`: Argon2id login/logout/me, CSPRNG session token + SHA-256 hash storage, `HttpOnly`/`SameSite=Lax` cookies, CSRF protection, login rate limiter, `AuthSession` extractor
  org/                   — `maxi-loop-org`: tenant/workspace/project/DSN service (`begin_tenant_tx` + `TenantContext`), CSPRNG DSN generation & rotation, retention override validation (`<= global`), slug & single-tenant routes
  ingest/                — `maxi-loop-ingest`: `POST /api/{project_id}/envelope/`, in-memory `DsnCache` (0 per-envelope DB queries, live sync via `ProjectDsnObserver`), bounded gzip/deflate/brotli decompression, early drop of unsupported items, per-project `governor` rate limiting, bounded `tokio::sync::mpsc` channel backpressure (`429` + `Retry-After` / `X-Sentry-Rate-Limits`), baseline PII scrubbing (`scrub.rs`), exception + in-app frame / message fallback fingerprinting (`fingerprint.rs`), and 100ms–1s batch writer (`worker.rs`: issue upsert, regression auto-reopen, `MAX_FULL_EVENTS_PER_ISSUE` cap + sample, multi-row `events` & `issue_hourly_counters` upserts)
  issues/                — `maxi-loop-issues`: RLS-enforced issues query/detail/stats/triage service (`begin_tenant_tx` + `TenantContext`), pagination & filtering (`project`, `environment`, `release`, `status`, `is_regressed`, `query`, `sort`), sample event & stack trace lookup, hourly/daily counter series, status triage (`unresolved`/`resolved`/`ignored` with `Role::can_triage_issues`), and single-tenant + slug-hierarchy routes
  server/                — `maxi-loop` binary: composition root, CLI (`serve` / `bootstrap`), router, security headers, `/healthz`, embedded SPA
  # Feature crates added one-by-one as each feature is built (no empty speculative crates):
  # retention/, alerts/, mcp/, performance/, sourcemaps/
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
- `crates/server/src/main.rs` — Single-binary entrypoint (`maxi-loop` / `maxi-loop serve` to run migrations, pre-create daily partitions, ensure default tenant/workspace, and serve HTTP on `APP_HOST:APP_PORT`; `maxi-loop bootstrap` CLI subcommand for idempotent tenant/workspace/admin provisioning).
- `crates/server/build.rs` — Ensures `../../web/dist` exists at compile time for `rust-embed`.
- `web/src/main.tsx` — React SPA entry point (Vite dev server proxies `/api` and `/healthz` to `:8080`).

## Main Data Flow
- Current (F001–F006): Startup → SQLx migrations (`migrations/0001_initial_schema.sql`) → pre-create `events_YYYY_MM_DD` partitions → idempotent default tenant/workspace (+ optional Argon2id admin user) bootstrap → preload in-memory `DsnCache` → spawn background ingest batch worker (`spawn_ingest_worker`) → Axum HTTP server (`/healthz`, `/api/v1/health`, `/api/{project_id}/envelope/`, `/api/v1/auth/{login,logout,me}`, `/api/v1/tenants/current`, `/api/v1/t/{tenant_slug}/...`, `/api/v1/workspaces`, `/api/v1/projects`, `/api/v1/issues/...`, embedded React SPA). Accepted envelopes flow through the bounded `mpsc` channel into the worker pool, which scrubs PII, computes SHA-256 fingerprints, and flushes multi-row batches every 100ms–1s via `begin_worker_tx` (`SET LOCAL ROLE maxi_loop_worker`) into `issues`, daily-partitioned `events` (capped at `MAX_FULL_EVENTS_PER_ISSUE` + sample), and `issue_hourly_counters`. All tenant API DB operations (`org`, `issues`) run via `begin_tenant_tx` (`SET LOCAL ROLE maxi_loop_app` + transaction-local `app.tenant_id` enforced by Postgres Row-Level Security).
- Planned (Phase 1): Sentry SDK → `ingest` crate (`/api/{project_id}/envelope/`, DSN cache auth, decompression, rate limit, bounded channel) → worker pool (PII scrub, fingerprint, batch insert) → PostgreSQL (daily partitioned events, RLS tenant isolation) → `issues`/`org`/`auth` services → JSON API & React UI (and later `mcp`).

## External Services & Config
- **PostgreSQL 16** (via `docker-compose.yml`, host port `55432` by default).
- Required/optional env vars (see `.env.example`, never commit `.env`):
  `DATABASE_URL` (required), `APP_HOST`, `APP_PORT`, `PUBLIC_BASE_URL`, `DB_MAX_CONNECTIONS`, `SINGLE_TENANT_MODE`, `COOKIE_SECURE`, `SESSION_TTL_HOURS`, `RETENTION_DEFAULT_DAYS`, `MAX_FULL_EVENTS_PER_ISSUE`, `MAX_ENVELOPE_COMPRESSED_BYTES`, `MAX_ENVELOPE_DECOMPRESSED_BYTES`, `MAX_EVENT_PAYLOAD_BYTES`, `INGEST_CHANNEL_CAPACITY`, `DEFAULT_TENANT_SLUG`, `DEFAULT_TENANT_NAME`, `DEFAULT_WORKSPACE_SLUG`, `DEFAULT_WORKSPACE_NAME`, `BOOTSTRAP_ADMIN_EMAIL`, `BOOTSTRAP_ADMIN_PASSWORD`, `BOOTSTRAP_ADMIN_NAME`, `LOG_FORMAT`, `RUST_LOG`, plus compose vars `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB`, `POSTGRES_PORT`.

## Conventions Found in the Code
- Rust 2024 edition (`cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, no `unwrap()`/`expect()` on production paths).
- All shared crate versions are declared in root `[workspace.dependencies]`.
- Security headers applied globally via `tower-http`; unknown `/api/v1/*` routes return JSON `404` (never HTML).
- `Config` implements custom `Debug` redacting `database_url`; health/error responses never leak raw DB errors or secrets.
- Frontend uses `npm` (`web/package-lock.json`), TypeScript strict mode, Tailwind CSS v4, English UI.

## Known Pitfalls
- `rust-embed` inside `crates/server` references `../../web/dist`: run `npm --prefix web run build` before `cargo build` when shipping UI changes. `crates/server/build.rs` creates an empty `../../web/dist` if absent so `cargo check`/`cargo test` still succeed on a clean checkout.
- `docker-compose.yml` requires `POSTGRES_PASSWORD` in `.env` (fails fast if missing; never commit `.env`).
