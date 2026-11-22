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

### 2026-11-22 — Claude Code + Antigravity — F017: Performance Issues (N+1 DB, N+1 API, Slow DB Query, Consecutive DB/HTTP)

**Status at end of session:** done

**Done:**
- Created `migrations/0011_performance_issues.sql`: added `issues.issue_type` (`TEXT NOT NULL DEFAULT 'error'` with `CHECK` constraint across `error` and 5 `performance_*` types + composite index `(tenant_id, project_id, issue_type, last_seen DESC)`) and expanded `traces.sample_reason` `CHECK` constraint to allow `'issue'`
- Added `crates/performance/src/detectors.rs` (`detect_performance_problems`) running on every ingested transaction before sampling with Sentry-like thresholds: N+1 DB (`>=5` identical normalized queries, `>=50ms`), N+1 API (`>=5` identical `http.client` calls, `>=100ms`), Slow DB Query (`>=1s`), Consecutive DB (`>=2` sequential sibling queries `>=30ms` each, savings `>=100ms`), Consecutive HTTP (`>=3` sequential calls `>=500ms` each, savings `>=1s`); triggering transactions are always retained as sampled traces (`sample_reason = 'issue'`)
- Added `crates/ingest/src/perf_issue.rs` converting detected `PerformanceProblem`s into `ProcessedEvent`s grouped by `(issue_type, transaction_name, group_hash)` with normalized (literal-free) span descriptions, wired into `crates/ingest/src/worker.rs` to reuse regression, releases, hourly counters, and alerts
- Extended `crates/issues` (`model.rs`, `service.rs`, `releases.rs`, `merge_split.rs`) and `crates/mcp/src/tools.rs` with allow-listed `issue_type` filtering (`error`, `performance`, or exact `performance_*`), `issue_type` in issue/MCP views, `performance_problem` in compact MCP event extraction, and cross-type merge rejection
- Extended `crates/performance/src/service.rs` & `web/src/modules/performance/PerformanceView.tsx` to support `sample_reason = 'issue'`
- Built UI in `web/src/modules/issues/PerformanceProblemView.tsx`, `IssuesView.tsx`, `IssueDetailDrawer.tsx`, `IssueDetailFullView.tsx`, `web/src/app/App.tsx`, and `web/src/lib/api.ts`: `Type` filter + `PERFORMANCE` lozenge, Performance Problem summary card with offending normalized spans table, and `View trace` deep link straight into the trace waterfall
- Added permanent E2E test `tests/e2e/python/verify_performance_issues.py` using official Python `sentry-sdk` + Django covering all 5 detectors, negative cases, grouping, and PII literal stripping
- Recorded follow-up roadmap items `F018` (users affected + impact sorting) and `F019` (p95/error-rate performance alerts) in `.harness/feature-list.json`; archived `F015` and React SDK CORS E2E entries to `.harness/progress-archive.md`

**Verification run:**
- `npm --prefix web run build` → PASS
- `cargo fmt --check` → PASS
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS
- `cargo test --workspace` → PASS (107 passed, 0 failed)
- `cargo build --workspace` & `cargo build --release -p maxi-loop` → PASS
- `cargo tree --depth 1 -p maxi-loop-mcp` → PASS (0 direct `sqlx` dependency, `D006` preserved)
- Live E2E (`tests/e2e/python/verify_performance_issues.py` + regression Sentry SDK & MCP E2E) on restarted `maxi-loop-dev.service` → PASS
- Headless Chrome UI screenshots (desktop Issues + N+1 drawer, `View trace` waterfall with `SAMPLE: ISSUE` & 12 N+1 spans, mobile overlay) → PASS

**Security notes:**
- Self-review passed: `issue_type` and `sample_reason` validated server-side against strict static allow-lists matching DB `CHECK` constraints; cross-type merge rejected with `400`; all queries parameterized inside `begin_tenant_tx`/`begin_worker_tx` (existing Postgres RLS inherited); performance issue payloads store only normalized (literal-free) span descriptions and PII-scrubbed transaction metadata (verified in E2E & MCP output); 0 new Rust or npm dependencies added; `cargo-audit` and `gitleaks` not installed on this host

**Key decisions & why:**
- Reused the existing `issues` table with `issue_type` rather than a separate table so performance problems automatically inherit grouping, regression detection, release links, hourly counters, alerts, RLS, and MCP/UI triage workflows

**Not finished / blockers:**
- Ready for `F018` (users affected per issue) or `F019` (performance p95/error-rate alerts)

### 2026-11-22 — Claude Code — Colours & fonts identical to MaxiBoard (D009)

