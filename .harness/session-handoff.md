# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: MCP Stdio Bridge (`maxi-loop mcp`), Streamable HTTP Protocol Header Hardening & Live Claude Code CLI E2E — done

## Current state
- What works and is verified:
  - All 16 roadmap features (`F001`–`F016`), the MaxiBoard-style top-left workspace switcher, AND dual-transport MCP (**Streamable HTTP `/mcp`** + **Stdio Bridge `maxi-loop mcp`** modeled after `maxi-code`'s `agentway mcp-board`) are implemented, tested, and live-verified.
  - `maxi-loop mcp` (`crates/mcp/src/stdio.rs`, wired in `crates/server/src/cli.rs` & `main.rs`) runs without `DATABASE_URL`, keeps `stdout` strictly line-delimited JSON-RPC 2.0 (diagnostics on `stderr`), forwards over `hyper` + `tokio-rustls` + `webpki-roots`, propagates `Mcp-Session-Id` & `MCP-Protocol-Version`, and handles unconfigured env / `401` / `429` / EOF `DELETE /mcp` cleanly.
  - `web/src/modules/mcp/McpView.tsx` now offers a toggle between **Streamable HTTP** and **Stdio Bridge (`maxi-loop mcp`)** config snippets and lists all 11 tools + 4 prompt templates.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace`, `cargo test --workspace` (94/94 passed), `cargo build --workspace`, `cargo build --release -p maxi-loop`, `cargo tree --depth 1 -p maxi-loop-mcp` (0 direct `sqlx` dependency), and live E2E `python3 tests/e2e/verify_mcp.py --test-claude` proving **real Claude Code CLI** connects and executes MCP tools cleanly over **both** Streamable HTTP and Stdio Bridge.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and E2E build/venv caches)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the latest release binary (with hardened `/mcp` headers, `maxi-loop mcp` stdio bridge, and updated `McpView.tsx`) on `127.0.0.1:8080`.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Production packaging & documentation: multi-stage `Dockerfile`, `docker-compose.prod.yml`, reverse-proxy TLS guide, and comprehensive `README.md`.
2. Any operational enhancements or real-world deployment tooling requested by the user.
