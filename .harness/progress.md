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

### 2026-11-23 — Claude Code — F027 production packaging + deploy to loop.maxiflow.id

**Status at end of session:** done

**Done:**
- `Dockerfile`, `.dockerignore`, `docker-compose.prod.yml`, `deploy/deploy.sh`, `deploy/nginx/loop.maxiflow.id.conf`, `deploy/README.md`.
- Server (shared VPS, user `maxiflow`): `~/maxi-loop/.env` generated there (random hex DB password), certbot cert `loop.maxiflow.id` (exp. 2027-02-20, auto-renew), vhost in `/etc/nginx/conf.d/`, stack up, admin `admin@maxiflow.id` bootstrapped once; its random password is only in `~/maxi-loop/ADMIN_CREDENTIALS` (0600).

**Verification run:**
- `docker build` (podman locally) → PASS, 112 MB, runs as uid 10001
- `deploy/deploy.sh --skip-build` → healthy; `https://loop.maxiflow.id/healthz` → 200; SPA served with CSP/X-Frame-Options; anon `/api/v1/auth/me` → 401; admin login via public URL → 200 + `/me` OK; wrong password → 401 without cookie
- Container: read-only rootfs, cap_drop ALL, no-new-privileges, `COOKIE_SECURE=true`, spool dir 0700, DB port not published; `code.maxiflow.id` still 200

**Security notes:**
- No secrets in git or chat; `.env`/credentials only on the server. Real client IP trusted only from Cloudflare ranges. Bootstrap env kept out of `.env` (would reset the admin password on restart).

**Key decisions & why:**
- D019.

**Not finished / blockers:**
- User must log in, change the admin password and delete `~/maxi-loop/ADMIN_CREDENTIALS`. No SMTP configured on prod (email alerts/invites off). No automated DB backup yet.

---

### 2026-11-23 — Claude Code — test fix: phase2 spike-alert flake

**Status at end of session:** done

**Done:**
- `crates/server/tests/phase2_pii_releases_alerts.rs`: before running the instance-wide `evaluate_spike_rules`, disable spike rules left by earlier `p2-alpha-*` test tenants (worker tx). Assertion `sent_spikes == 1` unchanged. Handoff test-count line corrected.

**Verification run:**
- test run twice back to back → PASS both; `cargo fmt --check`, clippy → PASS; `cargo test --workspace --no-fail-fast` → 145 passed, 0 failed

---

### 2026-11-22 — Claude Code — F026 per-member project access (Stage B)

**Status at end of session:** done (browser click-through pending by user)

**Done:**
- Migration 0016; `auth/src/project_limits.rs`; limits in `AuthSession` (`workspace_project_limits`, loaded per request) and `project_access()`; MCP `role_within_scope` honours limits; members API (`project_ids`, `PUT /members/{id}/projects`, promotion clears limits, `projects`/`project_limited` in views); invitations carry limits to accept.
- Web: `admin/ProjectAccessPicker.tsx` (All / Selected checklist) in add/invite form and inline "Edit project access" per member; limits shown in members/invites tables and on the accept page.

