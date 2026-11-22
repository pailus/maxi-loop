#!/usr/bin/env python3
"""End-to-end verification of Performance alerts (F019) with the official Python sentry-sdk.

Real transactions are sent by changing ONLY the DSN; `perf_p95` and `perf_error_rate` rules are
created through the public API and the running server's alert engine (60s tick) must evaluate
them from `transaction_minute_metrics` and record deliveries in the audit log.

Delivery goes to an email channel. When the server has no SMTP configured the attempt is
recorded as `failed` ("SMTP not configured") - that still proves evaluation + audit logging
without sending anything off-host. A rule that should NOT fire is checked as well.
"""

from __future__ import annotations

import os
import secrets
import subprocess
import sys
import time
import urllib.parse
from typing import Any

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from verify_sentry_sdk import MaxiLoopApiClient  # noqa: E402

SLOW_TX = "POST /e2e/checkout"
FAST_TX = "GET /e2e/health"


def run_worker() -> None:
    import sentry_sdk

    sentry_sdk.init(
        dsn=os.environ["SENTRY_DSN"], environment="e2e-perf-alerts", traces_sample_rate=1.0
    )
    # 8 slow checkouts (~250ms), 4 of them failing -> 50% error rate on SLOW_TX.
    for i in range(8):
        with sentry_sdk.start_transaction(op="http.server", name=SLOW_TX) as tx:
            time.sleep(0.25)
            if i % 2 == 0:
                tx.set_status("internal_error")
    # 12 fast health checks.
    for _ in range(12):
        with sentry_sdk.start_transaction(op="http.server", name=FAST_TX):
            time.sleep(0.005)
    sentry_sdk.flush(timeout=10)


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "--worker":
        run_worker()
        return

    email = os.environ.get("MAXI_LOOP_ADMIN_EMAIL")
    password = os.environ.get("MAXI_LOOP_ADMIN_PASSWORD")
    if not (email and password):
        raise SystemExit("set MAXI_LOOP_ADMIN_EMAIL and MAXI_LOOP_ADMIN_PASSWORD")
    api = MaxiLoopApiClient(os.environ.get("MAXI_LOOP_BASE_URL", "http://127.0.0.1:8080"))
    api.login(email, password)
    slug = "e2e-palert-" + secrets.token_hex(3)
    project = api.create_project(slug, f"E2E perf alerts {slug}")

    channel = api.request_json(
        "POST",
        "/api/v1/alerts/channels",
        {
            "name": f"{slug} email",
            "channel_type": "email",
            "config": {"to": ["oncall@example.com"]},
            "project_slug": slug,
        },
    )

    def rule(name: str, body: dict[str, Any]) -> dict[str, Any]:
        payload = {"name": name, "channel_id": channel["id"], "project_slug": slug, **body}
        return api.request_json("POST", "/api/v1/alerts/rules", payload)

    p95 = rule("p95 > 100ms", {"trigger_type": "perf_p95", "perf_threshold": 100,
                               "perf_window_minutes": 10, "perf_min_count": 5})
    err = rule("checkout errors > 20%", {"trigger_type": "perf_error_rate", "perf_threshold": 20,
                                         "transaction_name": SLOW_TX, "perf_window_minutes": 10,
                                         "perf_min_count": 5})
    quiet = rule("health p95 > 100ms", {"trigger_type": "perf_p95", "perf_threshold": 100,
                                        "transaction_name": FAST_TX, "perf_window_minutes": 10,
                                        "perf_min_count": 5})

    proc = subprocess.run(
        [sys.executable, os.path.abspath(__file__), "--worker"],
        env=dict(os.environ, SENTRY_DSN=project["dsn"]),
        capture_output=True, text=True, timeout=120, check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"worker failed:\n{proc.stdout}\n{proc.stderr[-3000:]}")
    print("[ok] 20 transactions sent with the official sentry-sdk")

    try:
        verify(api, slug, p95, err, quiet)
    finally:
        # Do not leave rules firing every minute on the shared dev server.
        for r in (p95, err, quiet):
            api.request_json("DELETE", f"/api/v1/alerts/rules/{r['id']}")


def verify(
    api: MaxiLoopApiClient,
    slug: str,
    p95: dict[str, Any],
    err: dict[str, Any],
    quiet: dict[str, Any],
) -> None:
    wanted = {p95["id"], err["id"]}
    deadline = time.time() + 150  # alert engine ticks every 60s
    ours: list[dict[str, Any]] = []
    while time.time() < deadline:
        deliveries = api.request_json("GET", "/api/v1/alerts/deliveries?limit=200")
        items = deliveries if isinstance(deliveries, list) else deliveries.get("items", [])
        ours = [d for d in items if d.get("project_slug") == slug]
        if wanted <= {d["rule_id"] for d in ours}:
            break
        time.sleep(3)
    by_rule = {d["rule_id"]: d for d in ours}
    assert wanted <= set(by_rule), f"expected deliveries for {wanted}, got {ours}"

    d_p95, d_err = by_rule[p95["id"]], by_rule[err["id"]]
    for d in (d_p95, d_err):
        assert d["issue_id"] is None, d
        assert d["status"] in ("sent", "failed"), d
    assert d_p95["trigger_type"] == "perf_p95" and d_p95["metric_value"] > 100, d_p95
    assert d_err["trigger_type"] == "perf_error_rate" and d_err["metric_value"] == 50.0, d_err
    assert d_err["transaction_name"] == SLOW_TX, d_err
    assert quiet["id"] not in by_rule, f"fast transaction must not alert: {by_rule[quiet['id']]}"
    print(f"[ok] p95 alert: {d_p95['issue_title']} ({d_p95['status']})")
    print(f"[ok] error-rate alert: {d_err['issue_title']} ({d_err['status']})")
    print("[ok] fast transaction rule stayed quiet")
    print(f"F019 performance alerts E2E: PASS (project {urllib.parse.quote(slug)})")


if __name__ == "__main__":
    main()
