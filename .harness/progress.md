# Progress Log

> Add new entries at the TOP (newest first). Every entry must be short,
> factual and verifiable (not opinion).
> Keep about the 10 newest entries here: when there are more, move the oldest
> ones (unchanged) to the top of `.harness/progress-archive.md` — never delete
> them. Each session reads this file, so a long log wastes context.

---

## Entry template (copy to the top for a new entry)

```
### [DATE] — [ASSISTANT: Claude Code / Codex / Cursor / ...] — [F00X: feature title]

**Status at end of session:** in-progress / done / blocked

**Done:**
- ...

**Verification run:**
- `<command>` → PASS/FAIL (short result)

**Security notes:** (only when input/auth/secrets/files/network/deps were touched)
- ...

**Key decisions & why:**
- ...

**Not finished / blockers:**
- ...
```

---

### 2026-11-21 — Antigravity — F005: Ingest worker pool: default PII scrubbing, fingerprinting, issue upsert/reopen, capped events, batch insert

**Status at end of session:** done

**Done:**
- Implemented `crates/ingest/src/scrub.rs`: depth-bounded recursive baseline PII scrubbing (`[Filtered]`) for sensitive headers (`Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key`, etc., in both object and `[key, value]` tuple-array formats), cookies, sensitive fields (`password`, `token`, `secret`, `api_key`, `client_secret`, `access_token`, etc.), sensitive query parameters in `query_string` / `url`, and `Bearer`/`Basic` auth values before persistence
- Implemented `crates/ingest/src/fingerprint.rs`: Sentry event normalization and deterministic SHA-256 fingerprinting based on exception type + in-app stack frames (`module`/`filename` + `function`, stripping Rust compiler hash suffixes), with fallback to normalized error message (UUID/hex/number tokenization), plus custom `fingerprint` array support, `culprit` extraction, and tag normalization
- Implemented `crates/ingest/src/worker.rs`: async batch worker (`spawn_ingest_worker`, flushing every 100ms–1s or 256 events) and transactional `flush_processed_batch` (`begin_worker_tx`): checks `to_regclass` before `ensure_events_partition`, upserts `issues` in deterministic order, auto-reopens `resolved` issues on regression (`status = 'unresolved'`, `is_regressed = TRUE`), caps full event payloads at `MAX_FULL_EVENTS_PER_ISSUE` (default 50) + sample (`is_sample = TRUE`), and executes multi-row batch inserts/upserts into `events` and `issue_hourly_counters`
- Wired `spawn_ingest_worker` into `crates/server` (`AppState::new` and `AppState::with_login_limiter`)
- Added PostgreSQL 16 integration test `crates/server/tests/ingest_worker.rs` verifying live background worker PII scrubbing, grouping consistency, regression reopen, `ignored` status retention, full-event cap + regression sample, and hourly counter aggregation

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (34 passed: 7 server unit + 3 server integration + 5 auth unit + 9 ingest unit + 1 org unit + 8 platform unit + 1 platform RLS integration)
- `cargo build --workspace` → PASS
- Manual smoke check (live `maxi-loop serve` + login + project creation + 2 gzipped envelopes with `Authorization` & `password` PII → verified grouped issue `event_count = 2`, `[Filtered]` in `events.payload`, and `issue_hourly_counters` count = 2 + clean SIGTERM) → PASS

**Security notes:**
- Self-review passed: PII scrubbing executes before any field extraction or database persistence so raw credentials (`Authorization`, `Cookie`, `password`, `token`, `secret`, `api_key`, URL query secrets) never reach PostgreSQL (`events.payload`, `events.tags`, or `issues`); recursion bounded by `MAX_SCRUB_DEPTH = 32`; parameterized SQLx `QueryBuilder` binds inside `begin_worker_tx`; negative tests verify zero raw secret substrings in persisted rows
- Dependencies: added `sha2.workspace = true` to `crates/ingest` (already in workspace lockfile, 0 new external crates)

