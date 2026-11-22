# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Claude Code
- Feature worked on: `F018` — Users affected per issue + impact sorting (migration 0012, keyed tenant-scoped user hash D010, `sort=users`, `spike_metric=users`, UI Users column) — done and committed. `F019` is next in the same session.

## Current state
- What works and is verified:
  - `F001`–`F018` are all `done` and verified (`F018`: 112/112 cargo tests, `verify_users_affected.py` + F017 E2E regression PASS, screenshots OK).
  - `F017` checks all passed: `npm --prefix web run build`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (107/107 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, `cargo tree --depth 1 -p maxi-loop-mcp` (0 direct `sqlx`), `tests/e2e/python/verify_performance_issues.py`, Sentry SDK & MCP E2E regressions, and desktop/trace/mobile UI screenshots on `maxi-loop-dev.service` (port `8080`).
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and E2E build/venv caches)

## What the next session MUST watch out for
- Visual tokens are 1:1 with MaxiBoard (D009): don't reintroduce `#F7F8F9` sidebar, navy toast, Noto Sans, or ad-hoc hex colours — use `--color-*` tokens / the Atlassian label palette.
- Responsive conventions (D008): `md` = phone breakpoint (drawer + bottom tab bar in `App.tsx`), `lg` = inspector docking breakpoint (below it, `aside` inspectors use `max-lg:fixed ... top-14 z-40` overlays). Keep new views on `px-4 md:px-6`, controls `text-sm`, tables inside `overflow-x-auto`.
- `instance_secrets.user_hash_key` (migration 0012) must stay readable only by `maxi_loop_worker`; rotating it would split every user into a new hash (counts would double) — don't rotate without a recount plan.
- `alert_deliveries.issue_id` is `NOT NULL` + FK to `issues`: performance alerts (`F019`) are not issues and need their own delivery/cooldown model.
- Migration `0011_performance_issues.sql` is applied on the running dev DB (`maxi-loop-dev.service` on `127.0.0.1:8080` runs the latest release binary built with F018; migration 0012 applied).
- `issues.issue_type` allow-list lives in both `migrations/0011_performance_issues.sql` (`issues_issue_type_chk`) and `crates/issues/src/service.rs` (`ISSUE_TYPES`); keep them in sync if new performance detectors are added.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets (rerun passes).
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. `F019`: Performance alerts (p95 latency and error-rate thresholds per transaction/project evaluated every minute from `transaction_minute_metrics`).
2. Production packaging & documentation (`Dockerfile`, `docker-compose.prod.yml`, reverse-proxy TLS guide).
