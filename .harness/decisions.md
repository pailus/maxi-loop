# Decision Log

> Decisions with lasting impact (architecture, libraries, data model, API
> shape, conventions, trade-offs the user chose). Newest at the TOP.
> Before proposing to change something listed here, read why it was decided;
> reversing a decision needs the user's explicit OK and a new entry.
> Keep each entry to a few lines.

---

### D021 — 2026-11-25 — Browser / client OS derived server-side from the User-Agent (F029)
**Decision:** Like Sentry, ingest parses `request.headers["User-Agent"]` (after PII scrubbing) into `contexts.browser` and an OS context when the SDK did not send them: `os` for `platform = javascript`, otherwise `client_os` (for backend SDKs `os` means the server). Context tags (`browser`, `client_os`, `os`, `runtime` + `.name`, `device`, `server_name`, `url` without query) are added unless the SDK set the same tag. Performance issue events copy a bounded snapshot (request line + ≤48 headers, no body; contexts except `trace`/`profile`, each ≤2 KB; sdk name/version; server_name). The parser is a small hand-written token matcher in `platform/src/user_agent.rs`.
**Why:** Backend SDKs (Django/FastAPI) only forward the raw header, so issues showed no browser/OS at all; performance issues showed no request/runtime either because only the trace context was copied.
**Rejected:** `uaparser`/`woothee` crates (big regex tables or extra dependency for a display-only feature); storing the full transaction payload on perf issues (size — transactions can be 2 MiB).
**Decided by:** assistant (user asked for Sentry-like browser/OS/info on issues)

### D020 — 2026-11-24 — Separate size cap for transaction items; analyze 1000 spans (F028)
**Decision:** `MAX_TRANSACTION_PAYLOAD_BYTES` (default 2 MiB) caps `transaction` items separately from error events (`MAX_EVENT_PAYLOAD_BYTES` stays 512 KiB); `MAX_ENVELOPE_DECOMPRESSED_BYTES` default rises to 3 MiB and must be >= both item caps (checked at startup). 413s from authenticated DSNs are logged as WARN. `MAX_SPANS_PER_TRANSACTION` = 1000. Span grouping hashes the full normalized SQL (bounded 8192 chars input); display text elides the middle.
**Why:** A real Django app showed N+1 requests produce 0.6–1.4 MB transactions (500–1000 SQL spans); they were rejected with a silent 413 (SDKs drop 413 without retrying), so the worst N+1 endpoints never appeared, while Sentry shows them. Truncating SQL to 512 chars before hashing merged queries that only differ in their WHERE clause.
**Rejected:** Raising the error-event cap too (no need); accepting the envelope but dropping only the oversized item (keeps 413 semantics SDKs already handle); lower N+1 thresholds (kept Sentry defaults). Worst-case queue memory grows from capacity × 2 MiB to capacity × 3 MiB.
**Decided by:** assistant (user asked to fix after the personalia test)

### D019 — 2026-11-23 — Production: Docker Compose behind host nginx, image built locally and shipped over SSH (F027)
**Decision:** Run production as `docker-compose.prod.yml` (PostgreSQL 16 unpublished + app on `127.0.0.1:8110`) behind the host's nginx (certbot TLS, Cloudflare Full). The image is built on the dev machine and transferred with `docker save | ssh docker load` (`deploy/deploy.sh`); the server never builds Rust. The first admin is created once with `maxi-loop bootstrap`; `BOOTSTRAP_ADMIN_*` is never put in the production `.env`.
**Why:** The VPS is shared with other services (4 vCPU / 8 GB) and already uses the nginx + loopback-container pattern; building there would compete for CPU/RAM. Startup bootstrap upserts the admin and would reset a changed password on every restart.
**Rejected:** Caddy/Traefik in the stack (host nginx already owns 80/443); building on the server; a registry (not needed for one host yet).
**Decided by:** assistant (deploy requested by user)

### D018 — 2026-11-22 — Project-limited workspace memberships (Stage B, F026)
**Decision:** `memberships.project_limited` + `membership_projects` (and `invitations.project_limited` + `invitation_projects`, migration 0016). Only workspace-scoped `member`/`viewer` grants can be limited; admins and tenant-level roles always cover their whole scope, and promotion clears limits. The explicit flag, not "has rows", marks a limited grant, so a deleted project (FK cascade) can only shrink access. Limits are read once per request into `AuthSession.workspace_project_limits` and applied through `project_access()` (D017), so every endpoint follows them; MCP `role_within_scope` and SQL `user_can_triage_project` apply the same rule (no workspace-wide MCP token for limited members). `replace_membership_limits_in_tx` widens before it narrows so the per-statement unassign triggers never see a transient narrower state. API: `project_ids` on member create and invitation create, `PUT /members/{id}/projects` (workspace managers only), `projects` + `project_limited` on member/invitation views and invite preview.
**Why:** User wants invites/members limited to chosen projects; issue assignment must follow the same access.
**Rejected:** Limiting tenant-level or admin roles (admins manage the whole workspace); "no rows = all projects" without a flag (a deleted project would silently grant the whole workspace); per-project roles (role stays per membership).
**Decided by:** assistant (user approved Stage B design)

