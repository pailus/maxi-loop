# Maxi Loop — Phase 2 Performance & Spool Benchmark Report

Recorded for **F021** against the full Phase 2–5 + P2 release (error ingest, custom PII scrubbing, users affected, performance transactions/spans/histograms, zstd trace sampling, span performance issue detectors `F017`, performance alerts `F019`, and optional crash-safe write-ahead spool `F020`). Compares behaviour **without** and **with** `INGEST_SPOOL_DIR` enabled and against the Phase 1 baseline ([`docs/benchmarks-phase1.md`](benchmarks-phase1.md)).

## Test Environment

| Component | Details |
|---|---|
| Host OS / Arch | Linux `6.8.0-60-generic` `x86_64` |
| CPU | AMD Ryzen 7 1700 Eight-Core Processor (16 logical threads) |
| Host Memory | 31 GiB RAM |
| Rust Toolchain | `rustc 1.99.1` (`--release` profile) |
| Database | PostgreSQL `16.13` (`postgres:16-alpine` via Docker Compose on `127.0.0.1:55432`) |
| Server Config | `INGEST_CHANNEL_CAPACITY=8192`, `MAX_FULL_EVENTS_PER_ISSUE=50`, `DB_MAX_CONNECTIONS=10`, `RUST_LOG=warn`, `SINGLE_TENANT_MODE=false` |
| Loadgen Config | `maxi-loop-loadgen`, `gzip` compression, `concurrency=32`, `unique_issues=25` templates (`1` in `5` transaction templates includes a 6-query N+1 DB pattern triggering `F017` detection + forced `'issue'` trace sampling) |
| Harness | [`tests/e2e/python/bench_phase2.py`](../tests/e2e/python/bench_phase2.py) against a dedicated `bench-phase2` tenant (project quota `1,000,000/min`, tenant aggregate `10,000,000/min` so measurements exercise true ingest + PostgreSQL persistence and channel-full `429` backpressure) |

## Binary Footprint & Memory (RSS)

| Metric | Phase 1 (F009) | Phase 2 — No Spool | Phase 2 — With Spool (`INGEST_SPOOL_DIR`) |
|---|---:|---:|---:|
| `target/release/maxi-loop` (server + embedded React SPA) | 13.0 MB | **22.7 MiB** (`23,760,784 B`) | **22.7 MiB** (`23,760,784 B`) |
| `target/release/maxi-loop-loadgen` | 3.0 MB | **2.97 MiB** (`3,111,400 B`) | **2.97 MiB** (`3,111,400 B`) |
| **Idle RSS (`VmRSS`)** after startup, migrations & cache preload | 8.4 MiB | **12.0 MiB** (`12,288 kB`) | **12.0 MiB** (`12,288 kB`) |
| **Peak RSS (`VmHWM`)** under sustained saturation (`INGEST_CHANNEL_CAPACITY=8192`) | 100.6 MiB | **115.3 MiB** (`118,088 kB`) | **106.0 MiB** (`108,552 kB`) |
| OS Threads (idle / peak) | 17 / 17 | 17 / 17 | 18 / 19 |

