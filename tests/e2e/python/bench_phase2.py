#!/usr/bin/env python3
"""Phase 2 benchmark harness (F021) for `docs/benchmarks-phase2.md`.

Bootstraps a dedicated `bench-phase2` tenant (random admin credentials, never printed), starts
its own release `maxi-loop serve` on a separate port (optionally with `INGEST_SPOOL_DIR`), runs
`maxi-loop-loadgen` scenarios (error / transaction / mixed; burst and sustained), and measures
HTTP throughput + latency, the end-to-end persistence rate (time until every accepted item is
visible through the API), idle and peak RSS. Writes a JSON summary to `--out`.

Needs DATABASE_URL etc. in the environment or `.env` (same as the server). Usage:
    bench_phase2.py [--out bench.json] [--port 18082] [--duration 10] [--burst 20000]
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.parse
from pathlib import Path
from typing import Any

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from verify_ingest_spool import wait_healthy  # noqa: E402
from verify_sentry_sdk import MaxiLoopApiClient  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[3]
SERVER_BIN = REPO_ROOT / "target/release/maxi-loop"
LOADGEN_BIN = REPO_ROOT / "target/release/maxi-loop-loadgen"
TENANT_SLUG = "bench-phase2"
CONCURRENCY = 32
TEMPLATES = 25


def proc_status(pid: int) -> dict[str, int]:
    out: dict[str, int] = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "VmHWM", "Threads"):
            out[key] = int(value.split()[0])
    return out


class Server:
    def __init__(self, port: int, spool_dir: Path | None, log_path: Path) -> None:
        env = dict(
            os.environ,
            APP_HOST="127.0.0.1",
            APP_PORT=str(port),
            INGEST_CHANNEL_CAPACITY="8192",
            DB_MAX_CONNECTIONS="10",
            RUST_LOG="warn",
            SINGLE_TENANT_MODE="false",
        )
        env.pop("INGEST_SPOOL_DIR", None)
        if spool_dir:
            env["INGEST_SPOOL_DIR"] = str(spool_dir)
        self.log = open(log_path, "ab")  # noqa: SIM115 - closed in stop()
        self.proc = subprocess.Popen(
            [str(SERVER_BIN), "serve"], cwd=REPO_ROOT, env=env, stdout=self.log, stderr=subprocess.STDOUT
        )
        self.base_url = f"http://127.0.0.1:{port}"
        wait_healthy(self.base_url, self.proc)

    def stop(self) -> dict[str, int]:
        status = proc_status(self.proc.pid)
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()
        return status


def bootstrap(email: str, password: str) -> None:
    env = dict(os.environ, BOOTSTRAP_ADMIN_EMAIL=email, BOOTSTRAP_ADMIN_PASSWORD=password)
    subprocess.run(
        [str(SERVER_BIN), "bootstrap", "--tenant-slug", TENANT_SLUG, "--tenant-name", "Bench Phase 2"],
        cwd=REPO_ROOT,
        env=env,
        check=True,
        stdout=subprocess.DEVNULL,
    )


def login(base_url: str, email: str, password: str) -> MaxiLoopApiClient:
    api = MaxiLoopApiClient(base_url)
    resp = api.request_json(
        "POST", "/api/v1/auth/login", {"email": email, "password": password, "tenant_slug": TENANT_SLUG}
    )
    api.csrf_token = str(resp["csrf_token"])
    return api


def persisted(api: MaxiLoopApiClient, slug: str) -> tuple[int, int]:
    issues = api.request_json("GET", f"/api/v1/projects/{urllib.parse.quote(slug)}/issues?per_page=100")
    errors = sum(int(i["event_count"]) for i in issues.get("items", []) if i.get("issue_type", "error") == "error")
    txs = api.request_json(
        "GET", f"/api/v1/performance/transactions?project_slug={urllib.parse.quote(slug)}&hours=1&per_page=100"
    )
    transactions = sum(int(t["total_count"]) for t in txs.get("items", []))
    return errors, transactions


def run_scenario(api: MaxiLoopApiClient, port: int, name: str, kind: str, mode: list[str]) -> dict[str, Any]:
    slug = f"bench-{name}-{secrets.token_hex(2)}"
    project = api.request_json(
        "POST",
        "/api/v1/projects",
        {"slug": slug, "name": slug, "platform": "python", "rate_limit_per_minute": 1_000_000},
    )
    dsn = f"http://{project['dsn_public_key']}@127.0.0.1:{port}/{project['id']}"
    started = time.monotonic()
    result = subprocess.run(
        [
            str(LOADGEN_BIN),
            *mode,
            "--kind",
            kind,
            "--concurrency",
            str(CONCURRENCY),
            "--unique-issues",
            str(TEMPLATES),
            "--compression",
            "gzip",
            "--json",
        ],
        env=dict(os.environ, SENTRY_DSN=dsn),
        check=True,
        capture_output=True,
        text=True,
    )
    report = json.loads(result.stdout)
    accepted = int(report["status_2xx"])

    last, last_change = (-1, -1), time.monotonic()
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        now = persisted(api, slug)
        if now != last:
            last, last_change = now, time.monotonic()
        elif sum(now) >= accepted and time.monotonic() - last_change > 3:
            break
        time.sleep(0.25)
    drain_secs = last_change - started
    stored = sum(last)
    return {
        "scenario": name,
        "kind": kind,
        "loadgen": report,
        "accepted": accepted,
        "stored_errors": last[0],
        "stored_transactions": last[1],
        "lost": max(0, accepted - stored),
        "persist_secs": round(drain_secs, 3),
        "persisted_per_sec": round(stored / drain_secs, 1) if drain_secs > 0 else None,
    }


def bench_server(port: int, spool_dir: Path | None, log: Path, email: str, password: str,
                 scenarios: list[tuple[str, str, list[str]]]) -> dict[str, Any]:
    server = Server(port, spool_dir, log)
    try:
        time.sleep(3)
        idle = proc_status(server.proc.pid)
        api = login(server.base_url, email, password)
        api.request_json("PATCH", "/api/v1/tenants/current/settings", {"aggregate_rate_limit_per_minute": 10_000_000})
        results = []
        for name, kind, mode in scenarios:
            res = run_scenario(api, port, name, kind, mode)
            res["rss_after_kb"] = proc_status(server.proc.pid)["VmRSS"]
            print(json.dumps({k: v for k, v in res.items() if k != "loadgen"}), flush=True)
            results.append(res)
    finally:
        final = server.stop()
    return {"spool": spool_dir is not None, "idle": idle, "final": final, "scenarios": results}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="bench-phase2.json")
    parser.add_argument("--port", type=int, default=18082)
    parser.add_argument("--duration", type=int, default=10)
    parser.add_argument("--burst", type=int, default=20000)
    args = parser.parse_args()

    email = f"bench-{secrets.token_hex(4)}@bench.local"
    password = secrets.token_urlsafe(24)
    bootstrap(email, password)

    dur = ["--duration-secs", str(args.duration)]
    burst = ["--total", str(args.burst)]
    full = [
        ("burst-error", "error", burst),
        ("burst-transaction", "transaction", burst),
        ("sustained-error", "error", dur),
        ("sustained-transaction", "transaction", dur),
        ("sustained-mixed", "mixed", dur),
    ]
    sustained = [s for s in full if s[0].startswith("sustained")]

    work = Path(tempfile.mkdtemp(prefix="maxi-loop-bench-"))
    try:
        summary = {
            "binary_bytes": SERVER_BIN.stat().st_size,
            "loadgen_bytes": LOADGEN_BIN.stat().st_size,
            "runs": [
                bench_server(args.port, None, work / "server.log", email, password, full),
                bench_server(args.port, work / "spool", work / "server.log", email, password, sustained),
            ],
        }
    finally:
        shutil.rmtree(work, ignore_errors=True)
    Path(args.out).write_text(json.dumps(summary, indent=2))
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