**Status at end of session:** done (fba02cc, merged to main)

**Done:**
- `web/src/index.css`: white sidebar, MaxiBoard font + mono stacks, lozenge 12px/radius 4/`#E9F2FF` primary, tab underline on the strip border (`bottom:-1px`), `(hover:none) and (pointer:coarse)` 16px input guard, skeleton tokens.
- `web/src/lib/avatar.ts` (port of MaxiBoard `initials.js`); topbar avatar uses it; brand mark is an icon tile like MaxiBoard's; toast restyled to MaxiBoard's sonner light toast (bottom-center).
- Off-palette colours replaced (`#974F0C`→`#A54800`, `#533F04`→`#7F5F01`, `#006644`→`#216E4E`, `#D97706`→`#D97008`, `#0E7490`→`#206A83`, chart bars → `--color-primary`).

**Verification run:**
- `npm --prefix web run build` → PASS; `cargo build --release -p maxi-loop` → PASS; dev service restarted; headless Chrome screenshots (desktop Issues, Projects + toast, mobile) OK

### 2026-11-22 — Claude Code — UI polish: MaxiBoard menu typography + responsive shell

**Status at end of session:** done (fba02cc, merged to main)

**Done:**
- Compared against MaxiBoard source (`maxi-code/static/src/pm/shell/BoardShell.jsx`, `maxiboard.css`): sidebar nav was 12px/16px icons vs MaxiBoard 14px/18px; no mobile layout at all (sidebar always visible, topbar overflowing at 390px).
- `web/src/app/App.tsx`: nav buttons → `NAV_ITEMS` + `NavItem` (text-sm, 18px icons, `min-h-11 md:min-h-9`); topbar responsive (hamburger, search icon → drop-down search row, `+` icon on mobile; lozenges hidden < lg/xl); sidebar becomes off-canvas drawer < md (backdrop, Esc, closes on navigation); new mobile bottom tab bar (Issues / Performance / Releases / Alerts / More→drawer); toast centered above tab bar on mobile; `h-[100dvh]`.
- `ProjectTree`, `WorkspaceSwitcher`: rows text-sm, touch heights on mobile.
- `index.css`: `.pm-tab` 14px / 40px (44px mobile); `.pm-input` 14px; inputs/selects 16px on phones (no iOS focus-zoom).
- All views: `h-8/h-9` controls text-xs → text-sm; `px-6` → `px-4 md:px-6`; page H1 `text-xl md:text-2xl font-semibold`; header rows wrap; tables scroll horizontally on mobile (`overflow-x-auto`, `max-md:min-w-[640px]`, base 13px).
- Side panels < lg: Issue/Transaction/Span inspectors become full-screen overlays under the topbar; Releases/Alerts/Projects action panels stack below content. Issues no longer auto-opens the first issue < lg; filter toolbar is a 2-col grid on mobile.

**Verification run:**
- `npm --prefix web run build` → PASS; `cargo build --release -p maxi-loop` → PASS; `maxi-loop-dev.service` restarted
- Headless Chrome (CDP) screenshots at 390×844, 900×1000, 1440×900 of Issues, issue inspector, drawer, Performance, Releases, Alerts, MCP, Projects, Team, Create-project wizard → layouts render without overflow

### 2026-11-22 — Claude Code — Official stack logos in Create Project wizard

**Status at end of session:** done

**Done:**
- Replaced text badges in the platform picker with official stack logos: `web/src/modules/projects/platformLogos.ts` (SVG paths vendored from Simple Icons v16.38.1, CC0-1.0, no new npm dependency) + `PlatformLogo.tsx` tile (brand color tinted to the light theme; black brands Rust/Express use navy `#172B4D`, React cyan darkened to `#149ECA` for contrast)
- `ProjectsView` project cards: replaced the plain platform lozenge with `PlatformChip` (logo + platform label; unknown platforms fall back to a neutral `</>` icon + raw id). Smoke: Projects & DSN page renders chips with logos for all cards.
- Removed the legacy inline "Create project" form from the `ProjectsView` right drawer (state, handler, unused imports); the drawer keeps the SDK quickstart and all project creation goes through the `CreateProjectDialog` wizard. Smoke: old form gone, header "Create project" opens the wizard.
- `ProjectsView` SDK quickstart rebuilt on the wizard's snippets: project DSN selector + logo tabs for all 10 platforms (defaults to the project's own platform); shared `SdkSetupSteps`/`CodeBlock` extracted to `web/src/modules/projects/SdkSetupSteps.tsx` and reused by `CreateProjectDialog`. Smoke: Go tab renders Go install/configure/verify with the selected project's DSN.

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