### D017 — 2026-11-22 — One access model for project data: `ProjectAccess` + `AccessGuard` (B0)
**Decision:** `maxi_loop_platform::access` defines `ProjectAccess` (visible workspace ids + project ids, pushed into list SQL via `push_access_sql`) and the `AccessGuard` trait (`check_read/check_triage/check_manage_project/check_manage_workspace/check_manage_tenant/check_manage_scope/can_read_scope`). `AuthSession` produces `project_access()` (`None` = tenant-level role) and implements `AccessGuard`; a bare `Role` implements it as a tenant-wide role (internal callers/tests). Services that resolve their own target (releases, source maps, alert channels/rules, PII rules) take `&dyn AccessGuard` and check the exact project/workspace they resolved, inside their transaction. Alert channels, alert rules and PII rules are managed by scope: tenant-wide ones need tenant owner/admin, workspace/project ones need admin rights there; lists show only readable scopes. MCP tokens: creation requires access (and triage, if requested) to the requested scope; on every request the role is recomputed inside the token scope (`role_within_scope`), so tokens lose power with their owner; tenant-wide token/audit listing and revoking only for tenant owners/admins.
**Why:** Several endpoints used the caller's highest role anywhere in the tenant or checked nothing for session users (performance, issue stats/events/notes), and post-pagination filtering leaked counts. One model also lets Stage B (per-project limits) change behaviour in one place.
**Rejected:** Handler-side project lookups before calling services (can drift from what the service resolves); keeping `retain` after pagination; making alert channels tenant-admin-only regardless of scope (channels already have scopes).
**Decided by:** assistant (user approved B0 before Stage B)

