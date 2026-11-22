# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: MaxiBoard-style Top-Left Workspace Switcher, Create Workspace Dialog, & Workspace Settings/MCP Wiring — done

## Current state
- What works and is verified:
  - All 16 roadmap features (`F001`–`F016`) plus the new MaxiBoard-style top-left workspace switcher & creation UX are implemented, tested, and live-verified.
  - Top-left sidebar workspace button (`web/src/modules/workspaces/WorkspaceSwitcher.tsx`) now opens a portaled MaxiBoard dropdown (`Ctrl/⌘+K` supported, keyboard navigation, search box when `>= 6` workspaces, Starred / Recently opened / All sections, active workspace parameters strip showing tenant/mode, role, effective retention days & default rate limit, plus fixed bottom actions: **Workspace settings & limits**, **Members & access**, and **Create workspace**).
  - `CreateWorkspaceDialog.tsx` creates workspaces via `POST /api/v1/workspaces` with name, auto-generated editable slug, MaxiBoard 9-color swatches, optional retention override (`1..=tenant.retention_days`), and optional default project rate limit override (`/min`), then auto-switches into the new workspace.
  - `TeamAndSettingsView.tsx` adds a `+ Create workspace` button, workspace name + badge color editing, and supports jumping directly to `settings` or `members` from the workspace dropdown; `McpView.tsx` scopes workspace MCP tokens to the active `workspace_slug`.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace`, `cargo test --workspace` (89/89 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live Headless Chrome E2E smoke test on `maxi-loop-dev.service` (port `8080`).
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and E2E build/venv caches)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the latest release binary (with the new top-left workspace switcher & create workspace dialog) on `127.0.0.1:8080`.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Production packaging & documentation: multi-stage `Dockerfile`, `docker-compose.prod.yml`, reverse-proxy TLS guide, and comprehensive `README.md`.
2. Optional: `maxi-loop mcp` stdio bridge subcommand or any custom operational enhancements the user requests.
