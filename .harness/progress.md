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

### 2026-11-22 — Claude Code — Official stack logos in Create Project wizard

**Status at end of session:** done

**Done:**
- Replaced text badges in the platform picker with official stack logos: `web/src/modules/projects/platformLogos.ts` (SVG paths vendored from Simple Icons v16.38.1, CC0-1.0, no new npm dependency) + `PlatformLogo.tsx` tile (brand color tinted to the light theme; black brands Rust/Express use navy `#172B4D`, React cyan darkened to `#149ECA` for contrast)
- `ProjectsView` project cards: replaced the plain platform lozenge with `PlatformChip` (logo + platform label; unknown platforms fall back to a neutral `</>` icon + raw id). Smoke: Projects & DSN page renders chips with logos for all cards.
- Removed the legacy inline "Create project" form from the `ProjectsView` right drawer (state, handler, unused imports); the drawer keeps the SDK quickstart and all project creation goes through the `CreateProjectDialog` wizard. Smoke: old form gone, header "Create project" opens the wizard.

**Verification run:**
- `npm --prefix web run build` → PASS; `cargo build --release -p maxi-loop` → PASS
- Live headless Chrome screenshot of the picker on `maxi-loop-dev.service` → all 10 logos render

**Security notes:**
- Vendored SVG path data validated (only SVG path-command characters, 6-digit hex colors) before embedding; rendered via React `d=` attribute (no innerHTML)

### 2026-11-22 — Claude Code — Sentry-style Create Project onboarding wizard

**Status at end of session:** done

**Done:**
- Rebuilt `web/src/modules/projects/CreateProjectDialog.tsx` as a 3-step wizard: (1) choose platform (searchable/filterable card grid), (2) name project (name, slug, icon, tags, collapsible advanced retention/rate limit), (3) set up SDK — DSN + platform-specific Install/Configure/Verify snippets with copy buttons, and a live "Waiting to receive first event…" indicator that polls issues + transactions for the new project every 3s until the first event arrives ("Take me to my event" → Issues filtered to the project)
- Added `web/src/modules/projects/onboardingPlatforms.ts` (platform catalogue + snippet builder: Python, FastAPI, Django, Flask, Go, Rust, Node.js, Express, React, Browser JS, Other)
- `web/src/app/App.tsx`: dialog stays open after create; new `onViewIssues` handler

**Verification run:**
- `npm --prefix web run build` → PASS; `npm --prefix web audit` → 0 vulnerabilities
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (95/95), `cargo build --workspace` → PASS
- Live headless Chrome smoke on `maxi-loop-dev.service`: wizard FastAPI → create → waiting state → sent a real `ZeroDivisionError` via official `sentry-sdk` → dialog showed "First error received" → navigated to Issues filtered to the project. Smoke project deleted from DB afterwards.

**Security notes:**
- No backend/API changes; uses existing CSRF-protected `POST /api/v1/projects` and authenticated read endpoints. Platform id still validated server-side by `parse_platform`.

**Not finished / blockers:**
- None

### 2026-11-22 — Antigravity — Fix Sidebar Projects '+' Button & Add MaxiBoard CreateProjectDialog Modal

**Status at end of session:** done

