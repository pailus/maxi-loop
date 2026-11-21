# Maxi Loop — Phase 1 (MVP) Performance & SDK Compatibility Baseline

Recorded for **F009** (Design §9 & §11) to establish reproducible Phase 1 baseline measurements for binary size, idle & load RSS memory footprint, HTTP ingest throughput, sustained PostgreSQL batch persistence, `429` rate-limiting / backpressure behavior, and official Python `sentry-sdk` compatibility.

## Test Environment

| Component | Details |
|---|---|
| Host OS / Arch | Linux 6.8.0-60-generic `x86_64` |
| CPU | AMD Ryzen 7 1700 Eight-Core Processor (16 logical threads) |
| Host Memory | 31 GiB RAM |
| Rust Toolchain | `rustc 1.99.1` (`--release` profile) |
| Database | PostgreSQL `16.13` (`postgres:16-alpine` via Docker Compose on `127.0.0.1:55432`) |
| Server Config | `INGEST_CHANNEL_CAPACITY=8192`, `MAX_FULL_EVENTS_PER_ISSUE=50`, `DB_MAX_CONNECTIONS=10`, `RUST_LOG=warn` |

## Binary Footprint & Memory (RSS)

| Metric | Measured Value |
|---|---|
| `target/release/maxi-loop` (server + embedded React SPA) | **13 MB** |
| `target/release/maxi-loop-loadgen` (load generator) | **3.0 MB** |
| **Idle RSS (`VmRSS`)** after startup, migrations & DSN cache preload | **8.4 MiB (`8,640 kB`)** |
| Startup peak (`VmHWM`, during migrations + Argon2id bootstrap) | **25.4 MiB (`25,984 kB`)** |
| Peak RSS (`VmHWM`) under 143k req / 5s saturation (`INGEST_CHANNEL_CAPACITY=8192`) | **100.6 MiB (`102,976 kB`)** |
| OS Threads | 17 |

> Design §9 target: *"Aplikasi Rust: puluhan MB RSS saat idle"*. Measured idle RSS is **~8.4–8.6 MiB**.

## Synthetic Envelope Load-Test Results (`maxi-loop-loadgen`)

Measured against a live release-mode `maxi-loop serve` + PostgreSQL 16 using HTTP/1.1 keep-alive workers:

| Scenario | Payload / Mode | Concurrency | Requests | Elapsed | Total req/s | 2xx req/s | 2xx | 429 | 5xx / Err | Latency p50 / p95 / p99 (ms) | `X-Sentry-Rate-Limits` |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| **Burst Gzip** | 25 issues, `gzip` | 32 | 5,000 | 0.181s | **27,597.7** | **27,597.7** | 5,000 | 0 | 0 / 0 | `0.83 / 1.72 / 2.37` | — |
| **Burst Uncompressed** | 25 issues, `none` | 32 | 5,000 | 0.079s | **63,564.8** | **63,564.8** | 5,000 | 0 | 0 / 0 | `0.41 / 0.86 / 1.13` | — |
| **Gzip + Early-Drop** | 25 issues + `session` item | 16 | 5,000 | 0.196s | **25,558.6** | **25,558.6** | 5,000 | 0 | 0 / 0 | `0.43 / 0.91 / 1.29` | — |
| **Per-Project Quota 429** | `120/min` project quota, `gzip` | 16 | 500 | 0.019s | **26,364.9** | 6,327.6 | **120** | **380** | 0 / 0 | `0.32 / 0.73 / 1.86` | `1:error:project` (`380/380`) |
| **Sustained 5s Saturation** | 25 issues, `gzip`, 5s wall time | 32 | 143,555 | 5.002s | **28,696.7** | **4,605.7** | **23,040** | **120,515** | 0 / 0 | `0.78 / 1.81 / 2.46` | `5:error:organization` (`120515/120515`) |

### Key Observations

1. **Zero lost accepted events & full-event cap enforced:**
   - Burst project: all `15,000` accepted events persisted across `25` grouped issues (`max_stored_full_events = 50`).
   - Quota project: exact `120` accepted events persisted across `5` grouped issues (`380` rejected cleanly with `429`, `Retry-After`, and `X-Sentry-Rate-Limits: 1:error:project`).
   - Sustained project: all `23,040` accepted events persisted across `25` grouped issues (`max_stored_full_events = 50`).
2. **Bottleneck matches Design §9 prediction:**
   - The Rust Axum ingest hot path sustains **~27k–63k req/s** with sub-3ms p99 latency and zero DB queries per envelope.
   - Sustained end-to-end PostgreSQL batch persistence via the single background worker settles around **~3,000 events/sec** steady-state (~4,605/s over 5s including the initial 8,192 channel buffer), with clean `429` (`5:error:organization`) backpressure protecting memory and latency once the bounded channel fills.

## Official Python `sentry-sdk` E2E Verification (FastAPI & Django)

Verified via [`tests/e2e/python/verify_sentry_sdk.py`](../tests/e2e/python/verify_sentry_sdk.py) using **Python 3.13.4**, **`sentry-sdk==2.75.0`**, **`fastapi==0.165.0`**, and **`django==5.2.19`** against a live release-mode `maxi-loop` server by changing **only the DSN**:

- **FastAPI (`FastApiIntegration`)** → **PASS**: 4 unhandled HTTP exceptions (`ZeroDivisionError` × 3 with distinct invoice IDs + `ValueError` × 1) grouped into 2 issues (`event_count = 3` and `1`), `in_app` Python frames & `context_line` preserved, `hourly_series` matched, `Authorization` & `Cookie` headers scrubbed to `[Filtered]` (0 raw secrets persisted).
- **Django (`DjangoIntegration`)** → **PASS**: 3 unhandled HTTP exceptions (`RuntimeError` × 2 with distinct order IDs + `KeyError` × 1) grouped into 2 issues (`event_count = 2` and `1`), `in_app` Python frames & `context_line` preserved, `hourly_series` matched, `Authorization` & `Cookie` headers scrubbed to `[Filtered]` (0 raw secrets persisted).
- **Negative check (tampered DSN)** → **PASS**: sending an exception with an invalid DSN public key ingested 0 events.

## Reproducing the Benchmarks

```bash
# 1. Start PostgreSQL 16 and build release binaries
docker compose up -d postgres
npm --prefix web run build
cargo build --release -p maxi-loop -p maxi-loop-loadgen

# 2. Run maxi-loop server
APP_HOST=127.0.0.1 APP_PORT=8080 ./target/release/maxi-loop serve

# 3. Check idle RSS (replace <PID> with maxi-loop PID)
grep -E '^(VmRSS|VmHWM|Threads):' /proc/<PID>/status

# 4. Run synthetic envelope load tests (set SENTRY_DSN to a project DSN)
SENTRY_DSN="http://<key>@127.0.0.1:8080/<project_id>" \
  ./target/release/maxi-loop-loadgen --total 5000 --concurrency 32 --unique-issues 25 --compression gzip

# 5. Run sustained duration load test (exercises DB write ceiling & 429 backpressure)
SENTRY_DSN="http://<key>@127.0.0.1:8080/<project_id>" \
  ./target/release/maxi-loop-loadgen --duration-secs 5 --concurrency 32 --unique-issues 25 --compression gzip
```