> Idle RSS remains **12.0 MiB** (well within Design §9's target of tens of MB), and enabling `INGEST_SPOOL_DIR` adds zero idle RSS overhead and only 1–2 background threads for 1s `fdatasync` / segment rotation.

## Benchmark Results

### Run 1 — Without Spool (`INGEST_SPOOL_DIR` unset)

| Scenario | Kind / Mode | Requests | Elapsed | HTTP req/s | 2xx | 429 | 5xx / Err | Latency p50 / p95 / p99 / max (ms) | Persisted (Err / Tx) | Lost | Drain Time | Persisted/s | RSS After |
|---|---|---:|---:|---:|---:|---:|---|---:|---|---:|---:|---:|---:|
| **burst-error** | `error`, `20k` total | 20,000 | 0.546s | **36,630.3** | 8,960 | 11,040 | `0 / 0` | `0.62 / 1.29 / 1.70 / 8.11` | `8,960 / 0` | **0** | 2.411s | **3,717.0** | 66.0 MiB |
| **burst-transaction** | `transaction`, `20k` total | 20,000 | 0.586s | **34,157.3** | 9,684 | 10,316 | `0 / 0` | `0.64 / 1.35 / 1.76 / 8.57` | `0 / 9,684` | **0** | 3.111s | **3,112.8** | 82.9 MiB |
| **sustained-error** | `error`, `10s` | 364,043 | 10.004s | **36,388.5** | 42,880 | 321,163 | `0 / 0` | `0.63 / 1.35 / 1.75 / 6.71` | `42,880 / 0` | **0** | 11.672s | **3,673.9** | 108.6 MiB |
| **sustained-transaction** | `transaction`, `10s` | 333,068 | 10.004s | **33,294.7** | 40,315 | 292,753 | `0 / 0` | `0.67 / 1.43 / 1.85 / 10.58` | `0 / 40,315` | **0** | 12.520s | **3,220.0** | 115.3 MiB |
| **sustained-mixed** | `mixed` (50/50), `10s` | 322,848 | 10.005s | **32,269.9** | 28,505 | 294,343 | `0 / 0` | `0.69 / 1.48 / 1.92 / 5.76` | `14,250 / 14,255` | **0** | 12.830s | **2,221.8** | 115.3 MiB |

### Run 2 — With Write-Ahead Spool (`INGEST_SPOOL_DIR` enabled)

| Scenario | Kind / Mode | Requests | Elapsed | HTTP req/s | 2xx | 429 | 5xx / Err | Latency p50 / p95 / p99 / max (ms) | Persisted (Err / Tx) | Lost | Drain Time | Persisted/s | Delta vs No Spool | RSS After |
|---|---|---:|---:|---:|---:|---:|---|---:|---|---:|---:|---:|---:|---:|
| **sustained-error** | `error`, `10s` | 363,668 | 10.005s | **36,350.3** | 41,759 | 321,909 | `0 / 0` | `0.64 / 1.37 / 1.78 / 11.87` | `41,759 / 0` | **0** | 11.742s | **3,556.3** | `-3.2%` | 95.8 MiB |
| **sustained-transaction** | `transaction`, `10s` | 318,738 | 10.004s | **31,861.9** | 40,165 | 278,573 | `0 / 0` | `0.70 / 1.50 / 1.93 / 33.68` | `0 / 40,165` | **0** | 12.543s | **3,202.3** | `-0.5%` | 105.5 MiB |
| **sustained-mixed** | `mixed` (50/50), `10s` | 329,496 | 10.004s | **32,934.8** | 28,332 | 301,164 | `0 / 0` | `0.69 / 1.48 / 1.91 / 18.32` | `14,122 / 14,210` | **0** | 13.056s | **2,170.0** | `-2.3%` | 106.0 MiB |

### Performance Issue Detection & Trace Sampling Observed

Verified directly in PostgreSQL for the `bench-phase2` projects created during the runs above:

| Scenario | Accepted Tx | N+1 DB Issues Detected (`F017`) | N+1 Occurrences | Sampled zstd Traces | `'issue'` Traces | `'error'` Traces |
|---|---:|---:|---:|---:|---:|---:|
| `burst-transaction` (no spool) | 9,684 | 5 | 1,907 | 2,324 | 1,634 | 186 |
| `sustained-transaction` (no spool) | 40,315 | 5 | 7,872 | 9,480 | 6,742 | 787 |
| `sustained-mixed` (no spool) | 14,255 | 5 | 2,737 | 3,346 | 2,575 | 0 |
| `sustained-transaction` (with spool) | 40,165 | 5 | 7,828 | 9,449 | 6,763 | 740 |
| `sustained-mixed` (with spool) | 14,210 | 5 | 2,785 | 3,363 | 2,652 | 0 |

*(In `mixed` mode, transactions carry odd `seq` values while the synthetic `internal_error` status triggers on `seq % 50 == 0`, so `'error'` traces are `0` for `mixed` and `~2%` for `transaction`.)*

## Key Observations

1. **Zero lost accepted envelopes across all 8 scenarios:** `240,600` envelopes accepted (`2xx`), `240,600` persisted and verified via the API (`0` lost, `0` `5xx`, `0` transport errors), both with and without `INGEST_SPOOL_DIR`.
2. **Spool overhead is negligible (~0.5%–3.2%):** Because `INGEST_SPOOL_DIR` appends CRC-framed records sequentially and batches `fdatasync` on a 1s interval (D012), sustained persistence drops by only `0.5%–3.2%` and p99 HTTP latency increases by `≤ 0.09 ms` (max latency rises slightly to `11.9–33.7 ms` during segment sync/rotation).
3. **`429` backpressure works identically with and without spool:** Across all 8 runs, `1,831,261` over-capacity requests returned clean `429 Too Many Requests` (`100%` carrying both `Retry-After` and `X-Sentry-Rate-Limits: 5:error:organization` or `5:transaction:organization`). When the bounded channel is full, `429` is returned before writing to the spool, keeping disk I/O and RSS bounded.
4. **Error vs Transaction vs Mixed persistence ceiling:**
   - **Errors (`~3,670/s`):** Slight improvement over Phase 1's ~3,000/s steady-state even with configurable PII rules, fingerprint aliases, and HMAC `issue_users` counting enabled.
   - **Transactions (`~3,220/s`):** Sustained >3,200 tx/s while running span normalization, 76-bucket histogram merges, `span_minute_metrics` upserts, `F017` N+1 DB detection (firing on 20% of transactions and emitting performance issues), and zstd trace compression (~23% sampled due to 20% forced `'issue'` + 2% `'error'` + random sampling).
   - **Mixed (`~2,220/s`):** Lower than either homogeneous workload because every worker batch flushes both the error pipeline (`issues` / `events` / `issue_hourly_counters` / `issue_users`) and the performance pipeline (`transaction_minute_metrics` / `span_minute_metrics` / `traces` + N+1 performance issues) in separate DB passes over half-sized sub-batches.
5. **Headroom vs target workload:** Even the slowest mode (`mixed` with spool at `2,170 items/s`) sustains **~187 million items/day** — over 1,800× the initial Design §9 target of `< 100k events/day` on a single binary and a single PostgreSQL 16 instance.

## Reproducing the Benchmarks

```bash
# 1. Start PostgreSQL 16 and build release binaries
docker compose up -d postgres
npm --prefix web run build
cargo build --release -p maxi-loop -p maxi-loop-loadgen

# 2. Run the automated Phase 2 benchmark harness (runs both no-spool and spool passes)
set -a && . ./.env && set +a
unset BOOTSTRAP_ADMIN_EMAIL BOOTSTRAP_ADMIN_PASSWORD
tests/e2e/python/.venv/bin/python tests/e2e/python/bench_phase2.py --out /tmp/bench-phase2.json

# 3. Or invoke maxi-loop-loadgen directly against any project DSN
SENTRY_DSN="http://<key>@127.0.0.1:8080/<project_id>" \
  ./target/release/maxi-loop-loadgen --kind error --duration-secs 10 --concurrency 32 --unique-issues 25 --compression gzip

SENTRY_DSN="http://<key>@127.0.0.1:8080/<project_id>" \
  ./target/release/maxi-loop-loadgen --kind transaction --duration-secs 10 --concurrency 32 --unique-issues 25 --compression gzip

SENTRY_DSN="http://<key>@127.0.0.1:8080/<project_id>" \
  ./target/release/maxi-loop-loadgen --kind mixed --duration-secs 10 --concurrency 32 --unique-issues 25 --compression gzip
```