**Done:**
- Diagnosed why clicking `+` next to `PROJECTS` in the sidebar (and `+ New project` in the topbar) appeared unresponsive when already on `Projects & DSN` or on narrow viewports: both handlers only called `setActiveNav("projects")` (a no-op on the projects view) and the sidebar `+` button had a tiny 14px hit area without opening a create modal
- Created `web/src/modules/projects/CreateProjectDialog.tsx`: portaled centered MaxiBoard modal (`Escape`/backdrop close, `role="dialog"`, `aria-modal="true"`) matching `CreateWorkspaceDialog.tsx` and MaxiBoard's `CreateBoardDialog.jsx`, with workspace header, MaxiBoard project icon/accent picker, auto-slugged project name & slug, platform selector, group tags (+ clickable existing-tag chips), retention & rate-limit overrides, and validation
- Updated `web/src/app/App.tsx`: wired sidebar `PROJECTS` `+` button (upgraded to MaxiBoard's `24×24px` hoverable hit area, hidden for non-managers), topbar `+ New project` button, and `IssuesView` empty-state CTA to open `CreateProjectDialog` directly from any view, reload projects, navigate to `Projects & DSN`, and show confirmation toast
- Updated `web/src/modules/projects/ProjectsView.tsx`: added `+ Create project` buttons in the header and empty state wired to `onOpenCreateProject`

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace` → PASS
- `cargo test --workspace` → PASS (94/94 passed)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- Live Headless Chrome E2E on restarted `maxi-loop-dev.service` (port `8080`) → PASS (clicked sidebar `+` from `Issues` view → modal opened; navigated to `Projects & DSN` and clicked sidebar `+` again → modal opened; created `smoke-proj-plus` with tag `smoke-tag` → DSN & toast rendered; cleaned up smoke project from DB)

**Security notes:**
- Self-review passed: project creation continues to use existing CSRF-protected `POST /api/v1/projects` with server-side slug/name/platform/tags/retention/rate-limit validation, `can_manage_workspace()`, and Postgres RLS; no new dependencies added

**Key decisions & why:**
- Followed MaxiBoard's `BoardShell` + `CreateBoardDialog` pattern so clicking `+` next to `PROJECTS` works identically from every view (including when already on `Projects & DSN` or on smaller screens)

**Not finished / blockers:**
- None

### 2026-11-22 — Antigravity — MCP Stdio Bridge (`maxi-loop mcp`), Streamable HTTP Protocol Header Hardening & Live Claude Code CLI E2E

**Status at end of session:** done

**Done:**
- Reviewed `maxi-code`'s stable stdio MCP proxy pattern (`src/mcp_board.rs`, `src/mcp_supervisor.rs`, ACP/OpenCode wiring) and brought parity to `maxi-loop` alongside Streamable HTTP `/mcp`:
  - Created `crates/mcp/src/stdio.rs` implementing `StdioBridge` & `run_stdio_bridge` (0 new external crates — reuses workspace `hyper` + `tokio-rustls` + `webpki-roots` with strict TLS verification, 30s timeout, 256 KiB bounded stdin line read, 4 MiB bounded HTTP response read, `Mcp-Session-Id` & `MCP-Protocol-Version` tracking, best-effort `DELETE /mcp` on EOF, token redaction in `Debug`/errors, and graceful unconfigured/401/429 handling)
  - Hardened `crates/mcp/src/http.rs` to echo both `Mcp-Session-Id` and `MCP-Protocol-Version` on every JSON-RPC response from `POST /mcp` (asserted in `crates/server/tests/mcp_server.rs`)
  - Wired `maxi-loop mcp [--url <URL>] [--token-env <ENV>]` in `crates/server/src/cli.rs` & `crates/server/src/main.rs` before `Config::from_env()` / DB connect, sending all tracing strictly to `stderr` so `stdout` stays pure line-delimited JSON-RPC 2.0 and zero `DATABASE_URL` is required
  - Updated `web/src/modules/mcp/McpView.tsx` with a **Streamable HTTP** / **Stdio Bridge (`maxi-loop mcp`)** config snippet switcher and updated the exposed tools & prompts summary (all 11 tools + 4 prompts)
  - Created `tests/e2e/verify_mcp.py` covering live Streamable HTTP `/mcp`, spawned `maxi-loop mcp` stdio subprocess (with `DATABASE_URL` stripped), negative checks (missing env, read-only denial, revoked token `401` on HTTP & stdio), and `--test-claude` live verification through real Claude Code CLI (`claude -p --strict-mcp-config`)
  - Updated `.harness/ARCHITECTURE.md` and archived `F011` to `.harness/progress-archive.md`

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo check --workspace` → PASS
- `cargo test --workspace` → PASS (94/94 passed, incl. 11/11 `maxi-loop-mcp` unit tests, 8/8 `maxi-loop` lib tests, `mcp_server`, and `p2_performance_mcp`)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- `cargo tree --depth 1 -p maxi-loop-mcp` → PASS (0 direct `sqlx` dependency, `D006` preserved)
- Live E2E against restarted `maxi-loop-dev.service` (`python3 tests/e2e/verify_mcp.py --test-claude`) → PASS (Streamable HTTP `/mcp` + Stdio Bridge `maxi-loop mcp` + Real Claude Code CLI connecting and executing `list_projects` & `list_issues` over **both** Streamable HTTP and Stdio Bridge + negative auth/revocation/unconfigured checks)

**Security notes:**
- Self-review passed: `maxi-loop mcp` rejects raw tokens on CLI argv (reads from `MAXI_LOOP_MCP_TOKEN` or `--token-env` env var only), redacts tokens in `Debug` & error strings, enforces `http`/`https` schemes without embedded userinfo, uses strict `rustls` + `webpki-roots` TLS verification, bounds stdin lines (256 KiB), HTTP responses (4 MiB), and timeouts (30s); `crates/mcp` remains strictly `sqlx`-free (`D006`); negative tests verify unconfigured env, invalid/revoked token `401` on both HTTP & stdio, read-only triage denial, PII scrubbing, and zero token leak in stdout/stderr
- Dependencies: 0 new external Rust or npm dependencies added (reused workspace `hyper`, `hyper-util`, `http-body-util`, `rustls`, `tokio`, `tokio-rustls`, `url`, `webpki-roots`). `cargo-audit` and `gitleaks` are not installed on this host (skipped, noted explicitly)

**Key decisions & why:**
- Used a dedicated blocking `stdin` reader thread feeding a bounded `mpsc` channel into the Tokio runtime rather than holding a blocking lock across `.await`, and routed all `maxi-loop mcp` tracing strictly to `stderr` so `stdout` never carries non-JSON-RPC bytes

**Not finished / blockers:**
- None

### 2026-11-22 — Antigravity — MaxiBoard-style Top-Left Workspace Switcher & Create Workspace Dialog

**Status at end of session:** done

**Done:**
- Reviewed `maxi-code` MaxiBoard (`WorkspaceMenu.jsx`, `workspaceSwitcher.js`, `BoardShell.jsx`, `CreateWorkspaceDialog.jsx`, `WorkspaceSettingsPage.jsx`) and identified why creating/switching workspaces was missing in MaxiLoop UI (`App.tsx` skipped `fetchWorkspaces()` in `single_tenant_mode`, rendered a non-clickable top-left workspace block, and had no UI form calling `POST /api/v1/workspaces`)
- Created `web/src/modules/workspaces/` vertical-slice UI module:
  - `workspacePrefs.ts`: MaxiBoard 9-color workspace palette, localStorage preferences (colors, starred workspaces, recently-opened timestamps, last-active workspace per tenant), and `arrangeWorkspaces` (Starred → Recently opened → All A–Z + ranked search & match highlight from `SEARCH_MIN = 6`)
  - `WorkspaceMark.tsx`: MaxiBoard rounded square colored initial mark
  - `CreateWorkspaceDialog.tsx`: Portaled centered MaxiBoard modal (`Escape`/backdrop close) calling `POST /api/v1/workspaces` with Workspace name, auto-generated editable URL-safe slug, 9-color swatches, and optional parameter overrides (`retention_days_override` bounded by tenant limit, `default_rate_limit_per_minute_override`)
  - `WorkspaceSwitcher.tsx`: Clickable top-left sidebar trigger (`WorkspaceMark`, name, project count, rotating chevron) opening a portaled MaxiBoard dropdown (`Ctrl/⌘+K` shortcut, keyboard `↑`/`↓`/`Enter` navigation, search box when `>= 6` workspaces, star/unstar, active workspace parameters strip showing tenant/mode, effective workspace role, retention days, and rate limit, plus fixed bottom actions: **Workspace settings & limits**, **Members & access**, and **Create workspace**)
- Updated `web/src/app/App.tsx`: always loads accessible workspaces (in both single-tenant and multi-tenant modes), restores `?workspace=` or last-opened workspace with accessible-workspace fallback, computes per-workspace effective role matching server RBAC, switches cleanly into newly created workspaces with bottom toast, and wires active workspace retention/limit into sidebar footer & `ProjectsView`
- Updated `web/src/modules/admin/TeamAndSettingsView.tsx` (supports `initialTab`/`tabNonce` deep jumps from dropdown, adds `+ Create workspace` button, workspace name + color badge editing alongside retention & rate-limit overrides) and `web/src/modules/mcp/McpView.tsx` (passes active `workspace_slug` when creating workspace-scoped MCP tokens)
- Added `.pm-fade-in` transition in `web/src/index.css`; updated `.harness/ARCHITECTURE.md` and archived `F010` to `.harness/progress-archive.md`

**Verification run:**
- `npm --prefix web run build` → PASS (`tsc --noEmit && vite build`)
- `npm --prefix web audit` → PASS (0 vulnerabilities)
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings)
- `cargo check --workspace` → PASS
- `cargo test --workspace` → PASS (89/89 passed across 39 suites)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- Live Headless Chrome E2E smoke test against restarted `maxi-loop-dev.service` (port `8080`) → PASS (unauth `GET`/`POST /api/v1/workspaces` `401`/`401`, invalid slug `400`, top-left dropdown opened with parameters & actions, created `Smoke Platform` `14d`/`500/min` via modal, auto-switched workspace + toast verified, temporary workspace cleaned up)

**Security notes:**
- Self-review passed: all workspace creation and update calls still go through existing server-side validated `POST /api/v1/workspaces` & `PATCH /api/v1/workspaces/{slug}/settings` with `AuthSession` + CSRF + `can_manage_tenant()`/`can_manage_workspace()` + Postgres RLS; UI only persists non-sensitive display prefs (color, stars, last-opened slug) in `localStorage`; negative checks verified unauthenticated `401` and invalid slug `400`
- Dependencies: 0 new Rust or npm dependencies added

**Key decisions & why:**
- Portaled the workspace switcher popover to `document.body` anchored to the top-left trigger button's bounding rect so it renders at a comfortable 296px width without being clipped by the 240px sidebar's `overflow-y-auto`, and enabled workspace loading in `single_tenant_mode` since single-tenant hides the tenant tier rather than workspaces within the tenant

**Not finished / blockers:**
- None

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
