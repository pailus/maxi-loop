# Decision Log

> Decisions with lasting impact (architecture, libraries, data model, API
> shape, conventions, trade-offs the user chose). Newest at the TOP.
> Before proposing to change something listed here, read why it was decided;
> reversing a decision needs the user's explicit OK and a new entry.
> Keep each entry to a few lines.

---

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
