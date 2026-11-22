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

### 2026-11-22 — Antigravity — F016: P2 Performance & MCP: Compare releases, slow DB queries/operations aggregation, MCP list_slow_queries/compare_releases & prompt templates

**Status at end of session:** done

**Done:**
- Created `migrations/0009_p2_span_operations.sql` adding `span_minute_metrics` (`PRIMARY KEY (tenant_id, project_id, op, description_hash, environment, release, minute_bucket)` with 76-bucket duration histograms and optional `sample_trace_id`) with `ENABLE`/`FORCE ROW LEVEL SECURITY` (`maxi_loop_app` + `maxi_loop_worker`)
- Extended `crates/performance`:
  - `normalize.rs`: zero-regex linear-time span description normalizer (SQL comments/strings/numbers/UUIDs/`$N` params/`IN (?, ...)` → `?`; HTTP client query/fragment stripping + numeric/UUID path `:id`; cache/Redis numeric/UUID key normalization) + SHA-256 `description_hash`
  - `ingest_proc.rs`: extracts normalized span operations from ALL ingested transactions (sampled or unsampled) and batch-upserts 1-minute `span_minute_metrics` buckets (preserving `sample_trace_id` when sampled)
  - `service.rs` (`list_slow_operations`) & `compare.rs` (`compare_releases`): RLS-enforced slow DB queries/operations aggregation (`p50`/`p95`/`p99`/`avg`/`total_ms`/`count`/`error_rate`, `db_only`/op/query/sort filters) and release comparison (`base_release` vs `target_release` across latency percentiles, throughput RPM, error rate, per-transaction deltas/statuses, linked/new/regressed issue counts, top newly introduced issues, and overall verdict)
  - `http.rs`: mounted `/api/v1/performance/slow-operations`, `/api/v1/performance/compare-releases`, project convenience routes, and `/t/{tenant}/w/{workspace}/p/{project}/...` slug routes with API token scoping
- Extended `crates/retention` (`model.rs`, `service.rs`) to clean expired `span_minute_metrics` rows during both shorter project-override and global retention cycles
- Extended `crates/mcp` (preserving **zero direct `sqlx` dependency**, `D006`):
  - `tools.rs` (`F-MCP-14`): added read-scoped `list_slow_queries` (defaults to `db_only: true`, includes `sample_trace_id` & UI deep link) and `compare_releases` (output PII-scrubbed, `<<<UNTRUSTED_EVENT_DATA>>>` wrapped, truncation markers)
  - `prompts.rs`, `protocol.rs`, `http.rs` (`F-MCP-15`): advertised `capabilities.prompts` on `initialize`, implemented `prompts/list` & `prompts/get` (audit-logged & rate-limited) with 4 curated templates (`triage_issue`, `fix_issue`, `triage_release_regression`, `investigate_slow_queries`)
- Updated MaxiBoard React UI (`web/src/lib/api.ts`, `web/src/modules/performance/SlowOperationsView.tsx`, `CompareReleasesView.tsx`, `PerformanceView.tsx`, `web/src/app/App.tsx`) with **Slow Queries & Ops** and **Compare Releases** sub-tabs + `?view=performance&tab=slow-queries|compare` deep links
- Added PostgreSQL 16 integration test `crates/server/tests/p2_performance_mcp.rs` and updated `crates/server/tests/mcp_server.rs` `tools/list` assertions (9 read / 11 total); archived `F009` to `.harness/progress-archive.md` and updated `.harness/ARCHITECTURE.md`

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo check --workspace` → PASS
- `cargo test --workspace` → PASS (89/89 passed across 39 suites, incl. 13 `maxi-loop-performance`, 7 `maxi-loop-mcp`, and `p2_performance_mcp` Postgres 16 integration test)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- `cargo tree --depth 1 -p maxi-loop-mcp` → PASS (0 direct `sqlx` dependency)
- Live smoke check on restarted `maxi-loop-dev.service` (port `8080`) → PASS (unauth `401`, normalized SQL `SELECT * FROM smoke_items WHERE id = ? AND state = ?`, compare-releases verdict `regressed`, MCP `initialize`/`prompts/list`/`prompts/get`/`list_slow_queries`/`compare_releases` + token revocation `401`, embedded SPA `200`)

**Security notes:**
- Self-review passed: all query/compare/prompt arguments validated server-side (slugs, non-whitespace distinct release versions, allow-listed sort keys, escaped `ILIKE`, 90-day time-window cap, positive integer/no-control-char prompt args); zero-regex O(n) SQL/URL normalization eliminates ReDoS risk; all DB access runs inside parameterized `begin_tenant_tx` (Postgres RLS `maxi_loop_app` + `span_minute_metrics` RLS); API token & MCP scope boundaries enforced on every endpoint/tool/prompt; MCP outputs scrub baseline + custom PII and wrap untrusted text in `<<<UNTRUSTED_EVENT_DATA>>>` with `UNTRUSTED_SECURITY_NOTICE`; negative tests verify unauth (`401`), identical-release compare (`400`), cross-project API token (`403`) & MCP tool/prompt denial, invalid prompt args, and cross-tenant isolation (`404`/empty)
- Dependencies: 0 new external Rust or npm dependencies added. `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Aggregated `span_minute_metrics` from ALL ingested transactions (not just sampled traces) keyed by `(op, SHA-256(normalized_description))` with 76-bucket log histograms so p50/p95/total_ms remain accurate under low trace sampling rates while keeping PKs compact and storage bounded via retention cleanup

**Not finished / blockers:**
- All roadmap features (`F001`–`F016`, Phases 1–5 + all PRD P2 features) are now 100% complete!

### 2026-11-22 — Antigravity — F015: P2 Issues: Manual issue merge/split (with fingerprint alias routing) and event breadcrumbs viewer

**Status at end of session:** done

