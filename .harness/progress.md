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

### 2026-11-22 — Claude Code — F019: Performance alerts (p95 latency / error rate)

**Status at end of session:** done

**Done:**
- `migrations/0013_performance_alerts.sql`: `alert_rules` trigger types `perf_p95` / `perf_error_rate` + `transaction_name`, `perf_threshold`, `perf_window_minutes`, `perf_min_count` (CHECKs per type); `alert_deliveries.issue_id` nullable + `transaction_name`, `metric_value`, `subject`, perf cooldown index
- `crates/alerts`: `evaluate_performance_rules` on the 60s engine tick (p95 from merged log histograms via `maxi_loop_performance::histogram::estimate_percentile`, error rate from counts), cooldown per (rule, project, transaction), audit log rows; validation for create/update (per-type threshold ranges, window, min count, transaction name); payload `performance` context, `issue_id` omitted for perf alerts
- UI (Alerts): p95 / error-rate triggers with transaction, threshold, window, min transactions; rule summary; delivery log handles issue-less rows
- Tests: `crates/server/tests/f019_performance_alerts.rs` (fire / no-fire / min traffic / cooldown / audit log / 9 validation negatives), engine unit tests, E2E `tests/e2e/python/verify_performance_alerts.py` (official sentry-sdk, cleans up its rules)

**Verification run:**
- `npm --prefix web run build` → PASS
- `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` → PASS
- `cargo test --workspace` → PASS (115 passed, 0 failed)
- `cargo build --workspace` + `cargo build --release -p maxi-loop` → PASS; dev service restarted (migration 0013 applied); `cargo tree --depth 1 -p maxi-loop-mcp` → 0 direct `sqlx`
- `verify_performance_alerts.py` → PASS (delivery `failed` = no SMTP on dev host, by design no off-host traffic)
- Headless Chrome screenshots (Alerts rules + perf form, delivery log) → OK

**Security notes:**
- New rule inputs allow-listed / range-checked server-side and by DB CHECKs (negatives tested); queries parameterized; same role checks + RLS as existing alert endpoints; delivery path (SSRF guard, header sanitizing) unchanged; only internal crate dependency added (`alerts` → `performance`).

**Key decisions & why:**
- D011 (reuse `alert_rules`/`alert_deliveries`, nullable `issue_id`)

**Not finished / blockers:**
- None.

### 2026-11-22 — Claude Code — F018: Users affected per issue + impact sorting

**Status at end of session:** done

**Done:**
- `migrations/0012_issue_users.sql`: `instance_secrets` (worker-only `user_hash_key`), `issues.user_count` + impact index, `events.user_hash`, RLS-enforced `issue_users`, `alert_rules.spike_metric` (`events`/`users`)
- `crates/ingest/src/user_identity.rs`: pre-scrub user digest (`id` > `email` > `username` > `ip_address`), tenant-scoped HMAC hash; worker upserts `issue_users` and bumps `user_count`; performance issues count the transaction user too
- `crates/issues`: `user_count` in views, `sort=users`; merge unions users, split carries users of moved events
- `crates/alerts`: spike rules with `spike_metric = users` (distinct users seen in window), allow-listed on create/update
- MCP `list_issues`/`get_issue`: `user_count` + `sort=users`
- UI: Users column + "Sort: Most users" in Issues, "Users affected" in drawer & full view, metric picker for spike rules
- Tests: `crates/server/tests/f018_users_affected.rs`, unit tests in `user_identity.rs`, E2E `tests/e2e/python/verify_users_affected.py` (official sentry-sdk)

**Verification run:**
- `npm --prefix web run build` → PASS
- `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` → PASS
- `cargo test --workspace` → PASS (112 passed, 0 failed)
- `cargo build --workspace` + `cargo build --release -p maxi-loop` → PASS; dev service restarted (migration 0012 applied)
- `verify_users_affected.py` → PASS; `verify_performance_issues.py` regression → PASS
- Headless Chrome screenshots (Issues list + drawer desktop, mobile) → OK

**Security notes:**
- Raw user identity never persisted for counting (keyed HMAC, 16 bytes, tenant-scoped); key readable only by `maxi_loop_worker` (test asserts `maxi_loop_app` is denied); `sort`/`spike_metric` allow-listed (+ DB CHECK), negative tests for both; no new dependencies (`hmac` already in workspace). Stored event payloads still contain the SDK-sent `user` object as before (unchanged behaviour; scrub via PII rules).

**Key decisions & why:**
- D010 (keyed tenant-scoped user hash, exact distinct counts in `issue_users`)

**Not finished / blockers:**
- None. Python SDK drops `user.ip_address` unless `send_default_pii=True`, so IP-only users count only when sent.

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
