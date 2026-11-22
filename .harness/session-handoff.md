# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Claude Code
- Feature worked on: UI polish — MaxiBoard menu typography (14px nav) + full responsive/mobile shell (drawer, bottom tab bar, overlay inspectors) + colours/fonts 1:1 with MaxiBoard (D009) — done, committed (fba02cc), merged to main and pushed

## Current state
- What works and is verified:
  - All 16 roadmap features (`F001`–`F016`), the MaxiBoard-style top-left workspace switcher, project tag folder grouping, `CreateProjectDialog` modal (triggered from sidebar `PROJECTS` `+`, topbar `+ New project`, and `ProjectsView` header/empty state), AND dual-transport MCP (**Streamable HTTP `/mcp`** + **Stdio Bridge `maxi-loop mcp`**) are implemented, tested, and live-verified.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace`, `cargo test --workspace` (94/94 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live Headless Chrome E2E on `maxi-loop-dev.service` (port `8080`).
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and E2E build/venv caches)

## What the next session MUST watch out for
- Visual tokens are now 1:1 with MaxiBoard (D009): don't reintroduce `#F7F8F9` sidebar, navy toast, Noto Sans, or ad-hoc hex colours — use `--color-*` tokens / the Atlassian label palette.
- Responsive conventions (D008): `md` = phone breakpoint (drawer + bottom tab bar in `App.tsx`), `lg` = inspector docking breakpoint (below it, `aside` inspectors use `max-lg:fixed ... top-14 z-40` overlays). Keep new views on `px-4 md:px-6`, controls `text-sm`, tables inside `overflow-x-auto`.
- Screenshot helper used for verification lived in `/tmp/mlshot/shot.py` (CDP + google-chrome headless, cookie cached; not in repo). Login endpoint rate-limits repeated logins (429).
- `CreateProjectDialog` is now a 3-step wizard; platform catalogue + SDK snippets live in `web/src/modules/projects/onboardingPlatforms.ts`. First-event detection polls `/api/v1/issues` + `/api/v1/performance/transactions` (project-scoped) every 3s while step 3 is open.
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the latest release binary (with hardened `/mcp` headers, `maxi-loop mcp` stdio bridge, and updated `McpView.tsx`) on `127.0.0.1:8080`.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Production packaging & documentation: multi-stage `Dockerfile`, `docker-compose.prod.yml`, reverse-proxy TLS guide, and comprehensive `README.md`.
2. Any operational enhancements or real-world deployment tooling requested by the user.