### D016 — 2026-11-22 — Issue assignment eligibility lives in one SQL function (F024)
**Decision:** `issues.assignee_user_id` (nullable, `ON DELETE SET NULL`). Who may be assigned = SQL `user_can_triage_project(tenant, user, project)` (active user with tenant-level or same-workspace role in owner/admin/workspace_admin/member). It is used by the assign API, the assignable-members list and the `memberships_unassign_issues` trigger (AFTER DELETE / UPDATE OF role, workspace_id) that clears assignments a user no longer qualifies for. Assigning requires the same rights as changing status; listing assignable members also requires triage rights (member emails are not exposed to viewers). Each change writes an issue note ("Assigned to …" / "Unassigned (was …)"). MCP: `assign_issue` (`me` = token owner, email, or null) and `list_assignable_members`, both `triage` scope.
**Why:** User wants assignment in UI and MCP, and project-level access next (Stage B). With one SQL rule, Stage B only changes the function body; the trigger keeps the invariant even when memberships change outside the members API (e.g. workspace delete cascade).
**Rejected:** Checking eligibility only in Rust (auth crate can't reach issues; two copies of the rule); allowing viewers as assignees; multiple assignees per issue.
**Decided by:** assistant (user approved Stage A "assignment + MCP" first, then project access)

### D015 — 2026-11-22 — Users join by single-use invite link (F022); self-signup still off
**Decision:** Admins create invitations (`invitations` table, 0014) for email + role + scope, using the same privilege checks as direct member creation (`members::resolve_grant_target`). The 256-bit token is stored only as SHA-256, shown once as `<PUBLIC_BASE_URL>/#invite=<token>` (fragment, so it never reaches server logs or Referer), expires after 7 days, is single-use, and re-inviting the same email/scope revokes the previous link. Accepting runs in one worker transaction (row-locked invite): new email → user sets name + password; existing email → must enter that account's password (shares the login rate-limit bucket); then membership is granted and a session is opened. Direct creation with an admin-set password stays available ("Create directly"). Users can change their own password (`POST /auth/password`), which revokes their other sessions.
**Why:** User asked how users get in; the admin typing and sending a password was the only path. Invites keep D002 (no self-service signup) while letting people pick their own password.
**Rejected:** Open self-registration (reverses D002); emailing the link now (SMTP lives in the alerts crate, would need a new mailer boundary — later step); token in query string (logged by proxies).
**Decided by:** assistant (user approved "invite + change password")

### D014 — 2026-11-22 — Menu, tabs and selection live in the query string
**Decision:** UI location is kept in `?view=<nav>&tab=<tab>&project=&workspace=&status=<issues tab>&issue=<id>&full=1&detail=<full-view tab>&trace=<id>` via `web/src/lib/urlState.ts` (`setUrlParams`, `useUrlEnum`). Menu changes `pushState` (Back works) and drop view-scoped params (`tab, status, issue, full, detail, trace`) synchronously before render; tabs/selection use `replaceState`. Defaults are omitted from the URL. URL values are validated (enum allow-lists, numeric issue id); an unreadable `?issue=` is dropped.
**Why:** Refresh lost the open menu/tab (user report). Query params keep the existing deep links (`?view=performance&project=`, `?trace=`, D011 `issue_url`) and need no server SPA fallback for paths.
**Rejected:** Path routes (`/issues/123`, needs server fallback + router dependency); react-router (new dependency for a single-shell app); localStorage (not shareable, not per-tab).
**Decided by:** assistant (user asked that menus/tabs survive refresh)

### D013 — 2026-11-22 — Softer hover & card border than MaxiBoard (amends D009)
**Decision:** `--color-bg-hover` `#ebecf0` → `#f4f5f7` (rows/nav/ghost buttons); surfaces lightened: `--pm-col` `#f1f2f4` → `#f5f6f8` (code blocks, filled filters/buttons), `--color-bg-muted` `#f7f8f9` → `#fafbfc`, `--pm-card-hover` → `#fafbfc`; new `--color-bg-hover-strong: #eef0f3` for hover on controls that already have a `--pm-col` fill and for neutral badges/lozenges; `--color-border` `#dcdfe4` → `#e4e6ea`; `--pm-shadow-card` → `0 1px 3px rgba(9,30,66,0.05)`; read-only/disabled `.pm-input` uses `--color-border` instead of `--color-border-strong` (no second 1px ring on top of the card border); 150ms colour transition on interactive elements via zero-specificity `:where()` in `index.css`.
**Why:** User found hover and grey content backgrounds (e.g. stack frame) too dark/abrupt and card borders too thick (border + ring shadow stacked to ~2px).
**Rejected:** Keeping exact MaxiBoard tokens (D009) for these values.
**Decided by:** user

### D012 — 2026-11-22 — Optional ingest write-ahead spool (F020) and MSRV 1.89
**Decision:** `INGEST_SPOOL_DIR` (unset = off) enables a local write-ahead spool: each accepted envelope is framed (`len | crc32 | seq | payload`, hand-written binary codec) and appended to `segment-<first_seq>.spool` under a mutex that also performs the channel `try_send`, so sequence order = worker order. The worker writes `checkpoint` (tmp + rename) after every flushed batch (also after a failed flush — the spool protects against crashes, not DB errors); segments are rotated at 64 MiB or 30 s and deleted once checkpointed; the active segment is `fdatasync`'d every second. Startup replays records after the checkpoint before HTTP is served. Channel full → `429` without writing; spool write error → `503`. Delivery is at-least-once. Dir must be `0700` (refused otherwise, never chmod'd), files `0600`, `flock` via `File::try_lock` prevents two processes sharing a spool. Workspace `rust-version` raised 1.85 → 1.89 for `File::try_lock` (existing let-chains already required ≥ 1.88).
**Why:** PRD §8 / design §5.2: accepted events must survive crashes/restarts when the operator opts in, without per-request fsync latency or new dependencies (`crc32fast` was already in the lockfile).
**Rejected:** Per-request fsync / group commit (latency on every envelope for power-loss-only gains); serde/bincode encoding (new dep, or JSON inflating raw payloads); spilling to disk to accept beyond channel capacity (unbounded backlog, more complexity); a `fs2`/`libc` lock crate.
**Decided by:** assistant (user chose "spool before load test, packaging last")

