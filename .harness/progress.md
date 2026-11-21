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

### 2026-11-21 — Antigravity — F009: Envelope load-test generator and real Python (Django/FastAPI) Sentry SDK E2E verification

**Status at end of session:** done

**Done:**
- Created `crates/loadgen` (`maxi-loop-loadgen` binary) in the Cargo workspace (`cli.rs`, `generator.rs`, `runner.rs`, `main.rs`):
  - Parses HTTP Sentry DSNs (`--dsn` or `SENTRY_DSN`, redacting `public_key` in `Debug` & output), validates concurrency/total/duration/timeout bounds, generates synthetic Python Sentry Envelopes with `none`/`gzip`/`deflate`/`br` compression and optional unsupported `session` items, runs concurrent HTTP/1.1 keep-alive workers via `hyper` + `hyper-util`, and reports throughput, status breakdown, `429` header telemetry (`Retry-After`, `X-Sentry-Rate-Limits`), and `min`/`p50`/`p95`/`p99`/`max` latency percentiles (text or `--json`)
  - Added 6 unit & live-HTTP tests covering DSN parsing/redaction, invalid-flag rejection, all compression codecs, latency percentiles, and `200`/`429` (`Retry-After` + `X-Sentry-Rate-Limits`) verification
- Created official Python `sentry-sdk` E2E verification suite under `tests/e2e/python/` (`requirements.txt`, `verify_sentry_sdk.py`, `README.md`) testing both **FastAPI** (`FastApiIntegration`) and **Django** (`DjangoIntegration`) by changing **only the DSN**, plus a tampered-DSN negative test
- Ran live release-mode idle RSS, throughput, `429` quota/backpressure, and Python E2E verification against `target/release/maxi-loop` + PostgreSQL 16 and documented baseline results in `docs/benchmarks-phase1.md`
- Updated `.gitignore` for `.venv/` / `__pycache__/` and `.harness/ARCHITECTURE.md`; archived `F001` progress entry to `.harness/progress-archive.md`

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (45/45 passed: 7 server unit + 5 server integration + 5 auth + 9 ingest + 1 issues + 6 loadgen + 1 org + 8 platform unit + 1 platform RLS + 2 retention)
- `cargo build --workspace` & `cargo build --release -p maxi-loop -p maxi-loop-loadgen` → PASS (binary sizes: `13 MB` server + embedded SPA, `3.0 MB` loadgen)
- Live release-mode benchmarks (`maxi-loop serve` + `maxi-loop-loadgen` + Postgres 16) → PASS (Idle RSS `8.4 MiB`, burst gzip `27.6k req/s`, burst uncompressed `63.6k req/s`, gzip + early-drop `25.6k req/s`, per-project `120/min` quota exact `120` 2xx / `380` 429 with `1:error:project`, sustained 5s `143,555` req / `23,040` persisted events with `5:error:organization` backpressure and 0 lost accepted events)
- Official Python `sentry-sdk==2.75.0` E2E (`fastapi==0.165.0` + `django==5.2.19`) → PASS (FastAPI `4` exceptions → `2` grouped issues, Django `3` exceptions → `2` grouped issues, `in_app` frames + `context_line` preserved, `Authorization`/`Cookie` headers scrubbed to `[Filtered]`, tampered DSN rejected with 0 events ingested)