**Key decisions & why:**
- Check `SELECT to_regclass($1) IS NOT NULL` before calling `ensure_events_partition` in `flush_processed_batch` so the 100ms ingest hot path avoids executing partition DDL (`AccessExclusiveLock`) when the daily partition already exists

**Not finished / blockers:**
- Ready to begin F006 (Issues query, detail, hourly stats, and triage JSON API with tenant isolation tests)

### 2026-11-21 — Antigravity — F004: Sentry Envelope ingest endpoint with DSN cache, decompression, rate limiting, and bounded channel

**Status at end of session:** done

**Done:**
- Created `crates/ingest` (`maxi-loop-ingest`):
  - `cache.rs`: thread-safe in-memory `DsnCache` preloaded from PostgreSQL at startup and updated live via `ProjectDsnObserver` on project creation/DSN rotation, with constant-time `subtle::ConstantTimeEq` key verification (0 per-envelope DB queries) and per-project `governor` GCRA rate limiters
  - `decompress.rs`: streaming decompression for `gzip`, `deflate` (RFC 1950 zlib + raw RFC 1951 fallback), and `br` (brotli) with bounded `Read::take` enforcement of `MAX_ENVELOPE_COMPRESSED_BYTES` and `MAX_ENVELOPE_DECOMPRESSED_BYTES` (zip-bomb protection)
  - `envelope.rs`: Sentry Envelope parser supporting `X-Sentry-Auth`, `?sentry_key=`, and envelope header `dsn` authentication, early dropping of unsupported item types (`session`, `replay_*`, `profile`, `attachment`, etc.) without deserializing their payloads, `MAX_EVENT_PAYLOAD_BYTES` enforcement, and lightweight event metadata extraction
  - `http.rs`: `POST /api/{project_id}/envelope/` (and `/api/{project_id}/envelope`) handler pushing accepted `QueuedEnvelope` batches into a bounded `tokio::sync::mpsc` channel and returning `429 Too Many Requests` with `Retry-After` and `X-Sentry-Rate-Limits` headers on per-project quota exhaustion or full-channel backpressure
- Wired `DsnCache` and `crates/ingest` router into `crates/server` and connected `crates/org` project create/rotate handlers via `ProjectDsnObserver`
- Added PostgreSQL 16 integration test `crates/server/tests/envelope_ingest.rs` verifying in-memory DSN auth (0 DB queries, live DSN rotation invalidation), gzip/deflate/brotli decompression, zip-bomb & oversized event rejections (`413`), early drop of unsupported items, per-project `governor` rate limiting (`429`), and bounded channel full backpressure (`429`)

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (29 passed: 7 server unit + 2 server integration + 5 auth unit + 5 ingest unit + 1 org unit + 8 platform unit + 1 platform RLS integration)
- `cargo build --workspace` → PASS
- Manual smoke check (live `maxi-loop` server preloading DSN cache + gzipped mixed envelope `POST /api/{project_id}/envelope/` returning `{"accepted_items":1,"dropped_items":1}` + clean SIGTERM) → PASS

**Security notes:**
- Self-review passed: fast-path `X-Sentry-Auth` / `?sentry_key=` authentication runs in memory before decompression so unauthorized callers cannot burn decompression CPU or trigger DB queries; streaming `Read::take(max_decompressed_bytes + 1)` prevents gzip/deflate/brotli decompression bombs; `MAX_EVENT_PAYLOAD_BYTES` checked before slicing/parsing event JSON; constant-time DSN key check; negative tests cover invalid/rotated DSN keys, unknown project IDs, gzip decompression bombs (`413`), oversized event items (`413`), per-project rate limit (`429`), and full-channel backpressure (`429`)
- New dependencies: `flate2` v1.1 (maintained by rust-lang, required by PRD F-ING-2 for gzip/deflate decompression), `brotli` v7.0 (standard pure-Rust Brotli implementation, required by PRD F-ING-2 for `Content-Encoding: br`), and `governor` v0.8 (GCRA rate limiter explicitly specified in PRD §11, Design §5.1/5.9, and F004 acceptance criteria). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Introduced `ProjectDsnObserver` in `crates/platform` so `crates/org` notifies `DsnCache` in memory whenever a project is created or its DSN is rotated, keeping `crates/org` decoupled from `crates/ingest` while guaranteeing 0 DB queries on the ingest hot path