### D011 — 2026-11-22 — Performance alerts reuse `alert_rules` / `alert_deliveries` (F019)
**Decision:** `perf_p95` and `perf_error_rate` are new `alert_rules.trigger_type`s (columns `transaction_name` (NULL = whole project), `perf_threshold` (ms / %), `perf_window_minutes` default 5, `perf_min_count` default 10), evaluated on the existing alert-engine tick (60s) from `transaction_minute_metrics` (all transactions, not just sampled). A rule fires per project in scope when the window value is strictly above the threshold. Deliveries reuse `alert_deliveries` with nullable `issue_id` (+ `transaction_name`, `metric_value`, `subject`); cooldown key = (rule, project, transaction). Webhook payload keeps the issue-alert shape, omits `issue_id` and adds a `performance` object; `issue_url` points to `/?view=performance&project=<slug>`.
**Why:** Same channels, SSRF-safe delivery, cooldown and audit log/UI as issue alerts with no parallel tables or UI.
**Rejected:** Separate `performance_alert_rules`/`performance_alert_deliveries` tables; firing only on state change (resolve/re-fire) — cooldown dedup matches the acceptance criteria and existing spike rules.
**Decided by:** assistant (F019 acceptance criteria agreed with user)

### D010 — 2026-11-22 — Users affected = keyed, tenant-scoped user hash (F018)
**Decision:** Count users from the event `user` interface (`id` > `email` (lowercased) > `username` > `ip_address`, `{{auto}}`/`[Filtered]` ignored), derived *before* PII scrubbing as an in-memory SHA-256 digest, then persisted only as `HMAC-SHA256(instance_secrets.user_hash_key, tenant_id || digest)[..16]` in `issue_users` / `events.user_hash`. The key is generated in migration 0012 (`gen_random_uuid`-based, 32 bytes), readable only by `maxi_loop_worker`. `issues.user_count` is a lifetime distinct count kept by the worker (merge = union, split = users of moved stored events). Spike rules get `spike_metric = events | users` (users = distinct `issue_users.last_seen` inside the window).
**Why:** Exact distinct counts and impact sorting without storing raw PII, no dictionary reversal of emails without the DB-held key, no cross-tenant correlation, and no new config/env needed.
**Rejected:** Unkeyed SHA-256 of the identity (brute-forceable emails/IPs); HyperLogLog sketches (no exact counts, harder merge/split); hashing after scrubbing (custom email rules would collapse all users into `[Filtered]`).
**Decided by:** assistant (F018 acceptance criteria agreed with user)

### D009 — 2026-11-22 — Colours & fonts identical to MaxiBoard (supersedes parts of D007)
**Decision:** Use MaxiBoard's exact tokens and type: white sidebar (`--color-bg-sidebar: #ffffff`, was `#F7F8F9`), font stack `-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif` (no Noto Sans), mono `ui-monospace, SFMono-Regular, Menlo, monospace`, lozenges 12px / radius 4 / `#E9F2FF` primary, sonner-style light toast bottom-center (was `#172B4D` navy toast), avatar initials+colour from MaxiBoard `initials.js` (`web/src/lib/avatar.ts`), touch-device 16px input guard. Off-palette hex values mapped to the MaxiBoard/Atlassian set.
**Why:** User asked for one design pattern across MaxiBoard and MaxiLoop — same colours and same font.
**Rejected:** Keeping MaxiLoop-specific sunken sidebar and navy toast from D007.
**Decided by:** user

### D008 — 2026-11-22 — Mobile shell follows MaxiBoard: drawer + bottom tab bar, 14px nav
**Decision:** Below `md` the sidebar is an off-canvas drawer (hamburger) and a 5-slot bottom tab bar (Issues / Performance / Releases / Alerts / More) is shown; sidebar nav uses MaxiBoard NavLink sizing (14px text, 18px icons). Below `lg`, item inspectors (issue, transaction, span) open as full-screen overlays and form panels stack under content. Text inputs are 16px on phones.
**Why:** Matches MaxiBoard (`maxi-code` BoardShell/TabBar) so the suite feels consistent; previous shell had no mobile layout and 12px menus.
**Rejected:** Hiding the sidebar without replacement; shrinking the desktop layout to fit phones.
**Decided by:** assistant (user asked to align UI with MaxiBoard menu fonts + responsiveness)

### D007 — 2026-11-21 — MaxiBoard design system, Lucide icons, and Maxiflow Doodle SVG project icons
**Decision:** Style the embedded React UI (`web/`) after the **MaxiBoard** light, Jira-inspired design system (`#FFFFFF` surfaces, `#F7F8F9` sunken sidebar/panels, `#172B4D` primary navy text, `#0C66E4` primary blue accent, semantic lozenges, 56px topbar + 240px sidebar + master-detail right inspector drawer + `#172B4D` bottom undo toast), using `lucide-react` for UI icons and the 12 Maxiflow Doodle SVG icons (`ProjectIcon.tsx`) for projects.
**Why:** Avoids generic dark-neon AI slop, keeps visual consistency across the user's `maxi-*` product suite (`MaxiBoard` in `maxi-code`), and prioritizes high-density glanceability for issue triage.
**Rejected:** Dark slate/indigo neon dashboard cards, monument KPI boxes, and raw `dangerouslySetInnerHTML` SVG injection.
**Decided by:** user

