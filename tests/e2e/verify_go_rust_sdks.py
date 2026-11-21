#!/usr/bin/env python3
"""End-to-end verification of Maxi Loop against official Go (`sentry-go`) and Rust (`sentry` + `tracing`)
SDKs for BOTH error tracking (grouping, stack frames, PII scrubbing) and performance monitoring
(transactions, histograms, sampled zstd trace waterfalls) — changing ONLY the Sentry DSN.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import json
import os
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

HTTP_TIMEOUT_SECS = 15
ROOT_DIR = Path(__file__).resolve().parents[2]


class MaxiLoopApiClient:
    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))
        self.csrf_token = ""

    def request_json(
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
        bearer_token: str | None = None,
    ) -> tuple[int, Any]:
        url = f"{self.base_url}{path}"
        headers: dict[str, str] = {"Accept": "application/json"}
        data: bytes | None = None
        if payload is not None:
            data = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        if bearer_token:
            headers["Authorization"] = f"Bearer {bearer_token}"
        elif method not in ("GET", "HEAD", "OPTIONS") and self.csrf_token:
            headers["X-CSRF-Token"] = self.csrf_token

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with self.opener.open(req, timeout=HTTP_TIMEOUT_SECS) as resp:
                raw = resp.read().decode("utf-8")
                return resp.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as err:
            raw = err.read().decode("utf-8", errors="replace")
            try:
                body = json.loads(raw) if raw else None
            except json.JSONDecodeError:
                body = raw
            return err.code, body

    def login(self, email: str, password: str, tenant_slug: str | None = None) -> dict[str, Any]:
        body: dict[str, Any] = {"email": email, "password": password}
        if tenant_slug:
            body["tenant_slug"] = tenant_slug
        status, data = self.request_json("POST", "/api/v1/auth/login", body)
        if status != 200:
            raise RuntimeError(f"Login failed ({status}): {data}")
        self.csrf_token = data["csrf_token"]
        return data

    def create_project(self, slug: str, name: str, platform: str) -> dict[str, Any]:
        status, data = self.request_json(
            "POST",
            "/api/v1/projects",
            {"slug": slug, "name": name, "platform": platform},
        )
        if status != 201:
            raise RuntimeError(f"Create project {slug} failed ({status}): {data}")
        return data

    def create_api_token(self, project_slug: str, name: str, can_write: bool = False) -> dict[str, Any]:
        status, data = self.request_json(
            "POST",
            "/api/v1/api-tokens",
            {"project_slug": project_slug, "name": name, "can_write": can_write},
        )
        if status != 201:
            raise RuntimeError(f"Create api token failed ({status}): {data}")
        return data


def poll_until(desc: str, fn: Any, timeout_secs: float = 10.0) -> Any:
    deadline = time.monotonic() + timeout_secs
    last_val = None
    while time.monotonic() < deadline:
        last_val = fn()
        if last_val:
            return last_val
        time.sleep(0.25)
    raise AssertionError(f"Timed out waiting for {desc}; last value={last_val}")


def parse_trace_id(stdout: str) -> str:
    for line in stdout.splitlines():
        if line.startswith("TRACE_ID="):
            tid = line.split("=", 1)[1].strip()
            if tid:
                return tid
    raise RuntimeError(f"Worker did not print TRACE_ID=; stdout={stdout!r}")


def verify_sdk_results(
    api: MaxiLoopApiClient,
    sdk_label: str,
    project_slug: str,
    api_token: str,
    expected_tx_name: str,
    trace_id: str,
    bearer_secret: str,
    cookie_secret: str,
) -> None:
    # 1. Verify grouped issues via project-scoped API token (`Authorization: Bearer mlapi_...`)
    def check_issues() -> list[dict[str, Any]] | None:
        st, body = api.request_json(
            "GET",
            f"/api/v1/issues?project_slug={urllib.parse.quote(project_slug)}",
            bearer_token=api_token,
        )
        if st != 200:
            return None
        items = body.get("items", [])
        total_events = sum(i.get("event_count", 0) for i in items)
        if len(items) >= 2 and total_events >= 4:
            return items
        return None

    issues = poll_until(f"{sdk_label} grouped issues", check_issues)
    event_counts = sorted((i["event_count"] for i in issues), reverse=True)
    assert len(issues) == 2, f"{sdk_label}: expected 2 grouped issues, got {len(issues)}: {[i['title'] for i in issues]}"
    assert event_counts == [3, 1], f"{sdk_label}: expected event counts [3, 1], got {event_counts}"

    # 2. Verify stack frames & PII scrubbing on issue detail
    for issue in issues:
        st, detail = api.request_json(
            "GET",
            f"/api/v1/issues/{issue['id']}",
            bearer_token=api_token,
        )
        assert st == 200, f"{sdk_label} issue detail failed: {st} {detail}"
        frames = detail.get("stack_frames") or []
        assert len(frames) > 0, f"{sdk_label}: expected non-empty stack_frames for issue {issue['title']}"
        raw_detail_str = json.dumps(detail)
        assert bearer_secret not in raw_detail_str, f"{sdk_label}: bearer PII leaked!"
        assert cookie_secret not in raw_detail_str, f"{sdk_label}: cookie PII leaked!"
        assert "[Filtered]" in raw_detail_str, f"{sdk_label}: expected [Filtered] marker in scrubbed payload"

    # 3. Verify performance transaction aggregation
    def check_transactions() -> list[dict[str, Any]] | None:
        st, body = api.request_json(
            "GET",
            f"/api/v1/performance/transactions?project_slug={urllib.parse.quote(project_slug)}",
            bearer_token=api_token,
        )
        if st != 200:
            return None
        items = body.get("items", [])
        if any(t.get("transaction_name") == expected_tx_name for t in items):
            return items
        return None

    txs = poll_until(f"{sdk_label} performance transaction", check_transactions)
    matched_tx = next(t for t in txs if t["transaction_name"] == expected_tx_name)
    assert matched_tx["total_count"] >= 1
    assert matched_tx["p50_ms"] > 0.0

    # 4. Verify sampled trace waterfall & child spans
    def check_trace() -> dict[str, Any] | None:
        st, body = api.request_json(
            "GET",
            f"/api/v1/performance/traces/{urllib.parse.quote(trace_id)}",
            bearer_token=api_token,
        )
        if st == 200 and body and len(body.get("spans", [])) >= 3:
            return body
        return None

    wf = poll_until(f"{sdk_label} trace waterfall {trace_id}", check_trace)
    spans = wf["spans"]
    ops = {s["op"] for s in spans}
    assert "http.server" in ops, f"{sdk_label}: missing root http.server span: {ops}"
    assert any(op.startswith("db.sql") for op in ops), f"{sdk_label}: missing db.sql child span: {ops}"
    assert "http.client" in ops, f"{sdk_label}: missing http.client child span: {ops}"

    print(
        f"[PASS] {sdk_label}: {len(issues)} grouped issues ({event_counts} events), "
        f"stack frames & [Filtered] PII verified, tx '{expected_tx_name}' p50={matched_tx['p50_ms']:.1f}ms, "
        f"trace waterfall {trace_id} ({len(spans)} spans)"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify official Go & Rust Sentry SDKs against Maxi Loop")
    parser.add_argument("--base-url", default="http://127.0.0.1:8080")
    parser.add_argument("--email", required=True)
    parser.add_argument("--password", required=True)
    parser.add_argument("--tenant-slug", default=None)
    args = parser.parse_args()

    api = MaxiLoopApiClient(args.base_url)
    api.login(args.email, args.password, args.tenant_slug)

    run_id = secrets.token_hex(4)
    bearer_secret = "e2e-bearer-" + secrets.token_hex(12)
    cookie_secret = "e2e-cookie-" + secrets.token_hex(12)

    go_proj = api.create_project(f"e2e-go-{run_id}", f"E2E Go {run_id}", "go")
    rust_proj = api.create_project(f"e2e-rust-{run_id}", f"E2E Rust {run_id}", "rust")
    neg_proj = api.create_project(f"e2e-neg-{run_id}", f"E2E Negative {run_id}", "go")

    go_tok = api.create_api_token(go_proj["slug"], f"e2e-go-ro-{run_id}", can_write=False)["raw_token"]
    rust_tok = api.create_api_token(rust_proj["slug"], f"e2e-rust-ro-{run_id}", can_write=False)["raw_token"]

    common_env = os.environ.copy()
    common_env["E2E_BEARER_TOKEN"] = bearer_secret
    common_env["E2E_COOKIE_VALUE"] = cookie_secret

    # --- Go SDK (github.com/getsentry/sentry-go) ---
    go_dir = ROOT_DIR / "tests" / "e2e" / "go"
    go_env = common_env.copy()
    go_env["SENTRY_DSN"] = go_proj["dsn"]
    go_res = subprocess.run(
        ["go", "run", ".", "--mode=error-and-trace"],
        cwd=str(go_dir),
        env=go_env,
        capture_output=True,
        text=True,
        check=True,
        timeout=120,
    )
    go_trace_id = parse_trace_id(go_res.stdout)
    verify_sdk_results(
        api,
        "Go (sentry-go)",
        go_proj["slug"],
        go_tok,
        "GET /api/go/orders/{id}",
        go_trace_id,
        bearer_secret,
        cookie_secret,
    )

    # --- Rust SDK (sentry + sentry-tracing) ---
    rust_dir = ROOT_DIR / "tests" / "e2e" / "rust"
    rust_env = common_env.copy()
    rust_env["SENTRY_DSN"] = rust_proj["dsn"]
    rust_res = subprocess.run(
        ["cargo", "run", "--quiet", "--", "error-and-trace"],
        cwd=str(rust_dir),
        env=rust_env,
        capture_output=True,
        text=True,
        timeout=180,
    )
    if rust_res.returncode != 0:
        raise RuntimeError(
            f"Rust SDK worker failed ({rust_res.returncode}):\nSTDOUT:\n{rust_res.stdout}\nSTDERR:\n{rust_res.stderr}"
        )
    rust_trace_id = parse_trace_id(rust_res.stdout)
    verify_sdk_results(
        api,
        "Rust (sentry + tracing)",
        rust_proj["slug"],
        rust_tok,
        "POST /api/rust/billing",
        rust_trace_id,
        bearer_secret,
        cookie_secret,
    )

    # --- Negative tampered DSN test (Go & Rust) ---
    tampered_dsn = neg_proj["dsn"].replace(neg_proj["dsn_public_key"], "0" * len(neg_proj["dsn_public_key"]))
    neg_env = common_env.copy()
    neg_env["SENTRY_DSN"] = tampered_dsn
    subprocess.run(
        ["go", "run", ".", "--mode=invalid-dsn"],
        cwd=str(go_dir),
        env=neg_env,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    subprocess.run(
        ["cargo", "run", "--quiet", "--", "invalid-dsn"],
        cwd=str(rust_dir),
        env=neg_env,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    time.sleep(0.6)
    st, neg_issues = api.request_json("GET", f"/api/v1/issues?project_slug={neg_proj['slug']}")
    assert st == 200 and len(neg_issues.get("items", [])) == 0, f"Tampered DSN ingested events: {neg_issues}"
    print("[PASS] Negative tampered-DSN test (Go & Rust): 0 events ingested")

    return 0


if __name__ == "__main__":
    sys.exit(main())