**Done:**
- Created `migrations/0008_p2_issue_merge_split.sql` adding backward-compatible nullable `events.fingerprint TEXT` (across daily partitions) and `issue_fingerprints` (`PRIMARY KEY (tenant_id, project_id, fingerprint)`) with `ENABLE`/`FORCE ROW LEVEL SECURITY` (`maxi_loop_app` + `maxi_loop_worker`)
- Updated `crates/ingest/src/worker.rs` to persist each event's original computed `fingerprint` on `events` and resolve merged fingerprint aliases via `issue_fingerprints` so future incoming envelopes for merged fingerprints automatically route to the target issue (preserving the target issue's title/exception metadata)
- Added `crates/issues/src/breadcrumbs.rs` (`F-ISS-10`) extracting & normalizing Sentry event breadcrumbs (`breadcrumbs.values` or flat array, normalizing numeric epoch & ISO-8601 timestamps, level, type, category, message, data; bounded to 200 most recent) and exposed `breadcrumbs` on `IssueEventView` & `IssueDetailView`
- Added `crates/issues/src/merge_split.rs` and HTTP routes (`POST /api/v1/issues/merge`, `POST /api/v1/issues/{issue_id}/split`, plus `/t/{tenant}/w/{workspace}/p/{project}/...` slug routes) (`F-ISS-9`):
  - Transactional `merge_issues` (deterministic `ORDER BY id ASC FOR UPDATE` locking, same-project enforcement, re-parenting `events`, `issue_notes`, `alert_deliveries`, merging `issue_hourly_counters` & `issue_releases`, recording `issue_fingerprints` aliases, audit note, deleting merged sources)
  - Transactional `split_issue` supporting both merged-fingerprint detachment (`fingerprints`) and arbitrary event splitting (`event_ids`), re-allocating events/counters/releases, refreshing `sample_event_id`, removing detached aliases so future events route to the new standalone issue, and recording audit notes
- Updated MaxiBoard React UI (`web/src/lib/api.ts`, `web/src/modules/issues/BreadcrumbsView.tsx`, `IssuesView.tsx`, `IssueDetailDrawer.tsx`, `IssueDetailFullView.tsx`):
  - Structured chronological **Breadcrumbs** timeline (level/text filter, type/category icons, expandable scrubbed `data` JSON) in full view, expandable stored-events inspector, and compact preview in the right drawer
  - Multi-select checkboxes + **Merge issues** toolbar in `IssuesView.tsx` (enforcing same-project selection and primary target selector)
  - **Merged fingerprints** panel & **Split into new issue** action in `IssueDetailFullView.tsx` plus checkbox multi-select **Split selected events** inside the **Stored events** tab
- Added PostgreSQL 16 integration test `crates/server/tests/p2_merge_split_breadcrumbs.rs` covering breadcrumbs + PII scrubbing, multi-issue merge, post-merge alias ingest routing, split by fingerprint + post-split routing, split by `event_ids`, RBAC (`viewer` `403`), API token scoping, cross-project denial (`400`), and Tenant A vs Tenant B isolation (`404`)
- Archived `F008` to `.harness/progress-archive.md` and updated `.harness/ARCHITECTURE.md`

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (82/82 passed, incl. 5 `maxi-loop-issues` unit tests & `p2_merge_split_breadcrumbs` Postgres 16 integration test)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- `cargo tree --depth 1 -p maxi-loop-mcp` → PASS (0 direct `sqlx` dependency)
- Live manual smoke check on restarted `maxi-loop-dev.service` (port `8080`) → PASS (unauth merge `401`, breadcrumbs normalized with `Authorization`/`password` `[Filtered]`, merge → post-merge alias ingest routing `3 events` → split `1+2`, embedded SPA `200`)

**Security notes:**
- Self-review passed: all merge/split inputs validated server-side (`target_issue_id > 0`, `source_issue_ids` 1..=50 non-self deduped, mutually exclusive `fingerprints` 1..=20 or `event_ids` 1..=50, cannot split all events out); all queries parameterized inside `begin_tenant_tx` (Postgres RLS `maxi_loop_app` + `issue_fingerprints` RLS); every endpoint enforces `AuthSession` + CSRF + `enforce_api_token_write()` + `enforce_project_write()` + `can_triage_issues()` + same-project check; breadcrumbs are derived strictly from PII-scrubbed event payloads (`[Filtered]`); negative tests verify unauth (`401`), viewer (`403`), read-only API token (`403`), cross-project merge (`400`), cross-project API token split denial, cross-tenant merge/split (`404`), self-merge (`400`), and splitting all events (`400`)
- Dependencies: 0 new external Rust or npm dependencies added (promoted workspace `sha2` & `hex` in `crates/issues`). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Stored secondary merged fingerprints in a dedicated RLS-protected `issue_fingerprints` table (plus `events.fingerprint`) rather than mutating `issues.fingerprint` so ingest lookup stays fast (direct PK hit on unmerged issues, single PK alias lookup for merged fingerprints) and split can accurately restore original fingerprints and events

**Not finished / blockers:**
- Ready to continue P2 with `F016` (Compare releases `F-PRF-6`, Slow DB queries/operations `F-PRF-7`, and MCP `list_slow_queries`/`compare_releases` `F-MCP-14` + prompt templates `F-MCP-15`)

### 2026-11-22 — Antigravity — Browser Ingest CORS & Official React SDK (`@sentry/react`) Headless Chrome E2E

**Status at end of session:** done

**Done:**
- Fixed browser cross-origin ingest in `crates/ingest/src/http.rs`: added `OPTIONS` preflight handler (`204 No Content`) and non-credentialed CORS middleware strictly scoped to `/api/{project_id}/envelope/` (`Access-Control-Allow-Origin: *`, `Access-Control-Allow-Methods: POST, OPTIONS`, `Access-Control-Allow-Headers: Content-Type, Content-Encoding, X-Sentry-Auth`, `Access-Control-Expose-Headers: X-Sentry-Rate-Limits, Retry-After`, `Access-Control-Max-Age: 86400`, explicitly omitting `Access-Control-Allow-Credentials`), keeping cookie-authenticated `/api/v1/*` routes strictly same-origin
- Extended `crates/server/tests/envelope_ingest.rs` with CORS preflight (`204`), `POST` CORS/expose-header assertions, no-credentials assertion, and negative verification that `/api/v1/health` never emits `Access-Control-Allow-Origin`
- Created isolated production-built Vite + React + TypeScript E2E app under `tests/e2e/react/` (`package.json` with `@sentry/react` ^9, `vite.config.ts` emitting minified JS + `.js.map`, `src/checkout.ts`, `src/App.tsx`, `src/main.tsx`, `README.md`) and real Headless Chrome orchestrator `tests/e2e/verify_react_sdk.py`
- Updated `.gitignore` for `/tests/e2e/react/node_modules/` & `/tests/e2e/react/dist/`, archived `F007` to `.harness/progress-archive.md`