**Not finished / blockers:**
- Ready to begin F005 (Ingest worker pool: default PII scrubbing, fingerprinting, issue upsert/reopen, capped events, batch insert)

### 2026-11-21 — Antigravity — F003: Session auth (Argon2id + CSRF) and Tenant/Workspace/Project/DSN management API

**Status at end of session:** done

**Done:**
- Created `crates/auth` (`maxi-loop-auth`): Argon2id login/logout/me (`/api/v1/auth/{login,logout,me}`) with timing-safe dummy Argon2id verification on unknown emails, 256-bit CSPRNG session tokens stored only as SHA-256 hashes in PostgreSQL `sessions`, `HttpOnly; SameSite=Lax` cookies, constant-time `X-CSRF-Token` validation automatically enforced by `AuthSession` extractor on all state-changing HTTP methods, and bounded in-memory `LoginRateLimiter` (`429 Too Many Requests` + `Retry-After`)
- Created `crates/org` (`maxi-loop-org`): RLS-enforced tenant/workspace/project/DSN management service (`begin_tenant_tx` + `TenantContext`), 128-bit CSPRNG Sentry DSN generation & rotation, retention override validation (`<= global` per `D003`), role authorization (`tenant_owner`/`tenant_admin`/`workspace_admin` vs `member`/`viewer`), slug-based hierarchy routes (`/api/v1/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}`), and single-tenant convenience routes (`/api/v1/tenants/current`, `/api/v1/workspaces`, `/api/v1/projects`)
- Extended `crates/platform` (`Config` + `AppError`) with `PUBLIC_BASE_URL`, `SINGLE_TENANT_MODE`, `COOKIE_SECURE`, `SESSION_TTL_HOURS`, and multi-tenant slug lookup in `bootstrap_defaults`
- Added PostgreSQL 16 integration test `crates/server/tests/auth_org_isolation.rs` covering login/me/logout, raw token non-persistence, rate limiting, CSRF rejection, role authorization, retention override caps, DSN creation/rotation, and Tenant A vs Tenant B cross-tenant isolation

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (23 passed: 7 server unit + 1 server auth/org/isolation integration + 5 auth unit + 1 org unit + 8 platform unit + 1 platform RLS integration)
- `cargo build --workspace` → PASS
- Manual smoke check (`maxi-loop bootstrap` + live HTTP server login → me → create project → rotate DSN → logout → verify 401 after logout + clean SIGTERM) → PASS

**Security notes:**
- Self-review passed: all inputs (email, password, slugs, names, platform, retention overrides, rate limits, `PUBLIC_BASE_URL`) validated server-side; parameterized SQLx queries inside `begin_tenant_tx` (Postgres RLS `maxi_loop_app` + explicit `tenant_id` scoping); server-side role & tenant checks on every endpoint; raw session tokens never stored in DB (SHA-256 hash only) or logged; generic `invalid credentials` + dummy Argon2id hash prevents user enumeration; constant-time `subtle::ConstantTimeEq` CSRF verification on all state-changing cookie-authenticated requests; negative tests cover unauthenticated access, wrong password/unknown user, login rate limit, missing/invalid CSRF token, unauthorized viewer role, invalid retention override, and cross-tenant read/mutation attempts
- Dependencies: promoted `sha2` v0.10, `subtle` v2, and `hex` v0.4 (all three already present in `Cargo.lock` via `sqlx` and `password-hash`, adding 0 new external crates). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Enforced `X-CSRF-Token` validation directly inside `AuthSession`'s `FromRequestParts` implementation for all non-`GET`/`HEAD`/`OPTIONS` requests so state-changing cookie-authenticated handlers cannot accidentally omit CSRF verification

**Not finished / blockers:**
- Ready to begin F004 (Sentry Envelope ingest endpoint with DSN cache, decompression, rate limiting, and bounded channel)

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
