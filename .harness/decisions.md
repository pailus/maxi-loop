# Decision Log

> Decisions with lasting impact (architecture, libraries, data model, API
> shape, conventions, trade-offs the user chose). Newest at the TOP.
> Before proposing to change something listed here, read why it was decided;
> reversing a decision needs the user's explicit OK and a new entry.
> Keep each entry to a few lines.

---

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