**Security notes:**
- Self-review passed: `ParsedDsn` redacts `public_key` in `Debug` and target output, accepts `SENTRY_DSN` via env to avoid CLI secret exposure, validates all numeric/string arguments with explicit bounds, enforces per-request network timeouts, and never disables TLS checks; Python E2E generates ephemeral runtime tokens via `secrets.token_hex`, passes `SENTRY_DSN` via env (never argv), executes worker subprocesses with argument arrays (`shell=False`), and includes negative invalid-DSN verification; `.venv/` gitignored and `.env` untracked
- Dependencies: promoted `hyper` (`client`, `http1`), `hyper-util` (`tokio`), and `http-body-util` already in `Cargo.lock` (adding only 2 tiny hyper-maintained transitive crates: `try-lock` v0.2.5 and `want` v0.3.2); Python E2E deps (`sentry-sdk`, `fastapi`, `httpx`, `django`) are isolated to `tests/e2e/python/requirements.txt` and not shipped in production; `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Placed the load generator in a separate workspace crate (`crates/loadgen`) so the production `maxi-loop` binary remains lean (13 MB, 8.4 MiB idle RSS) without bundling HTTP client/load-test code

**Not finished / blockers:**
- Phase 1 (MVP, `F001`–`F009`) is now 100% complete! Ready to start Phase 2 (`F010`: Configurable PII scrubbing, Release tracking, and Alerts — Webhook first, then Telegram and Email)

### 2026-11-21 — Antigravity — F008: Embedded React UI: Login, Projects & DSN, Issues list/detail, and hourly chart

**Status at end of session:** done

**Done:**
- Designed and iterated 4-screen FigCraft mockup (`/home/p4ck/Documents/maxi-loop-ui.figcraft`) via FigCraft MCP adopting the **MaxiBoard** light Jira-inspired design system (`D007`)
- Updated `web/src/index.css` & `web/index.html` with MaxiBoard CSS custom properties (`#FFFFFF`, `#F7F8F9`, `#172B4D`, `#0C66E4`), lozenge utilities (`.pm-lozenge-*`), `.pm-tab`, `.pm-input`, and `.pm-bicon`
- Extended `web/src/lib/api.ts` with typed endpoints for session auth (`login`, `logout`, `me`), workspaces, projects & DSN rotation, issues list/detail/events/stats, and triage status mutations with automatic `X-CSRF-Token` header enforcement
- Created `web/src/modules/projects/ProjectIcon.tsx` rendering 12 Maxiflow Doodle SVG project icons (`software-development`, `web-development`, `cloud-devops`, `cyber-security`, `data-analytics`, `ai-automation`, `mobile-app`, `ecommerce`, `finance`, `infrastructure-iot`, `project-management`, `ui-ux-design`) as pure typed React JSX `<svg>` components (zero `dangerouslySetInnerHTML`)
- Created `web/src/modules/auth/LoginView.tsx` (single-tenant default workspace context, optional tenant slug disclosure, `$ maxi-loop bootstrap` hint)
- Created `web/src/modules/issues/` (`IssuesView.tsx`, `IssueDetailDrawer.tsx`, `IssueDetailFullView.tsx`, `OccurrenceChart.tsx`, `StackTraceView.tsx`): 3-pane master-detail split layout, status tabs (`Unresolved`/`Regressed`/`Resolved`/`Ignored`/`All`), filter toolbar (`project`, `environment`, `release`, `query`, `sort`), pagination, `1h`/`1d` occurrence histogram chart, `In-app only`/`All frames` stack trace code viewer, PII-scrubbed sample event payload viewer, and `Resolve`/`Ignore`/`Unresolve` triage with `#172B4D` undo toast
- Created `web/src/modules/projects/ProjectsView.tsx` (project cards with copyable Sentry DSN, DSN rotation, retention/rate-limit metadata, and right-hand Create Project drawer with MaxiBoard icon picker + `sentry_sdk.init` quickstart snippet)
- Updated `web/src/app/App.tsx` and `web/src/modules/health/HealthCard.tsx` with the 56px MaxiBoard topbar (`/` global search shortcut) and 240px sidebar

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (39/39 passed)
- `cargo build --workspace` → PASS
- Manual smoke check (live `maxi-loop serve` serving embedded MaxiBoard-styled SPA + login + project creation + envelope ingest + filtered issues list & detail with `[Filtered]` PII + clean SIGTERM) → PASS

**Security notes:**
- Self-review passed: all dynamic strings rendered via React JSX auto-escaping (0 `dangerouslySetInnerHTML` or `eval`); all mutating requests send `X-CSRF-Token` and `credentials: "same-origin"`; role permissions (`canTriage`, `canManageProjects`) respected in UI and enforced server-side
- New dependency: `lucide-react` v1.21.0 in `web/package.json` (requested by user to match MaxiBoard icon set; `npm --prefix web audit` = 0 vulnerabilities)

