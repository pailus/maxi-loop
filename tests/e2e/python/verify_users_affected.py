#!/usr/bin/env python3
"""End-to-end verification of Users affected per issue (F018) with the official Python sentry-sdk.

The SDK is pointed at Maxi Loop by changing ONLY the DSN and sets `sentry_sdk.set_user(...)`
the way real apps do. Checks that Maxi Loop:

1. Counts distinct users per issue (`user_count`) across id / email identities, with email
   normalized case-insensitively and anonymous / IP-stripped events not counted.
2. Sorts by impact with `sort=users` (more users first, even with fewer events) while
   `sort=events` still prefers the noisier issue.
3. Never echoes the raw identity in the issues list (only a count is exposed).
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

SECRET_EMAIL = "Impact.Secret.User@Example.com"


class WideImpactError(Exception):
    pass


class NoisyError(Exception):
    pass


def run_worker() -> None:
    import sentry_sdk

    sentry_sdk.init(dsn=os.environ["SENTRY_DSN"], environment="e2e-users", traces_sample_rate=0)

    def capture(exc: Exception, user: dict[str, Any] | None) -> None:
        with sentry_sdk.new_scope() as scope:
            if user is not None:
                scope.set_user(user)
            try:
                raise exc
            except Exception as err:  # noqa: BLE001
                sentry_sdk.capture_exception(err)

    # 3 distinct users: id u-1, id 42, the (case-varied) secret email. Plus 2 that do not count:
    # an IP-only user (the SDK strips `ip_address` unless `send_default_pii=True`) and anonymous.
    for user in (
        {"id": "u-1"},
        {"id": "u-1", "username": "same-user"},
        {"id": 42},
        {"email": SECRET_EMAIL},
        {"email": SECRET_EMAIL.lower()},
        {"ip_address": "203.0.113.9"},
        None,
    ):
        capture(WideImpactError("checkout failed"), user)
    # 1 user, many events.
    for _ in range(12):
        capture(NoisyError("retry storm"), {"id": "u-1"})
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
    slug = "e2e-users-" + secrets.token_hex(3)
    project = api.create_project(slug, f"E2E users affected {slug}")

    proc = subprocess.run(
        [sys.executable, os.path.abspath(__file__), "--worker"],
        env=dict(os.environ, SENTRY_DSN=project["dsn"]),
        capture_output=True, text=True, timeout=120, check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"worker failed:\n{proc.stdout}\n{proc.stderr[-3000:]}")

    def list_issues(sort: str) -> list[dict[str, Any]]:
        resp = api.request_json(
            "GET", f"/api/v1/projects/{urllib.parse.quote(slug)}/issues?per_page=50&sort={sort}"
        )
        return list(resp.get("items", []))

    deadline = time.time() + 20
    items: list[dict[str, Any]] = []
    while time.time() < deadline:
        items = list_issues("users")
        if sum(i["event_count"] for i in items) >= 19:
            break
        time.sleep(0.3)
    by_type = {i["exception_type"]: i for i in items}
    assert set(by_type) == {"WideImpactError", "NoisyError"}, by_type.keys()

    wide, noisy = by_type["WideImpactError"], by_type["NoisyError"]
    assert wide["event_count"] == 7 and wide["user_count"] == 3, wide
    assert noisy["event_count"] == 12 and noisy["user_count"] == 1, noisy
    print(f"[ok] user_count: WideImpactError={wide['user_count']} NoisyError={noisy['user_count']}")

    assert [i["exception_type"] for i in items] == ["WideImpactError", "NoisyError"], items
    by_events = list_issues("events")
    assert by_events[0]["exception_type"] == "NoisyError", by_events
    print("[ok] sort=users ranks by impact, sort=events by volume")

    listing = str(items).lower()
    assert SECRET_EMAIL.lower() not in listing and "203.0.113.9" not in listing
    print("[ok] issues list exposes only counts, never raw user identities")
    print("F018 users affected E2E: PASS")


if __name__ == "__main__":
    main()