**Verification run:**
- `cargo test -p maxi-loop --test f026_project_access` → PASS (visibility, issue reads/writes, assignment eligibility + auto-unassign on narrowing, validation, member can't widen self, deleted last project → no projects, MCP workspace/project tokens, promotion clears, invite limits end to end)
- b0/f024/f022/mcp_server/phase5/auth_org suites → PASS; `npm --prefix web run build`, fmt, clippy → PASS; `cargo test --workspace --no-fail-fast` → 144 passed, 1 failed (known `phase2_pii_releases_alerts.rs:777` shared-DB spike flake, fails on pre-B0 code too)
- Release build + restart `maxi-loop-dev`; anon `PUT /members/1/projects` → 401; bundle contains new UI.

**Security notes:**
- Limits fail closed (flag + cascade), checked server-side on every path via D017; only workspace managers edit limits; parameterized SQL; no new dependencies.

**Key decisions & why:**
- D018.

**Not finished / blockers:**
- None for this feature. Invite e-mail delivery, forgot-password and assignee notifications remain open (need an e-mail sender outside `alerts`).

---

### 2026-11-22 — Claude Code — F025 workspace isolation hardening (B0)

**Status at end of session:** done

**Done:**
- `platform/src/access.rs` (`ProjectAccess`, `push_access_sql`, `AccessGuard`); `AuthSession::{project_access, can_read_project, can_triage_project, can_read_whole_workspace}` + `AccessGuard` impls for `AuthSession` and `Role`; `enforce_project_read/write` now project-aware.
- Issues: list filter in SQL (`IssueListFilter.access`, `#[serde(skip)]`), read checks on stats/events/event/notes, release/source-map/PII/alert services take `&dyn AccessGuard`; performance: `PerformanceScope.access` applied to all top-level queries + compare-releases project check; alerts: scope-based channel/rule management, filtered lists and deliveries; MCP: scoped token creation, `role_within_scope` at auth, tenant-wide listing/revoke for tenant admins only; API-token and project lists via `can_read_project`.
- Tests calling `create_alert_channel` pass `&Role::TenantOwner` (signature change only).

**Verification run:**
- `cargo test -p maxi-loop --test b0_workspace_isolation` → PASS; the same test on the pre-fix commit (worktree) → FAIL, soft-run listed 26 failing checks (every hole reproduced).
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` → PASS; `cargo test --workspace --no-fail-fast` → 141 passed, 1 failed: `phase2_pii_releases_alerts.rs:777` (`sent_spikes` 2 vs 1) — fails identically on the pre-B0 commit today (shared-DB leftover spike rules, known flake), not a regression.

**Security notes:**
- Fixes privilege escalation via MCP tokens, cross-workspace reads (performance, issue stats/events/notes, list totals), cross-workspace writes (alerts, PII rules, releases, source maps incl. delete rolled back on denial). SQL stays parameterized; dynamic identifiers are compile-time constants.

**Key decisions & why:**
- D017.

**Not finished / blockers:**
- Stage B (per-project membership limits) next.

---

### 2026-11-22 — Claude Code — F024 issue assignment (UI + MCP)

**Status at end of session:** done (browser click-through pending by user)

**Done:**
- `migrations/0015_issue_assignment.sql` (column, `user_can_triage_project`, unassign trigger); `crates/issues/src/assignment.rs`; `assignee` on `IssueSummaryView` (all three issue SELECTs); `assigned_to` list filter (`me` resolved in HTTP/MCP layers); routes `/issues/{id}/assignee` (PATCH/POST) and `/issues/{id}/assignable-members` (+ `/t/.../p/...` variants); `get_issue_summary` helper.
- MCP: `assign_issue`, `list_assignable_members`, `list_issues.assigned_to`, assignee in `list_issues`/`get_issue`, triage prompt step; `mcp_server.rs` tool count 11 → 13 (intended: two new triage tools).
- Web: `issues/AssigneePicker.tsx` (drawer + full view, "Assign to me", unassign, search), assignee avatar on list rows, "Assignee" filter (All / Assigned to me / Unassigned).

**Verification run:**
- `cargo test -p maxi-loop --test f024_issue_assignment` → PASS (eligibility incl. other workspace/viewer/other tenant/unknown, viewer 403, cross-tenant 404, no-op, filters me/none/email/invalid, timeline notes, auto-unassign on delete + downgrade, MCP assign me/email-rejected/null, read-only token denied)
- `npm --prefix web run build`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` → PASS
- `cargo test --workspace --no-fail-fast` → all PASS except known flake `phase2_pii_releases_alerts.rs:777` (passes 2/2 when run alone; see handoff)
- Release build + restart `maxi-loop-dev`; :8080 anon assign/assignable → 401; bundle contains new UI. No browser click-through.

**Security notes:**
- Server-side checks: triage role + `enforce_project_write` (API tokens need write + same project), tenant RLS (foreign/unknown users resolve as not assignable without revealing existence), assignable list limited to triagers, LIMIT 500. Parameterized SQL only; `tracing` added to issues crate from the existing workspace dependency (no new package).

**Key decisions & why:**
- D016.

**Not finished / blockers:**
- Stage B (per-member project access + invite project picker) not started. Trigger-based unassign writes no timeline note. No notification to the assignee.

---

### 2026-11-22 — Claude Code — F022 invite links + F023 change password (+ Team & settings full width)

**Status at end of session:** done (browser click-through pending by user)

**Done:**
- `migrations/0014_invitations.sql`; `crates/auth/src/invitations.rs` (create/list/revoke, public preview/accept); `members::resolve_grant_target` and `service::open_session` extracted for reuse; `service::change_password` + `POST /api/v1/auth/password`.
- Web: Members tab "Invite link" / "Create directly" toggle, show-once link box, pending invitations table with revoke; `auth/AcceptInviteView.tsx` (opened from `/#invite=`); `auth/ChangePasswordDialog.tsx` (sidebar "Change password").
- Team & settings tabs no longer capped at `max-w-5xl/6xl`.

**Verification run:**
- `cargo test -p maxi-loop --test f022_invitations_password` → PASS (new: isolation, single-use, revoke, re-invite, expiry, existing-account password, weak password, member forbidden, CSRF, other sessions revoked)
- `npm --prefix web run build`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` → PASS; `cargo test --workspace --no-fail-fast` → PASS (0 failures)
- Release build + restart `maxi-loop-dev`; curl on :8080: preview/accept bad token 401, anon `/invitations` and `/auth/password` 401; served bundle contains the new UI. No browser click-through (no account credentials for the dev DB).

**Security notes:**
- Token: CSPRNG 256-bit, SHA-256 at rest, fragment-only, 7-day expiry, single-use with row lock; hash never returned. Admin endpoints: session + CSRF, scope/role hierarchy checks, tenant RLS; cross-tenant revoke → 404. Accept for existing accounts verifies password and uses the login rate limiter; password change rate-limited, verifies current password, revokes other sessions. Generic errors; tokens/passwords not logged. No new dependencies.

**Key decisions & why:**
- D015.

**Not finished / blockers:**
- Invite email delivery and forgot-password reset (both need an email sender outside `alerts`).

---

### 2026-11-22 — Claude Code — UI: menu & tabs kept in URL (refresh-safe)

**Status at end of session:** done (browser check pending by user)

**Done:**
- New `web/src/lib/urlState.ts`; App menu (`view`), project/workspace, back/forward (`popstate`); tabs in Issues (`status`), issue selection (`issue`, `full`), full-view tab (`detail`), Performance (`tab`, `trace`), Releases, Alerts, MCP, Team & settings (`tab`).

**Verification run:**
- `npm run build` (tsc + vite) → PASS; `cargo build --release -p maxi-loop` + restart `maxi-loop-dev` → PASS
- Automated browser check NOT run (creating a throwaway test tenant in the dev DB was denied).

**Key decisions & why:**
- D014.

**Not finished / blockers:**
- Manual check by user on :8080.

---

### 2026-11-22 — Claude Code — UI polish: softer hover & card borders

**Status at end of session:** done

**Done:**
- `web/src/index.css`: lighter `--pm-col` / `--color-bg-muted` / `--pm-card-hover` surfaces, softer `--color-bg-hover`, new `--color-bg-hover-strong`, lighter `--color-border`, card shadow without extra ring, 150ms colour transitions on interactive elements.
- Filled (`--pm-col`) buttons in Issues/IssueDetail/Projects and neutral badges (Alerts, Breadcrumbs, IssueDetailFullView) switched to `--color-bg-hover-strong` so they keep contrast.

**Verification run:**
- `npm run build` (web) → PASS; `npx tsc --noEmit` → PASS
- `cargo build --release -p maxi-loop` + `systemctl --user restart maxi-loop-dev` → new CSS served on :8080 (UI is embedded via rust-embed; restart needed after web changes)

**Key decisions & why:**
- D013 (user-approved deviation from D009 MaxiBoard tokens).

**Not finished / blockers:**
- none (visual check left to user on the running dev server)

---

### 2026-11-22 — Claude Code + Antigravity — F021: Phase 2 load test & benchmark report

**Status at end of session:** done

**Done:**
- Extended `crates/loadgen` (`cli.rs`, `generator.rs`, `runner.rs`, `main.rs`) with `--kind error|transaction|mixed` (`PayloadKind`), realistic FastAPI-style transaction envelopes with DB + `http.client` spans where 1 in 5 templates emits a 6-query N+1 DB pattern (triggering `F017` detection and `'issue'` trace sampling), `payload_kind` in JSON/text reports, and 3 new unit tests (9 total in `maxi-loop-loadgen`)
- Added automated benchmark harness `tests/e2e/python/bench_phase2.py` running release `maxi-loop serve` on a dedicated `bench-phase2` tenant both without and with `INGEST_SPOOL_DIR`, measuring binary sizes, idle/peak RSS, HTTP req/s & latency, `429` headers, and end-to-end persistence drain rate
- Wrote `docs/benchmarks-phase2.md` with full tables, PostgreSQL verification of N+1 DB performance issues + sampled zstd traces, Phase 1 comparison, and reproduction commands; updated `.harness/ARCHITECTURE.md` and archived 2 oldest progress entries to `.harness/progress-archive.md`

**Verification run:**
- `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo check --workspace` → PASS
- `cargo build --workspace` / `cargo build --release -p maxi-loop -p maxi-loop-loadgen` → PASS
- `cargo test -p maxi-loop-loadgen` → PASS (9/9)
- `cargo test --workspace --no-fail-fast` → 132 passed, 1 failed (`phase2_configurable_pii_releases_alerts_ssrf_and_tenant_isolation`: pre-existing shared-DB spike rule count issue documented in handoff)
- `tests/e2e/python/bench_phase2.py --out /tmp/bench-phase2.json` → PASS across all 8 scenarios (240,600 accepted / 240,600 persisted / 0 lost / 0 5xx; 1,831,261 clean `429`s with 100% `Retry-After` + `X-Sentry-Rate-Limits`; idle RSS 12.0 MiB, peak RSS 115.3 MiB no-spool / 106.0 MiB spool; sustained 3,674 err/s, 3,220 tx/s, 2,222 mixed/s; spool overhead 0.5%–3.2%)

**Security notes:**
- `bench_phase2.py` generates ephemeral CSPRNG admin credentials (`secrets.token_urlsafe(24)`) passed only via env vars (never printed or passed in argv), passes `SENTRY_DSN` via env, and binds loopback only; `ParsedDsn` redacts `public_key` in `Debug`; `--kind` validated against a strict allow-list with negative test; 0 new dependencies added.

**Key decisions & why:**
- Raised per-project (`1M/min`) and tenant aggregate (`10M/min`) quotas inside the dedicated `bench-phase2` tenant so benchmarks measure true ingest + PostgreSQL persistence and channel-full `429` backpressure instead of artificial quota throttling.

**Not finished / blockers:**
- None for F021; pre-existing `phase2_pii_releases_alerts.rs:777` failure untouched (out of scope).

### 2026-11-22 — Claude Code — F020: Optional ingest write-ahead spool

**Status at end of session:** done

**Done:**
- Security tooling check first (no code change): `cargo-audit` installed; `cargo audit` → 2 advisories (`libsqlite3-sys` RUSTSEC-2026-0459, `rsa` RUSTSEC-2023-0071) only in `Cargo.lock` as unused optional `sqlx` drivers (`cargo tree --target all -e all` empty = not compiled), `chrono` unmaintained warning; `npm audit` 0 vulnerabilities. `gitleaks` install was denied by the sandbox → replaced by an `rg` secret-pattern scan of tracked files + history (only test placeholders found)
- `crates/ingest/src/spool/` (`mod.rs`, `codec.rs`, `tests.rs`): CRC-framed binary segments, writer lock doing write + `try_send`, checkpoint (tmp + rename), startup replay, rotation at 64 MiB / 30 s, 1 s `fdatasync` + checkpointed-segment deletion, `0700`/`0600`, `flock`
- `INGEST_SPOOL_DIR` config (absolute path), `IngestState::with_spool` (503 on spool write error), worker `SpoolAck` commit after each batch, `AppState::with_spool`, replay in `main.rs` after PII cache load / before listen
- Workspace `rust-version` 1.85 → 1.89 (`File::try_lock`; let-chains already needed 1.88)
- Tests: 13 spool unit tests, `crates/server/tests/f020_ingest_spool.rs` (crash → replay once → no duplicates), E2E `tests/e2e/python/verify_ingest_spool.py` (real binary, SIGKILL mid-burst)

**Verification run:**
- `npm --prefix web run build` / `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` → PASS
- `cargo test --workspace --no-fail-fast` → 129 passed, 1 failed: `phase2_pii_releases_alerts` (pre-existing: also fails on clean `main` via `git stash`, global `evaluate_spike_rules` counts old test tenants — see handoff)
- `cargo build --workspace` + `cargo build --release -p maxi-loop` → PASS; `maxi-loop-mcp` 0 direct `sqlx`; dev service restarted
- `verify_ingest_spool.py` → PASS twice (1208 accepted / 79 replayed / 1208 stored; 1200 / 210 / 1200; 0 lost, 0 duplicates)
- Regression E2E on new binary: `verify_sentry_sdk.py`, `verify_users_affected.py` → PASS

**Security notes:**
- Spool holds raw pre-scrub payloads: dir must be `0700` (startup refuses group/other bits, never chmods), files `0600`, segments deleted ≤ ~30 s after checkpoint; exclusive lock; frames CRC- and bounds-checked on replay (no allocation from on-disk lengths); no payloads in logs; 503 body generic. No new endpoint/auth change (spool runs after DSN auth + rate limit). `crc32fast` was already in the lockfile.

**Key decisions & why:**
- D012 (spool design, at-least-once, MSRV 1.89)

**Not finished / blockers:**
- none for F020; pre-existing flaky `phase2_pii_releases_alerts` not fixed (out of scope, reported to user)