**Verification run:**
- `npm --prefix tests/e2e/react run build` → PASS (`tsc --noEmit && vite build`, minified bundle + `1.87 MB` `.js.map`)
- `npm --prefix tests/e2e/react audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (77/77 passed, incl. `envelope_ingest` CORS & `/api/v1/*` non-CORS assertions)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- Live Headless Chrome E2E (`tests/e2e/verify_react_sdk.py` against restarted `maxi-loop-dev.service` on port `8080`) → PASS (204 CORS preflight on `/api/{id}/envelope/` & absent on `/api/v1/health`; Vite `.js.map` uploaded via `mlapi_...` write token; 4 minified errors → 2 grouped issues `TypeError` ×3 + `RangeError` ×1 resolved back to `src/checkout.ts` with `context_line`/`pre_context`/`post_context` + `node_modules/` marked `in_app=False` + `[Filtered]` PII; Browser React → Backend Django distributed waterfall `6 spans` across 2 services `p50=605.0ms`; tampered DSN in Chrome: 0 issues & 0 transactions)

**Security notes:**
- Self-review passed: wildcard CORS (`Access-Control-Allow-Origin: *`) is attached exclusively to the non-credentialed public DSN ingest route `/api/{project_id}/envelope/` and NEVER sets `Access-Control-Allow-Credentials`; cookie-authenticated `/api/v1/*` and `/mcp` routes remain untouched (verified by negative test on `/api/v1/health`); React E2E injects ephemeral DSN/secrets dynamically via local `/e2e-config.js` (never baked into committed code), static file handler validates `Path.resolve().is_relative_to(dist_dir)`, Chrome/backend spawned with argument arrays (`shell=False`), and negative check verifies tampered DSN ingests 0 issues & 0 transactions
- Dependencies: 0 new production Rust or `web/` npm dependencies added; isolated test-only `@sentry/react` in `tests/e2e/react/` (`npm audit` = 0 vulnerabilities). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Implemented ingest CORS via a lightweight 20-line `axum::middleware::from_fn` directly on `crates/ingest/src/http.rs` (0 new crates) rather than a global CORS layer so cookie-authenticated `/api/v1/*` endpoints can never accidentally inherit permissive CORS

**Not finished / blockers:**
- All 5 target PRD SDK stacks (**Python FastAPI, Python Django, Go, Rust, React/Browser**) are now 100% live-verified E2E for both Error Tracking and Performance Monitoring / Distributed Tracing! Ready for user's next choice (production `Dockerfile`/`README.md` or PRD P2 features)

### 2026-11-22 — Antigravity — FastAPI & Django SDK E2E upgrade (Error + Performance + Distributed Trace) & OTel status-code parsing

**Status at end of session:** done

**Done:**
- Hardened `crates/performance/src/ingest_proc.rs` (`extract_http_status_code`) to also inspect nested OpenTelemetry/Sentry 2.x `contexts.trace.data["http.response.status_code"]` / `contexts.trace.data["http.status_code"]`, with unit test `detects_5xx_error_from_nested_otel_trace_data_status_code`
- Upgraded `tests/e2e/python/verify_sentry_sdk.py` and `tests/e2e/python/README.md` to test official Python `sentry-sdk==2.75.0` with **FastAPI** (`FastApiIntegration`, `fastapi==0.165.0`) and **Django** (`DjangoIntegration`, `django==5.2.19`, real WSGI handler via `get_wsgi_application()`) for **both** Error Tracking AND Performance Monitoring (`traces_sample_rate=1.0`):
  - Error grouping (`ZeroDivisionError` ×3 + `ValueError` ×1; `RuntimeError` ×2 + `KeyError` ×1), `in_app` frames + `context_line`, `Authorization`/`Cookie` PII scrubbing
  - 1-minute duration histograms (`p50_ms >= 500ms`), error transaction rate (`3/3` FastAPI, `2/2` Django), sampled zstd trace span PII scrubbing (`password`, `api_key` → `[Filtered]`), per-project API token read access (`mlapi_...`)
  - Real cross-service **FastAPI → Django** distributed trace waterfall (`sentry-trace` + `baggage` propagation, verifying exact `parent_span_id` linkage & 7 spans across 2 services)
  - Negative tampered-DSN check confirming `0` issues AND `0` transactions ingested
- Archived `F006` (and removed duplicate `F005`) from `.harness/progress.md` to `.harness/progress-archive.md`

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (77/77 passed, incl. 9/9 `maxi-loop-performance` unit tests)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- Live official Python `sentry-sdk==2.75.0` E2E (`tests/e2e/python/verify_sentry_sdk.py` against restarted `maxi-loop-dev.service` on port `8080`) → PASS (FastAPI `p50=584.2ms` + `3/3` errors + 2 grouped issues; Django `p50=550.9ms` + `2/2` errors + 2 grouped issues; 2-service distributed waterfall `7 spans` FastAPI=3/Django=4 + `[Filtered]` PII; tampered DSN: 0 issues & 0 transactions)

**Security notes:**
- Self-review passed: no untrusted SQL/shell/path changes; `extract_http_status_code` safely parses bounded `u16` from JSON without panics; Python E2E generates ephemeral runtime secrets via `secrets.token_hex`, passes DSN/secrets via env (never argv), uses argument arrays (`shell=False`), verifies `[Filtered]` PII scrubbing in both issues and sampled zstd trace waterfalls, and includes negative tampered-DSN verification for both events and transactions
- Dependencies: 0 new production or test dependencies added

**Key decisions & why:**
- Switched the Django E2E worker from `django.test.Client` to real WSGI (`get_wsgi_application()`) so `DjangoIntegration` exercises the full production WSGI transaction, signal span, and `sentry-trace` propagation pipeline

**Not finished / blockers:**
- All SDK stacks tested so far (Python FastAPI/Django, Go, Rust) pass live Error + Performance E2E. Remaining optional next work: React SDK (`@sentry/react`) E2E, production Docker/deployment docs, or P2 features from PRD §7

### 2026-11-22 — Antigravity — F014: Phase 5: Go & Rust SDK verification, multi-user RBAC, inherited settings, API tokens, aggregate tenant limits

**Status at end of session:** done

**Done:**
- Created `migrations/0007_phase5_rbac_settings_tokens.sql` adding inherited rate-limit columns (`tenants.default_rate_limit_per_minute`, `tenants.aggregate_rate_limit_per_minute`, `workspaces.default_rate_limit_per_minute_override`, `projects.rate_limit_per_minute_override` with backward-compatible backfill) and per-project `api_tokens` table (SHA-256 hash only, `can_read`/`can_write`, `expires_at`, `revoked_at`, `last_used_at`, leading `(tenant_id, project_id)` PK) with `ENABLE`/`FORCE ROW LEVEL SECURITY` (`maxi_loop_app` + `maxi_loop_worker`)
- Implemented **Multi-User RBAC (`F-ADM-5`)** in `crates/auth`:
  - Extended `AuthSession` (`model.rs`, `service.rs`) with explicit `tenant_role: Option<Role>` and per-workspace `workspace_roles: BTreeMap<i64, Role>` so workspace-scoped admins/members/viewers are enforced per workspace (never leaking permissions to sibling workspaces)
  - Created `crates/auth/src/members.rs` (`/api/v1/members` `GET`/`POST`, `PATCH`/`DELETE /api/v1/members/{membership_id}`) with Argon2id hashing for new accounts (never overwriting an existing user's password across tenants), privilege-hierarchy enforcement (only `tenant_owner` can grant/modify/remove `tenant_owner`; `workspace_admin` limited to managed workspaces), last-`tenant_owner` protection, and immediate session revocation when a user's last tenant membership is removed
- Implemented **Per-Project API Tokens (`F-ADM-4`)** in `crates/auth/src/api_tokens.rs`:
  - 256-bit CSPRNG `mlapi_<64-hex>` tokens stored strictly as SHA-256 hashes (`/api/v1/api-tokens` `GET`/`POST`, `DELETE /api/v1/api-tokens/{token_id}`), supporting `Authorization: Bearer mlapi_...` in `AuthSession` without CSRF
  - Enforced strict single-project scoping + `can_write` checks across `org`, `issues`, `performance`, and `sourcemaps` (enabling CI source map upload & release/issue automation), while failing closed (`403 Forbidden` via `require_user_session()`) on all admin, member, settings, alerts, PII, and MCP-token endpoints
- Implemented **Inherited Settings (`F-ADM-8`)** and **Aggregate Tenant Rate Limits (`F-ADM-9`)**:
  - Extended `crates/org` with `PATCH /api/v1/tenants/current/settings`, `PATCH /api/v1/workspaces/{slug}/settings`, and `PATCH /api/v1/projects/{slug}/settings` (plus `/t/{tenant}/...` slug routes), validating retention bounds & rate limits and propagating inherited changes live via `ProjectDsnObserver::on_tenant_rate_limits_updated`
  - Extended `crates/ingest/src/cache.rs` & `http.rs` with per-tenant aggregate `governor` token buckets checked alongside per-project buckets (0 per-envelope DB queries), returning `429 Too Many Requests` + `Retry-After` + `X-Sentry-Rate-Limits: <secs>:<category>:organization` when the aggregate tenant cap is exceeded
  - Enhanced `crates/ingest/src/fingerprint.rs` with `threads.values[].stacktrace` and `frame.package` fallbacks for Go & Rust SDK stack traces
- Added MaxiBoard UI `web/src/modules/admin/TeamAndSettingsView.tsx` (**Team & settings** sidebar tab with Members & RBAC, Inherited Settings & Aggregate Limits, and Project API Tokens + one-time copyable token & `curl` CI snippet), plus inline inherited-override editing and Python/Go/Rust SDK quickstarts in `ProjectsView.tsx`
- Added PostgreSQL 16 integration test `crates/server/tests/phase5_rbac_settings_tokens.rs` and isolated official SDK E2E suites `tests/e2e/go/` (`github.com/getsentry/sentry-go` v0.31.1), `tests/e2e/rust/` (`sentry` v0.34.0 + `sentry-tracing`), and `tests/e2e/verify_go_rust_sdks.py`

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (76/76 passed, incl. `phase5_rbac_settings_tokens`, `api_tokens` unit test, and `DsnCache` aggregate tenant limit unit test)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- `cargo tree --depth 1 -p maxi-loop-mcp` → PASS (0 direct `sqlx` dependency)
- Live official Go & Rust Sentry SDK E2E (`tests/e2e/verify_go_rust_sdks.py` against restarted `maxi-loop-dev.service` on port `8080`) → PASS (`Go sentry-go`: 2 grouped issues `[3, 1]` + stack frames + `[Filtered]` PII + `GET /api/go/orders/{id}` p50=40.9ms + 3-span waterfall; `Rust sentry + tracing`: 2 grouped issues `[3, 1]` + stack frames + `[Filtered]` PII + `POST /api/rust/billing` p50=40.2ms + 3-span waterfall; tampered-DSN negative test: 0 events ingested; unauth `/api/v1/members` & `/api/v1/api-tokens` `401`)

**Security notes:**
- Self-review passed: all member, role, setting, and API token inputs validated server-side; all tenant queries run inside `begin_tenant_tx` (Postgres RLS `maxi_loop_app` + explicit `tenant_id`); privilege escalation blocked (`tenant_admin` cannot grant/modify `tenant_owner`, `workspace_admin` cannot access sibling workspaces, last `tenant_owner` protected, removed users' sessions revoked immediately); attaching an existing email never modifies their password; per-project API tokens (`mlapi_<64-hex>`) stored strictly as SHA-256 hashes, shown once, confined to one project, read-only by default, and fail-closed (`403`) on all admin/settings/alert/PII/MCP-token endpoints; negative tests cover privilege escalation, cross-workspace denial, cross-project token denial, admin-route token denial, token revocation (`401`), aggregate tenant `429`, tampered DSNs, and Tenant A vs Tenant B cross-tenant isolation
- Dependencies: 0 new production Rust or npm dependencies added (`github.com/getsentry/sentry-go` v0.31.1 and `sentry` v0.34.0 live strictly in isolated `tests/e2e/{go,rust}/`). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Checked per-project rate limit before aggregate tenant rate limit in `DsnCache::check_rate_limit` so a noisy single project exceeding its own cap cannot starve sibling projects' shared tenant aggregate quota

**Not finished / blockers:**
- All roadmap features (`F001`–`F014`, Phases 1–5) are now 100% complete!

### 2026-11-22 — Antigravity — F013: Phase 4: React Source Maps upload per release and bounded LRU ingest-time resolution

**Status at end of session:** done

**Done:**
- Created `migrations/0006_phase4_sourcemaps.sql` adding `source_maps` (leading `(tenant_id, project_id)` primary key, unique `(tenant_id, project_id, release_version, filename)`, partial index on `(tenant_id, project_id, debug_id)`, zstd-compressed `content_zstd BYTEA`, SHA-256 digest, raw/compressed sizes) with `ENABLE`/`FORCE ROW LEVEL SECURITY` (`maxi_loop_app` + `maxi_loop_worker`)
- Extended `crates/platform/src/config.rs` (and `.env.example`) with validated `SOURCEMAP_MAX_UPLOAD_BYTES` (default `16 MiB`), `SOURCEMAP_CACHE_MAX_BYTES` (default `64 MiB`), and `SOURCEMAP_CACHE_MAX_ENTRIES` (default `128`)
- Created vertical-slice crate `crates/sourcemaps` (`maxi-loop-sourcemaps`):
  - `codec.rs`: canonical JS bundle filename/URL normalization with strict path-traversal rejection (`..`, `.`, control chars, backslashes), Source Map v3 / Indexed Source Map content validation via `sourcemap::decode_slice`, automatic embedded `debugId` extraction, zstd compression, and bounded streaming decompression (`Read::take(max_raw_bytes + 1)`)
  - `cache.rs`: thread-safe `SourceMapLruCache` bounded by BOTH memory weight (`max_bytes`, conservatively 2x raw JSON size) AND entry count (`max_entries`), with negative caching, per-project invalidation on upload/delete, and hit/miss/eviction telemetry
  - `resolver.rs`: `SourceMapResolver` matching frames by `debug_id` (from `frame.debug_id` or `debug_meta.images`) first and `release + filename` (normalized path + basename fallback) second, resolving original TypeScript/React filenames, lines/cols, original function names, `pre_context`/`context_line`/`post_context` from embedded `sourcesContent`, and `in_app` (`false` for `node_modules/`)
  - `service.rs` & `http.rs`: RLS-enforced CRUD (`GET`/`POST /api/v1/sourcemaps`, `DELETE /api/v1/sourcemaps/{id}`, raw CI upload `POST /api/v1/projects/{slug}/releases/{version}/sourcemaps`, and slug-scoped list) with `can_triage_issues()` authorization, auto-registration of release rows, and live cache invalidation
- Wired `SourceMapResolver` into `crates/ingest` (`fingerprint.rs` + `worker.rs`) so minified JS/React stack frames are resolved **after PII scrubbing and BEFORE fingerprinting**, ensuring different minified bundle hashes for the same React component crash group into one issue
- Updated MaxiBoard UI `web/src/modules/releases/ReleasesView.tsx` and `web/src/lib/api.ts` with a **Source Maps** sub-tab (file picker `.js.map` upload, release/filename/Debug ID fields, zstd compression telemetry, and delete action)
- Added PostgreSQL 16 integration test `crates/server/tests/phase4_sourcemaps.rs` verifying RBAC (`viewer` `403`), invalid/traversal upload rejection (`400`), JSON + raw CI upload routes, grouping two distinct minified builds (`bundle-v1.js` + `bundle-v2.js`) into one single issue via pre-fingerprint resolution, release-independent `debug_id` resolution, bounded LRU cache hits, and Tenant A vs Tenant B cross-tenant isolation (API + ingest resolution)

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (73/73 passed, incl. 7 `maxi-loop-sourcemaps` unit tests & `phase4_sourcemaps` Postgres 16 integration test)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- Manual live smoke check (restarted `maxi-loop-dev.service` on port `8080`, verified unauth `401`, uploaded `.js.map` via `/api/v1/sourcemaps`, ingested minified React error envelope, confirmed resolved `src/App.tsx:2 in handleClick` + `context_line`, deleted smoke artifact) → PASS

**Security notes:**
- Self-review passed: uploaded source maps are validated by actual Source Map v3 parsing (`sourcemap::decode_slice`) and bounded by `SOURCEMAP_MAX_UPLOAD_BYTES` (`413` on oversize); filenames reject `..`/`.`/control/backslash characters; debug IDs restricted to 8..=64 hex/hyphen chars; all uploads/deletes require `AuthSession` + CSRF + `can_triage_issues()` (`viewer` rejected `403`); stored in PostgreSQL as zstd `BYTEA` with RLS (`maxi_loop_app` + `maxi_loop_worker`) rather than host filesystem paths; decompression bounded at `max_raw_bytes` against zip-bombs; LRU cache keys include `(tenant_id, project_id)` so cached maps never leak across tenants; negative tests cover `viewer` upload (`403`), malformed map (`400`), path traversal (`400`), oversized map (`413`), and cross-tenant list/delete/ingest isolation
- New dependency: `sourcemap` v9.3.2 (maintained by Sentry/getsentry, explicitly specified in PRD §11 and Design §5.7/§5.9 for Source Map v3 / Indexed Source Map parsing). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Implemented a zero-dependency memory-byte-bounded + entry-bounded `SourceMapLruCache` storing `Arc<DecodedMap>` (evicting when estimated RAM exceeds `SOURCEMAP_CACHE_MAX_BYTES`, skipping caching for single maps larger than the total budget) to directly mitigate the Design §5.7 large-sourcemap memory risk

**Not finished / blockers:**
- Ready to begin Phase 5 — `F014` (Go & Rust SDK verification, multi-user RBAC, inherited settings, per-project API tokens, and aggregate tenant rate limits)

### 2026-11-22 — Antigravity — F012: Phase 3: Performance monitoring (transactions, spans, histograms, sampled zstd traces, waterfall, MCP tools)

**Status at end of session:** done

**Done:**
- Created `migrations/0005_phase3_performance.sql` adding `merge_histogram_buckets(BIGINT[], BIGINT[])`, `sum_histogram_buckets(BIGINT[])` SQL aggregate, `transaction_minute_metrics` (1-minute aggregated counts, error counts, min/max/sum, and 76-bucket duration histograms), daily-partitioned `traces` (`spans_zstd BYTEA`, cross-project `(tenant_id, trace_id, start_timestamp)` index), `ensure_traces_partition(DATE)`, `drop_expired_traces_partitions(DATE)`, and `ENABLE`/`FORCE ROW LEVEL SECURITY` (`maxi_loop_app` + `maxi_loop_worker`)
- Extended `crates/platform` with validated `TRACE_SLOW_THRESHOLD_MS` (default `500`) and `TRACE_RANDOM_SAMPLE_RATE` (default `0.05`) config fields (plus `.env.example`) and `ensure_upcoming_traces_partitions`
- Created vertical-slice crate `crates/performance` (`maxi-loop-performance`):
  - `histogram.rs`: 76-bucket log-scale duration histogram (0.5ms to ~361s, factor 1.2) with `observe_duration`, `merge_histograms`, and linear-interpolated `estimate_percentile` (p50/p95/p99)
  - `codec.rs`: zstd trace span blob compression & streaming decompression bounded at `MAX_DECOMPRESSED_TRACE_BLOB_BYTES = 4 MiB` (decompression-bomb guard)
  - `ingest_proc.rs`: Sentry `transaction` parser, mandatory baseline + inherited custom PII scrubbing before storage, head-consistent deterministic sampling (`error`, `slow >= threshold`, and SHA-256(`trace_id`) 1–5% random so distributed services make identical sampling decisions), and batched worker flush (`transaction_minute_metrics`, `traces`, `releases`)
  - `waterfall.rs`: distributed cross-service trace waterfall assembler linking segments across projects via `trace_id` + `parent_span_id`, DFS ordering with `depth`, `offset_ms`, `self_duration_ms`, `pct_of_trace`, and `is_slow` / `slowest_spans` bottleneck detection
  - `service.rs` & `http.rs`: RLS-enforced `PerformanceService` facade and `/api/v1/performance/{transactions,transactions/detail,traces,traces/{trace_id}}` (+ slug-scoped) routes
- Extended `crates/ingest` to accept `"transaction"` Sentry envelope items alongside `"event"` items (still dropping unsupported items early) and flush transactions in 100ms worker batches
- Extended `crates/retention` to pre-create & `DROP TABLE` expired `traces_YYYY_MM_DD` partitions and run bounded batch deletes for shorter overrides & global cutoffs on `traces` and `transaction_minute_metrics`
- Extended `crates/mcp` (preserving **zero direct `sqlx` dependency**, `D006`) with `list_transactions` (`F-MCP-12`) and `get_trace` (`F-MCP-13`) enforcing token scope (`tenant`/`workspace`/`project`), output PII scrubbing, `<<<UNTRUSTED_EVENT_DATA>>>` wrapping, truncation markers, slow-span flagging, and UI deep links
- Added MaxiBoard UI `web/src/modules/performance/PerformanceView.tsx` and `TraceWaterfallView.tsx` (**Performance** sidebar tab, p50/p95/p99/RPM/error-rate table, latency trend & sampled traces inspector, interactive cross-service Gantt waterfall + span inspector, and `?view=performance&trace=...` deep links)
- Added PostgreSQL 16 integration test `crates/server/tests/phase3_performance.rs` covering transaction/span ingest, 1-minute histogram p50/p95/p99/throughput/error_rate, zstd frame storage + PII scrubbing, 2-service distributed waterfall (`web-frontend` → `orders-api`), MCP `list_transactions`/`get_trace` (scope isolation + post-ingest custom PII scrubbing), retention partition drop, and Tenant A vs Tenant B isolation; updated `crates/server/tests/mcp_server.rs` `tools/list` assertions from 5/7 to 7/9 tools to reflect the 2 new Phase 3 read tools

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (65/65 passed, incl. 8 `maxi-loop-performance` unit tests & `phase3_performance` Postgres 16 integration test)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- `cargo tree --depth 1 -p maxi-loop-mcp` → PASS (0 direct `sqlx` dependency)
- Manual live smoke check (restarted `maxi-loop-dev.service` on port `8080`, verified unauth `401`, ingested live transaction envelope with child SQL span, queried UI `/api/v1/performance/transactions` + `/api/v1/performance/traces/{trace_id}` and MCP `list_transactions` + `get_trace`, revoked smoke token) → PASS

**Security notes:**
- Self-review passed: all performance query params (`project_slug`, `workspace_slug`, `environment`, `release`, `op`, `query` with escaped `ILIKE`, `sort` allow-list, `hours`/`since`/`until` capped at 90 days, `trace_id` hex validation, `sample_reason` allow-list) validated server-side; all queries run via `begin_tenant_tx` (Postgres RLS `maxi_loop_app` + explicit `tenant_id` scoping); baseline + inherited custom PII scrubbing runs before zstd trace storage AND again on MCP output; zstd decompression strictly bounded at 4 MiB (`MAX_DECOMPRESSED_TRACE_BLOB_BYTES`) against decompression bombs; MCP `list_transactions` & `get_trace` enforce project/workspace/tenant token scope and wrap untrusted span/transaction strings in `<<<UNTRUSTED_EVENT_DATA>>>` with `security_notice`; negative tests cover cross-tenant UI & MCP access (`404`/denied), cross-project MCP scope enforcement, invalid `trace_id` (`400`), invalid `sort` (`400`), and oversized zstd payload rejection
- New dependency: `zstd` v0.13.5 (`zstd-safe` v7.4.1, `zstd-sys` v2.1.3+zstd.1.5.7; official Rust bindings to libzstd, explicitly required by PRD §11, Design §5.5/§5.9, and F012 acceptance criteria). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Used a 76-bucket log-scale histogram (`0.5 * 1.2^i` ms) merged via Postgres immutable function `merge_histogram_buckets` + aggregate `sum_histogram_buckets` so 1-minute metrics support exact SQL rollups and accurate p50/p95/p99 across arbitrary windows without storing raw durations; used deterministic SHA-256(`trace_id`) for 1–5% random trace sampling so all services in a distributed trace make consistent head-sampling decisions

**Not finished / blockers:**
- Ready to begin Phase 4 — `F013` (React Source Maps upload per release and bounded LRU ingest-time stack frame resolution)

### 2026-11-22 — Antigravity — F011: Phase 2b: Embedded MCP server (/mcp Streamable HTTP) for AI issue analysis and triage

**Status at end of session:** done

**Done:**
- Created `migrations/0004_phase2b_mcp.sql` adding `mcp_tokens` (SHA-256 hash only, tenant/workspace/project access scope, `can_read` default + `can_triage` opt-in, `rate_limit_per_minute`, expiry & revocation), `mcp_audit_logs`, and `issue_notes` (shared by UI and MCP), all with leading `tenant_id` keys/indexes and `ENABLE`/`FORCE ROW LEVEL SECURITY` (`maxi_loop_app` + `maxi_loop_worker`)
- Moved pure PII scrubbing (`scrub_event_payload`, `scrub_event_payload_with_rules`, `scrub_text_with_rules`) to `crates/platform/src/scrub.rs` (re-exported in `crates/ingest/src/scrub.rs`) so MCP output scrubbing shares exact ingest-parity logic without depending on `crates/ingest` or `sqlx`
- Extended `crates/auth/src/mcp.rs` with revocable scoped MCP Bearer tokens (`mlmcp_<64-hex>`), `McpAuthService` facade, audit logging, and `/api/v1/mcp/tokens` (`GET`/`POST`/`DELETE`) + `/api/v1/mcp/audit-logs` routes
- Extended `crates/org/src/service.rs` with `OrgService` facade (`list_projects`, `list_effective_pii_rules`) and `crates/issues` with `notes.rs` (`issue_notes` CRUD + `/api/v1/issues/{id}/notes`), `since`/`until` & `sort=regressed` filters, `resolve_release_commit_sha`, `update_issue_status_with_actor`, and `IssuesService` facade
- Created `crates/mcp` (`maxi-loop-mcp`) with **zero `sqlx` dependency** (`D006` compiler-enforced single service layer):
  - `http.rs` & `protocol.rs`: Streamable HTTP `/mcp` JSON-RPC 2.0 endpoint (`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`; `GET /mcp` → `405`, `DELETE /mcp` → `204`) with Bearer auth, 256 KiB payload cap, `Mcp-Session-Id`, and full audit logging (`ok`/`error`/`denied`/`rate_limited`)
  - `rate_limit.rs`: per-token `governor` GCRA rate limiter returning HTTP `429` + `Retry-After`
  - `sanitize.rs`: output PII scrubbing (baseline + tenant/workspace/project custom rules), untrusted-data envelope (`<<<UNTRUSTED_EVENT_DATA>>>` + `security_notice`), token-efficient truncation with explicit markers, in-app stack frame formatting with code context (`pre_context`/`context_line`/`post_context`), and UI deep links
  - `tools.rs`: implements all 7 tools (`list_projects`, `list_issues`, `get_issue` with release-to-`commit_sha` mapping, `get_event`, `get_issue_stats`, `update_issue_status`, `add_issue_note`) with least-privilege `tools/list` filtering and strict project/workspace/tenant scope enforcement
- Added MaxiBoard UI `web/src/modules/mcp/McpView.tsx` (MCP Bearer token management with one-time copyable token banner + Streamable HTTP client config snippet + Audit Log tab) and **Notes & AI Triage** tab in `IssueDetailFullView.tsx`
- Added PostgreSQL 16 integration test `crates/server/tests/mcp_server.rs` covering auth/revocation, viewer privilege-escalation denial, read vs triage scopes, project-scope isolation, post-ingest output PII scrubbing, untrusted-data wrapping, truncation, release-to-SHA mapping, per-token rate limiting (`429`), audit logs, and Tenant A vs Tenant B cross-tenant isolation

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (55/55 passed, including `mcp_server` integration test & 5 `maxi-loop-mcp` unit tests)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- Manual live smoke check (restarted `maxi-loop-dev.service` on port `8080`, verified `POST /mcp` unauth `401`, `GET /mcp` `405`, created read-only token → `initialize` → `tools/list` (5 read tools) → `tools/call` `list_projects` → verified audit log → revoked token → confirmed `401`) → PASS

**Security notes:**
- Self-review passed: `crates/mcp` cannot import `sqlx` (`D006`), enforcing all queries through RLS-protected tenant services; raw MCP tokens are 256-bit CSPRNG, shown once at creation, and stored strictly as SHA-256 hashes; viewers cannot mint `triage` tokens (`403`); read-only tokens cannot invoke `update_issue_status` or `add_issue_note` (denied + audit-logged); `list_projects` never exposes DSN keys; every MCP event/issue output applies baseline + inherited custom PII scrubbing and wraps untrusted event text in `<<<UNTRUSTED_EVENT_DATA>>>` markers with a `security_notice`; per-token rate limits enforce `429` + `Retry-After` and audit logging; audit log arguments are PII-scrubbed and size-capped; negative tests cover auth/revocation, privilege escalation, sibling-project scope isolation, rate limiting, and cross-tenant isolation
- Dependencies: 0 new external Rust or npm dependencies added. `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Implemented MCP Streamable HTTP JSON-RPC 2.0 directly over `axum` + `serde_json` (0 new crates) and moved pure PII scrubbing to `crates/platform/src/scrub.rs` so `crates/mcp` stays strictly free of `sqlx` while guaranteeing ingest-parity PII output scrubbing

**Not finished / blockers:**
- Ready to begin Phase 3 — `F012` (Performance monitoring: transactions, spans, 1-minute duration histograms, sampled zstd traces, distributed waterfall, and MCP `list_transactions` / `get_trace` tools)

### 2026-11-22 — Antigravity — F010: Phase 2: Configurable PII scrubbing, Release tracking, and Alerts (Webhook, Telegram, Email)

**Status at end of session:** done

**Done:**
- Created `migrations/0003_phase2_pii_releases_alerts.sql` adding `issues.first_release`, `pii_scrub_rules`, `releases`, `issue_releases`, `alert_channels`, `alert_rules`, and `alert_deliveries`, all with leading `tenant_id` keys/indexes and `ENABLE`/`FORCE ROW LEVEL SECURITY` (`maxi_loop_app` + `maxi_loop_worker`)
- Extended `crates/platform` with decoupled observer traits (`PiiScrubRulesObserver`, `AlertSignalObserver`) and optional `SmtpConfig` (with redacted password in `Debug`); updated `.env.example` with safe commented SMTP placeholders
- Implemented inherited configurable PII scrubbing:
  - `crates/org/src/pii.rs` + `/api/v1/pii-rules` CRUD API (tenant/workspace/project scope, `field_name`/`header_name`/`value_substring`) updating `ScrubRulesCache` live with 0 per-event DB queries
  - `crates/ingest/src/scrub.rs` & `scrub_cache.rs`: always enforces mandatory baseline scrubbing (`D002`) first, then applies enabled inherited custom rules
- Implemented Release tracking:
  - `crates/ingest/src/worker.rs`: persists `issues.first_release`, batch-upserts `releases` (auto-inferring 7..40 hex `commit_sha`) & `issue_releases`, and emits real-time `AlertSignal`s (`NewIssue`, `Regression`) after commit
  - `crates/issues/src/releases.rs`: `/api/v1/releases`, `/api/v1/projects/{slug}/releases`, `POST /api/v1/releases` (register/annotate commit SHA & notes), and `GET /api/v1/releases/{id}/issues`; updated issue release filter to match `issue_releases`
- Created `crates/alerts` (`maxi-loop-alerts`):
  - `ssrf.rs`: static URL + DNS-resolved IP-pinned SSRF guard blocking loopback, private RFC1918, link-local/cloud metadata (`169.254.169.254`), CGNAT, IPv4-mapped IPv6, and non-HTTP(S) schemes
  - `delivery.rs`: Webhook JSON `POST` with optional HMAC-SHA256 `X-MaxiLoop-Signature` (priority #1 per `D002`), Telegram Bot API (`https://api.telegram.org`), and server-configured SMTP/TLS Email (`tokio-rustls` + `webpki-roots`), with masked secrets in API responses
  - `engine.rs`, `service.rs`, `http.rs`: real-time `new_issue`/`regression` signal consumer + 60s `spike` threshold evaluator with cooldown deduplication and `alert_deliveries` audit log, plus `/api/v1/alerts/{channels,rules,deliveries}` routes
- Added MaxiBoard UI views `web/src/modules/releases/ReleasesView.tsx` and `web/src/modules/alerts/AlertsView.tsx` (Alert Rules, Channels & Delivery Log, Configurable PII Scrubbing) wired into `App.tsx` and `api.ts`
- Added PostgreSQL 16 integration test `crates/server/tests/phase2_pii_releases_alerts.rs` covering inherited PII scrubbing, release-issue linking, SSRF blocking, secret masking, Webhook HMAC-SHA256 (`new_issue`), Telegram (`regression`), SMTP Email (`spike` + cooldown), and Tenant A vs Tenant B isolation

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (48/48 passed)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- Manual live smoke check (restarted `maxi-loop-dev.service` on port `8080`, verified `/api/v1/health`, `/api/v1/pii-rules`, `/api/v1/releases`, `/api/v1/alerts/{channels,rules,deliveries}`, and live `400` SSRF block on `169.254.169.254`) → PASS

**Security notes:**
- Self-review passed: all PII rules, releases, alert channels, and alert rules validated server-side with strict allow-lists and length/range bounds (no user regexes — prevents ReDoS); webhook & Telegram outbound requests validate URLs statically AND resolve/pin IPs at connect time (blocking loopback, RFC1918, CGNAT, link-local, `169.254.169.254`, IPv4-mapped IPv6, redirects disabled, 10s timeout, strict `webpki-roots` TLS verification); SMTP host/credentials come strictly from server env (`Config.smtp`), never user input, and CRLF header injection is stripped; webhook secrets and Telegram bot tokens are masked in API responses and never logged; all tenant CRUD runs via `begin_tenant_tx` (Postgres RLS) + CSRF + role checks (`can_manage_projects` / `can_triage_issues`); negative tests cover SSRF blocking, secret masking, cooldown suppression, and cross-tenant isolation
- New dependency: `tokio-rustls` v0.26.6 (official Rustls async Tokio wrapper using `rustls` 0.23 already in `Cargo.lock`, required for TLS-verified outbound Webhook/Telegram HTTPS and SMTP STARTTLS/SMTPS without pulling heavy HTTP/mail client stacks). Promoted `base64`, `hmac`, `rustls`, `url`, `webpki-roots` already in `Cargo.lock`. `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Kept configurable PII scrubbing strictly additive to mandatory baseline scrubbing (`D002`) using non-regex matchers (`field_name`, `header_name`, `value_substring`) to eliminate ReDoS risk on the ingest worker hot path

**Not finished / blockers:**
- Ready to begin Phase 2b — `F011` (Embedded MCP server `/mcp` Streamable HTTP for AI issue analysis and triage)