**Key decisions & why:**
- Recorded `D007` in `.harness/decisions.md` (MaxiBoard light theme + `lucide-react` + Maxiflow Doodle project SVG icons)

**Not finished / blockers:**
- Ready to begin F009 (Envelope load-test generator and real Python Django/FastAPI Sentry SDK E2E verification)

### 2026-11-21 — Antigravity — F007: Retention scheduler: daily partition maintenance, DROP PARTITION, and override batch delete

**Status at end of session:** done

**Done:**
- Created `migrations/0002_drop_events_partition.sql` with `SECURITY DEFINER` function `drop_expired_events_partitions(cutoff_date DATE) RETURNS TEXT[]` granted exclusively to `maxi_loop_worker` to drop expired daily child partitions of `public.events` (`^events_[0-9]{4}_[0-9]{2}_[0-9]{2}$`) older than global retention without table bloat, and added `crates/platform/build.rs` (`cargo:rerun-if-changed=../../migrations`)
- Created `crates/retention` (`maxi-loop-retention`):
  - `model.rs`: `RetentionConfig` (`global_retention_days`, `days_ahead`, `batch_delete_limit`, `max_batches_per_run`, `schedule_interval`) and `RetentionReport`
  - `service.rs`: `compute_effective_retention_days` (inheriting and clamping `global -> tenant -> workspace -> project`), `compute_partition_cutoff_date`, `run_retention_cycle` (pre-creates upcoming daily partitions via `ensure_events_partition`, drops expired partitions via `drop_expired_events_partitions`, and executes bounded `LIMIT`-capped batch deletes for shorter workspace/project retention overrides as well as global expired `issue_hourly_counters`, stale `issues`, and expired `sessions`), and `spawn_retention_scheduler`
- Wired `spawn_retention_scheduler` into `crates/server/src/main.rs` on `maxi-loop serve`
- Added PostgreSQL 16 integration test `crates/server/tests/retention_scheduler.rs` verifying override validation (`<= global` and `<= workspace`), upcoming partition pre-creation, `DROP TABLE` on expired partitions (`-35d` and `-31d`), and multi-batch bounded deletion for 7-day project override and 14-day workspace override while preserving sibling 30-day project data in the same daily partitions

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (39 passed: 7 server unit + 5 server integration + 5 auth unit + 9 ingest unit + 1 issues unit + 1 org unit + 8 platform unit + 1 platform RLS integration + 2 retention unit)
- `cargo build --workspace` → PASS
- Manual smoke check (created expired partition `events_2025_01_15` → started live `maxi-loop serve` → verified background retention scheduler dropped `events_2025_01_15` automatically + clean SIGTERM) → PASS

**Security notes:**
- Self-review passed: `drop_expired_events_partitions` queries `pg_inherits` scoped strictly to child tables of `public.events` matching `^events_[0-9]{4}_[0-9]{2}_[0-9]{2}$` and quotes identifier via `format('DROP TABLE IF EXISTS public.%I', rec.part_name)`; `REVOKE ALL ... FROM PUBLIC` and `GRANT EXECUTE ... TO maxi_loop_worker`; all batch deletes use parameterized queries inside `begin_worker_tx` bounded by `batch_delete_limit` and `max_batches_per_run` to prevent database lock starvation; negative tests verify out-of-range workspace/project retention overrides (`> global` / `> workspace`) are rejected with `400 Bad Request`
- Dependencies: 0 new external crates added

**Key decisions & why:**
- Added `crates/platform/build.rs` emitting `cargo:rerun-if-changed=../../migrations` so Cargo always recompiles `sqlx::migrate!("../../migrations")` whenever a new SQL migration file is added

**Not finished / blockers:**
- Ready to begin F008 (Embedded React UI: Login, Projects & DSN, Issues list/detail, and hourly chart)

