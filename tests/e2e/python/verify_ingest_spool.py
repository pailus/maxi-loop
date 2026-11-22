#!/usr/bin/env python3
"""End-to-end crash test for the optional ingest write-ahead spool (F020).

Starts its own `maxi-loop` release binary on a separate port with `INGEST_SPOOL_DIR` set,
fires a burst of envelopes from many threads, kills the server with SIGKILL in the middle
of the burst, restarts it on the same spool directory, and checks that every envelope that
was answered with `200` ends up stored (at-least-once: a few duplicates are tolerated,
losses are not).

Environment: MAXI_LOOP_ADMIN_EMAIL / MAXI_LOOP_ADMIN_PASSWORD (an existing admin), plus the
server's own settings (DATABASE_URL etc.) from the environment or `.env`. Optional:
MAXI_LOOP_BIN (default target/release/maxi-loop), SPOOL_E2E_PORT (default 18081).
"""

from __future__ import annotations

import json
import os
import re
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from verify_sentry_sdk import MaxiLoopApiClient  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[3]
TOTAL_ENVELOPES = 3000
THREADS = 32
KILL_AFTER_ACCEPTED = 1200
# At-least-once: a batch flushed right before SIGKILL but not yet checkpointed is replayed.
MAX_DUPLICATES = 512


def wait_healthy(base_url: str, proc: subprocess.Popen[bytes], timeout: float = 60.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise SystemExit(f"server exited early with code {proc.returncode}")
        try:
            with urllib.request.urlopen(f"{base_url}/healthz", timeout=2) as resp:
                if resp.status == 200:
                    return
        except (urllib.error.URLError, ConnectionError):
            pass
        time.sleep(0.2)
    raise SystemExit("server did not become healthy")


def start_server(binary: Path, port: int, spool_dir: Path, log_path: Path) -> subprocess.Popen[bytes]:
    env = dict(
        os.environ,
        APP_HOST="127.0.0.1",
        APP_PORT=str(port),
        INGEST_SPOOL_DIR=str(spool_dir),
        INGEST_CHANNEL_CAPACITY="4096",
        LOG_FORMAT="json",
        RUST_LOG="maxi_loop=info,maxi_loop_ingest=info",
    )
    log = open(log_path, "ab")  # noqa: SIM115 - closed with the process
    return subprocess.Popen(
        [str(binary), "serve"], cwd=REPO_ROOT, env=env, stdout=log, stderr=subprocess.STDOUT
    )


def envelope_body(event_id: str, error_type: str) -> bytes:
    header = json.dumps({"event_id": event_id})
    item = json.dumps(
        {
            "event_id": event_id,
            "level": "error",
            "platform": "python",
            "environment": "e2e-spool",
            "exception": {"values": [{"type": error_type, "value": "spool crash test"}]},
        }
    )
    return f'{header}\n{{"type":"event"}}\n{item}\n'.encode()


def fire_burst(url: str, key: str, error_type: str, proc: subprocess.Popen[bytes]) -> int:
    accepted = 0
    lock = threading.Lock()
    killed = threading.Event()
    counter = iter(range(TOTAL_ENVELOPES))

    def worker() -> None:
        nonlocal accepted
        while True:
            with lock:
                n = next(counter, None)
            if n is None or killed.is_set():
                return
            req = urllib.request.Request(
                url,
                data=envelope_body(secrets.token_hex(16), error_type),
                headers={
                    "Content-Type": "application/x-sentry-envelope",
                    "X-Sentry-Auth": f"Sentry sentry_version=7, sentry_key={key}",
                },
                method="POST",
            )
            try:
                with urllib.request.urlopen(req, timeout=5) as resp:
                    ok = resp.status == 200
            except (urllib.error.URLError, ConnectionError, TimeoutError):
                ok = False
            if not ok:
                continue
            with lock:
                accepted += 1
                if accepted == KILL_AFTER_ACCEPTED and not killed.is_set():
                    killed.set()
                    proc.send_signal(signal.SIGKILL)

    threads = [threading.Thread(target=worker) for _ in range(THREADS)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    if not killed.is_set():
        raise SystemExit("burst finished before the kill threshold was reached")
    proc.wait(timeout=10)
    return accepted


def stored_events(api: MaxiLoopApiClient, project_slug: str) -> int:
    return sum(int(i.get("event_count", 0)) for i in api.list_project_issues(project_slug))


def main() -> None:
    email = os.environ.get("MAXI_LOOP_ADMIN_EMAIL")
    password = os.environ.get("MAXI_LOOP_ADMIN_PASSWORD")
    if not (email and password):
        raise SystemExit("set MAXI_LOOP_ADMIN_EMAIL and MAXI_LOOP_ADMIN_PASSWORD")
    binary = Path(os.environ.get("MAXI_LOOP_BIN", REPO_ROOT / "target/release/maxi-loop"))
    port = int(os.environ.get("SPOOL_E2E_PORT", "18081"))
    base_url = f"http://127.0.0.1:{port}"

    work = Path(tempfile.mkdtemp(prefix="maxi-loop-spool-e2e-"))
    spool_dir = work / "spool"
    log_path = work / "server.log"
    proc = start_server(binary, port, spool_dir, log_path)
    try:
        wait_healthy(base_url, proc)
        api = MaxiLoopApiClient(base_url)
        api.login(email, password)
        slug = "e2e-spool-" + secrets.token_hex(3)
        project = api.create_project(slug, f"E2E spool {slug}")
        url = f"{base_url}/api/{project['id']}/envelope/"

        accepted = fire_burst(url, project["dsn_public_key"], "SpoolCrashE2E", proc)
        print(f"accepted before/at SIGKILL: {accepted}")

        proc = start_server(binary, port, spool_dir, log_path)
        wait_healthy(base_url, proc)
        api = MaxiLoopApiClient(base_url)
        api.login(email, password)

        stored, stable_since = -1, time.monotonic()
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            now = stored_events(api, slug)
            if now != stored:
                stored, stable_since = now, time.monotonic()
            elif now >= accepted and time.monotonic() - stable_since > 3:
                break
            time.sleep(0.5)

        replay_lines = [
            line
            for line in log_path.read_text(errors="replace").splitlines()
            if "replayed unflushed envelopes" in line
        ]
        replayed = re.search(r'"replayed":(\d+)', replay_lines[-1]) if replay_lines else None
        print(f"replayed after restart: {replayed.group(1) if replayed else 'unknown'}")
        print(f"stored events: {stored}")

        if stored < accepted:
            raise SystemExit(f"FAIL: lost {accepted - stored} accepted envelopes")
        if stored - accepted > MAX_DUPLICATES:
            raise SystemExit(f"FAIL: {stored - accepted} duplicates exceed {MAX_DUPLICATES}")
        print(f"PASS: no accepted envelope lost ({stored - accepted} at-least-once duplicates)")
    finally:
        if proc.poll() is None:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
