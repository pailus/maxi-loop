# Session Handoff

> Fill this in BEFORE the session ends — even if the task is unfinished.
> The assistant in the next session MUST read this before starting work.
> Overwrite this file at the end of every session (the old content is already
> recorded in progress.md).

## Last session
- Date: 2026-11-22
- Assistant: Antigravity
- Feature worked on: F012 (Phase 3: Performance monitoring — transactions, spans, 1-minute duration histograms, sampled zstd traces, distributed waterfall, MCP `list_transactions` / `get_trace`) — done

## Current state
- What works and is verified:
  - **Phase 1 (`F001`–`F009`), Phase 2 (`F010`), Phase 2b (`F011`), and Phase 3 (`F012`) are complete.**
  - Added migration `migrations/0005_phase3_performance.sql`: immutable `merge_histogram_buckets` + `sum_histogram_buckets` aggregate, `transaction_minute_metrics`, daily-partitioned `traces` (`spans_zstd BYTEA`), `ensure_traces_partition`, `drop_expired_traces_partitions`, and RLS (`maxi_loop_app` + `maxi_loop_worker`).
  - Created vertical-slice crate `crates/performance` (`maxi-loop-performance`) with 76-bucket log-scale duration histograms (`histogram.rs`), bounded zstd trace blob codec (`codec.rs`), Sentry transaction parser + PII scrubber + head-consistent sampling (`error`/`slow`/1–5% deterministic `trace_id` hash) + batch writer (`ingest_proc.rs`), distributed cross-service trace waterfall assembler (`waterfall.rs`), RLS-enforced `PerformanceService` (`service.rs`), and `/api/v1/performance/*` HTTP API (`http.rs`).
  - Extended `crates/ingest` to accept `"transaction"` Sentry envelope items alongside `"event"` items and flush 1-minute histograms + sampled zstd traces in 100ms worker batches.
  - Extended `crates/retention` to pre-create & `DROP TABLE` expired `traces_YYYY_MM_DD` partitions and batch-delete override/global expired `traces` & `transaction_minute_metrics`.
  - Extended `crates/mcp` (still **zero direct `sqlx` dependency**, `D006`) with `list_transactions` (`F-MCP-12`) and `get_trace` (`F-MCP-13`) with scope enforcement, output PII scrubbing, untrusted-data wrapping, truncation markers, and slow-span flagging.
  - Added MaxiBoard UI `web/src/modules/performance/PerformanceView.tsx` and `TraceWaterfallView.tsx` (**Performance** sidebar nav item + `?view=performance&trace=...` deep links), rebuilt embedded SPA.
  - All checks pass: `npm --prefix web run build`, `npm --prefix web audit` (0 vulnerabilities), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (65/65 passed, incl. `phase3_performance` integration test & 8 `maxi-loop-performance` unit tests), `cargo build --workspace`, `cargo build --release -p maxi-loop`, and live smoke check against restarted `maxi-loop-dev.service` on port `8080`.
- What is in progress but unfinished: none
- Files left "dirty" / half-done (if any): none (only gitignored `.env`, `.env.local`, and `tests/e2e/python/.venv/`)

## What the next session MUST watch out for
- Running service: `maxi-loop-dev.service` (systemd user unit) is running the updated `F012` release binary on `127.0.0.1:8080` for the user's Agentway tunnel preview.
- Test hygiene note: `crates/server/tests/phase2_pii_releases_alerts.rs` calls global `evaluate_spike_rules(&pool, ...)` across all tenants in the shared dev DB; if older test tenants' 15-minute cooldown expires within their 60-minute spike window, the first run can count those older rules until cooldown resets.
- Tests skipped on purpose (and why): `cargo audit` and `gitleaks` are not installed on this host.

## Suggested next steps
1. Start **Phase 4 — `F013`**: React Source Maps upload per release (matched by release + filename or debug id) and bounded LRU ingest-time stack frame resolution before fingerprinting (`sourcemap` crate).
