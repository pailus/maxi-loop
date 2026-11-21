# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-21
- Assistant: Antigravity
- Feature worked on: F008 (Embedded React UI: Login, Projects & DSN, Issues list/detail, and hourly chart) — done

## Current state
- What works and is verified:
  - Designed 4-screen FigCraft mockup (`/home/p4ck/Documents/maxi-loop-ui.figcraft`) via FigCraft MCP using the **MaxiBoard** light Jira-inspired design system (`D007`)
  - Implemented vertical-slice React frontend in `web/src/`:
    - `web/src/index.css`: MaxiBoard tokens (`#FFFFFF`, `#F7F8F9`, `#172B4D`, `#0C66E4`), `.pm-lozenge`, `.pm-tab`, `.pm-input`, `.pm-bicon`
    - `web/src/lib/api.ts`: typed HTTP client for auth, workspaces, projects/DSN rotation, issues list/detail/events/stats, and status triage with automatic `X-CSRF-Token` header attachment
    - `web/src/modules/projects/ProjectIcon.tsx`: 12 Maxiflow Doodle SVG project icons rendered as pure typed React JSX components
    - `web/src/modules/auth/LoginView.tsx`: single-tenant default workspace login card with optional tenant slug disclosure and `$ maxi-loop bootstrap` tip
    - `web/src/modules/issues/` (`IssuesView.tsx`, `IssueDetailDrawer.tsx`, `IssueDetailFullView.tsx`, `OccurrenceChart.tsx`, `StackTraceView.tsx`): 3-pane MaxiBoard master-detail layout, status tabs, filters (`project`, `environment`, `release`, `query`, `sort`), pagination, `1h`/`1d` occurrence histogram chart, `In-app only`/`All frames` stack trace code viewer, PII-scrubbed sample event payload inspector, and `Resolve`/`Ignore`/`Unresolve` triage with `#172B4D` undo toast
    - `web/src/modules/projects/ProjectsView.tsx`: project cards with copyable Sentry DSN, DSN rotation, retention/rate-limit metadata, and right-hand Create Project drawer with MaxiBoard icon picker + `sentry_sdk.init` snippet
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (39/39 passed), `cargo build --workspace`, and live smoke test
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only untracked `.env` for local dev)

## What the next session MUST watch out for
- Unresolved blockers: none
- Decisions still waiting for user confirmation: none
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host

## Suggested next steps
1. Start **F009**: Envelope load-test generator and real Python (Django/FastAPI) Sentry SDK E2E verification:
   - Build synthetic envelope load-test generator / benchmark subcommand or tool to measure ingest throughput, latency, and `429 Too Many Requests` (`Retry-After` / `X-Sentry-Rate-Limits`) behavior against a running `maxi-loop` server
   - Verify real Python `sentry-sdk` (FastAPI/Django) E2E exception ingestion & grouping by only changing the DSN
   - Measure and document baseline throughput and idle RSS memory footprint (completing Phase 1 MVP).