### 2026-11-21 — Antigravity — F006: Issues query, detail, hourly stats, and triage JSON API with tenant isolation tests

**Status at end of session:** done

**Done:**
- Created `crates/issues` (`maxi-loop-issues`):
  - `model.rs`: typed `IssueStatus` (`unresolved`/`resolved`/`ignored`), allow-listed `IssueSort` (`last_seen`/`first_seen`/`events`), allow-listed `StatsInterval` (`1h`/`1d`), `IssueListFilter`, `IssueSummaryView`, `PaginatedIssuesResponse`, `IssueDetailView`, `IssueEventView`, `CounterBucketPoint`, `IssueStatsResponse`, and `UpdateIssueStatusInput`
  - `service.rs`: RLS-enforced (`begin_tenant_tx` + `TenantContext`) shared service layer (`list_issues`, `get_issue_detail`, `get_issue_stats`, `list_issue_events`, `get_issue_event`, `update_issue_status`) reusable by both the JSON HTTP API and Phase 2b MCP server
  - `http.rs`: convenience routes (`/api/v1/issues`, `/api/v1/projects/{project_slug}/issues`, `/api/v1/issues/{issue_id}`, `/api/v1/issues/{issue_id}/status`, `/api/v1/issues/{issue_id}/stats`, `/api/v1/issues/{issue_id}/events`, `/api/v1/issues/{issue_id}/events/{event_id}`) and explicit slug-scoped hierarchy routes (`/api/v1/t/{tenant_slug}/w/{workspace_slug}/p/{project_slug}/issues/...`)
- Extended `Role` in `crates/auth` with `can_triage_issues()` (`tenant_owner`, `tenant_admin`, `workspace_admin`, `member` permitted; `viewer` read-only)
- Mounted `maxi_loop_issues::http::router` under `/api/v1` in `crates/server`
- Added PostgreSQL 16 integration test `crates/server/tests/issues_api_isolation.rs` verifying pagination, filtering (`project`, `environment`, `release`, `status`, `sort`), issue detail (`stack_frames`, `tags`, `sample_event`, `hourly_series`, `daily_series`), stats endpoint (`1h` & `1d`), triage RBAC (`viewer` `403`, missing CSRF `403`, invalid status `400`, owner/member `200`), and Tenant A vs Tenant B cross-tenant isolation

**Verification run:**
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo test --workspace` → PASS (36 passed: 7 server unit + 4 server integration + 5 auth unit + 9 ingest unit + 1 issues unit + 1 org unit + 8 platform unit + 1 platform RLS integration)
- `cargo build --workspace` → PASS
- Manual smoke check (live `maxi-loop serve` + login + project create + envelope ingest → `GET /api/v1/issues` filter → `GET /api/v1/issues/{id}` detail & series → `PATCH /api/v1/issues/{id}/status` resolve + clean SIGTERM) → PASS

**Security notes:**
- Self-review passed: all query/path inputs (`issue_id`, `project_id`, `workspace_slug`, `project_slug`, `environment`, `release`, `status`, `sort`, `interval`, `page`, `per_page`, `hours`, `limit`, search `query` with escaped `ILIKE` wildcards) validated server-side with strict allow-lists; all queries run inside `begin_tenant_tx` (`SET LOCAL ROLE maxi_loop_app` + `app.tenant_id` RLS) with explicit `tenant_id` binds; `viewer` role denied on status mutation (`403 Forbidden`); CSRF header enforced by `AuthSession` extractor on `PATCH`/`POST`; negative tests cover `viewer` triage, missing CSRF, invalid status, and Tenant A vs Tenant B cross-tenant reads & mutations across all issue/event/stats routes
- Dependencies: 0 new external crates added

**Key decisions & why:**
- Built `crates/issues/src/service.rs` without Axum/HTTP coupling so Phase 2b `crates/mcp` can call the exact same tenant-scoped service methods directly per `D006` and Design §5.10

**Not finished / blockers:**
- Ready to begin F007 (Retention scheduler: daily partition maintenance, DROP PARTITION, and override batch delete)

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