### D006 — 2026-11-21 — Cargo workspace with vertical-slice feature crates (Option A)
**Decision:** Organize the Rust backend as a Cargo workspace under `crates/` using vertical-slice feature crates (`crates/platform`, `crates/server`, plus per-feature crates added as features are built: `auth`, `org`, `ingest`, `issues`, `retention`, `alerts`, `mcp`, `performance`, `sourcemaps`), and mirror the feature-modular layout in the frontend (`web/src/modules/<feature>/`).
**Why:** Compiler-enforced module boundaries (e.g., `crates/mcp` cannot import `sqlx` directly and must go through service crates), clean per-module ownership matching the 1-feature-at-a-time workflow, and cheap future scaling (Design §6: splitting ingest/worker roles or swapping queue/storage behind traits).
**Rejected:** Single-crate monolith; layer-first top-level folders (`domain/`, `storage/`, `service/`, `http/`).
**Decided by:** user

### D005 — 2026-11-21 — Frontend stack & ingest protocol scope
**Decision:** Build the embedded UI with Vite + React + TypeScript + Tailwind CSS (managed with `npm`, English UI), embedded in the Rust binary via `rust-embed`. Implement Sentry's modern `/api/{project_id}/envelope/` endpoint only (skip legacy `/store/`).
**Why:** `npm` is available in the environment (no `pnpm`/`bun`), single-binary deployment requires static asset embedding, and all target modern Sentry SDKs (Python, Go, Rust, JS) use the envelope protocol.
**Rejected:** Separate frontend deployment, legacy `/store/` endpoint, Indonesian UI copy.
**Decided by:** assistant (confirmed by user)

### D004 — 2026-11-21 — Isolated Docker Compose PostgreSQL 16 for dev & tests
**Decision:** Use a project-scoped `docker-compose.yml` running `postgres:16-alpine` on `127.0.0.1:55432`, configured via untracked `.env` (`.env.example` committed with placeholders only).
**Why:** Isolates schema migrations, partitioning, and RLS tenant-isolation tests from host databases and keeps local dev reproducible.
**Rejected:** Sharing the host PostgreSQL instance on port 5432.
**Decided by:** assistant (confirmed by user)

### D003 — 2026-11-21 — Retention, partitioning, and full-event cap defaults
**Decision:** Global default retention = 30 days using daily PostgreSQL partitions (`DROP PARTITION`); store full payloads for the first 50 events per issue (+ sample), counting subsequent occurrences in hourly counters; workspace/project retention overrides are capped at `<= global`. Initial sizing targets small volume (< 100k events/day).
**Why:** Prevents event storms from inflating storage while keeping partition count manageable and avoiding bloat from unbounded overrides.
**Rejected:** 90-day or 14-day defaults; per-tenant partitioning.
**Decided by:** assistant (confirmed by user)

### D002 — 2026-11-21 — Phase 1 MVP scope, default PII scrubbing, and admin-only tenant provisioning
**Decision:** Start with Phase 1 (MVP) per `docs/sentry-lite-prd.md`, pulling baseline hardcoded PII scrubbing (Authorization/Cookie headers, password/token/secret/api_key fields) into Phase 1 for defense-in-depth. Tenants are created via admin/CLI only, with automatic single-tenant default tenant + workspace creation on first boot. Phase 2 prioritizes Webhook alerts first (followed by Telegram/Email); Phase 2b MCP uses `read` + `triage` scopes with release names mapped as commit SHAs.
**Why:** Ensures sensitive payloads are scrubbed from day one while keeping Phase 1 focused and single-tenant UX frictionless.
**Rejected:** Delaying all PII scrubbing to Phase 2; self-service tenant signup in Phase 1.
**Decided by:** assistant (confirmed by user)

### D001 — 2026-11-21 — Project identity (`maxi-loop`) and repository docs
**Decision:** Name the Rust crate, binary, and project `maxi-loop`, initialize git (`main`), and store the PRD and design docs under `docs/`.
**Why:** Matches the repository directory and `.harness/feature-list.json` while keeping design documentation versioned with the code.
**Rejected:** Keeping temporary name `sentry-lite` for the crate/binary.
**Decided by:** assistant (confirmed by user)

---
