# Progress Archive

> Older entries moved from `.harness/progress.md` to keep `.harness/progress.md`
> around 10 entries. Never delete entries from here.

---

### 2026-11-21 — Antigravity — F002: PostgreSQL schema, daily-partitioned events, RLS tenant isolation, and bootstrap CLI

**Status at end of session:** done

**Done:**
- Created `migrations/0001_initial_schema.sql` defining roles (`maxi_loop_app`, `maxi_loop_worker`), fail-closed `app_current_tenant_id()`, core tables (`tenants`, `workspaces`, `projects`, `users`, `memberships`, `sessions`, `issues`, daily range-partitioned `events`, `issue_hourly_counters`) with leading `(tenant_id, project_id)` primary keys and indexes, `ENABLE`/`FORCE ROW LEVEL SECURITY` policies, and `ensure_events_partition(DATE)`
- Implemented `TenantContext`, transaction-local `begin_tenant_tx` (`SET LOCAL ROLE maxi_loop_app` + `set_config('app.tenant_id', ..., true)`), and `begin_worker_tx` (`SET LOCAL ROLE maxi_loop_worker`) in `crates/platform/src/tenant.rs`
- Implemented SQLx migration runner and daily partition helpers (`ensure_events_partition`, `ensure_upcoming_events_partitions`) in `crates/platform/src/db.rs`
- Implemented Argon2id password hashing/verification and idempotent `bootstrap_defaults` in `crates/platform/src/bootstrap.rs`, plus `maxi-loop bootstrap` CLI subcommand and automatic first-boot bootstrap in `crates/server`
- Added PostgreSQL 16 integration test `crates/platform/tests/schema_rls_bootstrap.rs` verifying table RLS flags, daily partition creation, idempotent bootstrap, cross-tenant read/write/forged-insert isolation (including direct child partition queries), and fail-closed unset `app.tenant_id`

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (7 server + 8 platform unit + 1 Postgres 16 RLS integration = 16 passed)
- `cargo build --workspace` → PASS
- Manual smoke check (`maxi-loop bootstrap` x2 + `maxi-loop` server + `/healthz` + SIGTERM) → PASS

**Security notes:**
- Self-review passed: all slug/email/password/tenant_id inputs validated server-side; `bootstrap_admin_password` and `database_url` redacted in `Config::Debug`; Argon2id (`m=19456, t=2, p=1`) with CSPRNG `OsRng` salt; transaction-local `SET LOCAL ROLE` and `set_config(..., true)` prevent connection-pool tenant bleed; `app_current_tenant_id()` fails closed on empty/non-numeric GUC; negative tests cover cross-tenant read/update/delete/insert and unset `app.tenant_id`
- New dependencies: `argon2` v0.5 + `password-hash` v0.5 (RustCrypto, required by Design §5.9 / PRD F-ADM-1 for Argon2id password hashing) and `chrono` v0.4 (standard Rust UTC date/timestamp arithmetic for daily partitions). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Used transaction-local `SET LOCAL ROLE maxi_loop_app` alongside `set_config('app.tenant_id', $1, true)` so RLS is enforced even when the pool connection user is the DB owner/superuser, and automatically resets on commit/rollback

**Not finished / blockers:**
- Ready to begin F003 (Session auth + Tenant/Workspace/Project/DSN management API)

### 2026-11-21 — Antigravity — Refactor: Cargo Workspace + Vertical-Slice Module Layout (D006)

**Status at end of session:** done

**Done:**
- Converted `maxi-loop` to a Cargo workspace (`crates/platform` as `maxi-loop-platform`, `crates/server` as binary `maxi-loop`) with shared `[workspace.dependencies]` in root `Cargo.toml`
- Organized `web/src/` into vertical-slice modules (`app/`, `lib/`, `modules/health/`)
- Recorded `D006` in `.harness/decisions.md` and updated `.harness/ARCHITECTURE.md` & `.harness/AGENTS.md` with module dependency rules and `--workspace` verification commands

**Verification run:**
- `npm --prefix web run build` → PASS
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS
- `cargo test --workspace` → PASS (4 server + 5 platform = 9 passed)
- `cargo build --workspace` → PASS
- Manual smoke check (`maxi-loop` + Postgres 16) → PASS (`/healthz` 200 OK, embedded SPA served, clean SIGTERM)

**Key decisions & why:**
- User chose Option A (vertical-slice per feature) + Cargo Workspace (`D006`) for compiler-enforced boundaries and clean module ownership

**Not finished / blockers:**
- Ready to begin F002

### 2026-11-21 — Antigravity — F001: Project scaffolding, configuration, Docker Compose Postgres 16, and health server

**Status at end of session:** done

**Done:**
- Initialized git repository (`main`), copied PRD & design docs into `docs/`, filled `.harness/ARCHITECTURE.md`, recorded decisions `D001`–`D005` in `.harness/decisions.md`, and populated the Phase 1–5 roadmap (`F001`–`F014`) in `.harness/feature-list.json`
- Scaffolded `maxi-loop` Rust crate (`axum`, `tokio`, `sqlx`, `rust-embed`, `tower-http`, `tracing-subscriber`) with validated centralized config (redacted `DATABASE_URL` in Debug), security headers, bounded DB `/healthz` & `/api/v1/health`, and embedded SPA asset handler
- Scaffolded `web/` React + TypeScript + Vite + Tailwind CSS v4 shell (`npm`) embedded into the single binary
- Added isolated `docker-compose.yml` (`postgres:16-alpine` on `127.0.0.1:55432`) and `.env.example` (`.env` gitignored)
- Performed manual smoke check against running Postgres 16 (`/healthz`, `/api/v1/health`, `/`, `/api/v1/unknown`, graceful SIGTERM shutdown)

**Verification run:**
- `npm --prefix web run build` → PASS (TypeScript check + Vite production bundle built cleanly)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test` → PASS (9 passed, 0 failed)
- `cargo build` → PASS
- Manual smoke check (`maxi-loop` + Postgres 16) → PASS (`200 OK` on `/healthz`, `/api/v1/health`, `/`; `404` JSON on unknown API; clean SIGTERM shutdown)

**Security notes:**
- Self-review passed: all config env inputs validated with range/scheme checks; `DATABASE_URL` redacted from `Debug`/logs; health errors return generic `unavailable` without leaking DB details; global security headers (`nosniff`, `DENY`, `Referrer-Policy`, `CSP`) applied; path traversal rejected on SPA handler; `.env` gitignored and not committed; negative test cases cover missing/invalid config, unreachable DB, unknown API route, and path traversal
- Dependency audit: `npm --prefix web audit` = 0 vulnerabilities; `cargo-audit` and `gitleaks` are not installed on this machine (skipped, noted explicitly)

**Key decisions & why:**
- Recorded `D001`–`D005` in `.harness/decisions.md` (user confirmed all recommendations)

**Not finished / blockers:**
- None for F001; ready to begin F002 (Postgres schema, daily-partitioned events, RLS policies, and bootstrap CLI)

### 2026-11-21 — Agentway — Initial setup
**Status at end of session:** done

**Done:**
- dev-harness template installed into `.harness/` by Agentway
  (detected stack: rust (from project type))

**Verification run:**
- No project code changed at this stage (scaffolding only)

**Key decisions & why:**
- Plain text files (not a database) so every coding assistant can read them
  without extra dependencies

**Not finished / blockers:**
- Check the verification commands in `.harness/AGENTS.md` section 3 — they were
  detected automatically; adjust any that do not match the real project
