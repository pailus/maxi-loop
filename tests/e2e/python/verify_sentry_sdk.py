#!/usr/bin/env python3
"""End-to-end verification of Maxi Loop against the official Python sentry-sdk
with both FastAPI and Django integrations — changing ONLY the Sentry DSN.
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
import types
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

HTTP_TIMEOUT_SECS = 10


def run_fastapi_worker() -> None:
    import sentry_sdk
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from sentry_sdk.integrations.fastapi import FastApiIntegration

    dsn = os.environ["SENTRY_DSN"]
    sentry_sdk.init(
        dsn=dsn,
        integrations=[FastApiIntegration()],
        environment="e2e-fastapi",
        release="e2e-sha-fa1234",
        traces_sample_rate=0.0,
        send_default_pii=True,
    )

    app = FastAPI()

    @app.get("/billing/divide/{invoice_id}")
    def billing_divide(invoice_id: int) -> dict[str, Any]:
        denominator = 0
        ratio = invoice_id / denominator
        return {"invoice_id": invoice_id, "ratio": ratio}

    @app.get("/orders/validate/{sku}")
    def validate_order(sku: str) -> dict[str, Any]:
        raise ValueError(f"invalid SKU format: {sku}")

    client = TestClient(app, raise_server_exceptions=False)
    sensitive_headers = {
        "Authorization": f"Bearer {os.environ['E2E_BEARER_TOKEN']}",
        "Cookie": f"session_id={os.environ['E2E_COOKIE_VALUE']}",
    }

    for invoice_id in (1001, 1002, 1003):
        resp = client.get(f"/billing/divide/{invoice_id}", headers=sensitive_headers)
        assert resp.status_code == 500, f"expected 500, got {resp.status_code}"

    resp = client.get("/orders/validate/bad-sku-99", headers=sensitive_headers)
    assert resp.status_code == 500, f"expected 500, got {resp.status_code}"

    sentry_sdk.flush(timeout=10.0)


def run_django_worker() -> None:
    import django
    from django.conf import settings
    from django.http import HttpRequest, JsonResponse
    from django.test import Client
    from django.urls import path
    import sentry_sdk
    from sentry_sdk.integrations.django import DjangoIntegration

    def django_checkout_view(_request: HttpRequest, order_id: int) -> JsonResponse:
        raise RuntimeError(f"payment gateway timeout for order #{order_id}")

    def django_lookup_view(_request: HttpRequest) -> JsonResponse:
        inventory: dict[str, int] = {}
        return JsonResponse({"qty": inventory["missing_warehouse_key"]})

    urlconf_mod = types.ModuleType("e2e_django_urls")
    urlconf_mod.urlpatterns = [  # type: ignore[attr-defined]
        path("django/checkout/<int:order_id>/", django_checkout_view),
        path("django/lookup/", django_lookup_view),
    ]
    sys.modules["e2e_django_urls"] = urlconf_mod

    settings.configure(
        DEBUG=False,
        SECRET_KEY="ephemeral-test-only-not-production-" + secrets.token_hex(16),
        ALLOWED_HOSTS=["testserver", "localhost", "127.0.0.1"],
        ROOT_URLCONF="e2e_django_urls",
        MIDDLEWARE=[],
        INSTALLED_APPS=[],
    )
    django.setup()

    dsn = os.environ["SENTRY_DSN"]
    sentry_sdk.init(
        dsn=dsn,
        integrations=[DjangoIntegration()],
        environment="e2e-django",
        release="e2e-sha-dj5678",
        traces_sample_rate=0.0,
        send_default_pii=True,
    )

    client = Client(raise_request_exception=False)
    auth_header = f"Bearer {os.environ['E2E_BEARER_TOKEN']}"
    cookie_header = f"session_id={os.environ['E2E_COOKIE_VALUE']}"
    for order_id in (501, 502):
        resp = client.get(
            f"/django/checkout/{order_id}/",
            HTTP_AUTHORIZATION=auth_header,
            HTTP_COOKIE=cookie_header,
        )
        assert resp.status_code == 500, f"expected 500, got {resp.status_code}"

    resp = client.get(
        "/django/lookup/",
        HTTP_AUTHORIZATION=auth_header,
        HTTP_COOKIE=cookie_header,
    )
    assert resp.status_code == 500, f"expected 500, got {resp.status_code}"

    sentry_sdk.flush(timeout=10.0)


def run_invalid_dsn_worker() -> None:
    import sentry_sdk

    dsn = os.environ["SENTRY_DSN"]
    sentry_sdk.init(
        dsn=dsn,
        environment="e2e-negative",
        traces_sample_rate=0.0,
    )
    try:
        raise PermissionError("should be rejected due to invalid DSN key")
    except PermissionError as exc:
        sentry_sdk.capture_exception(exc)
    sentry_sdk.flush(timeout=5.0)


class MaxiLoopApiClient:
    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(self.jar)
        )
        self.csrf_token: str | None = None

    def request_json(
        self, method: str, path: str, payload: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        url = f"{self.base_url}{path}"
        headers = {"Accept": "application/json"}
        data: bytes | None = None
        if payload is not None:
            data = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        if method not in ("GET", "HEAD", "OPTIONS") and self.csrf_token:
            headers["X-CSRF-Token"] = self.csrf_token

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with self.opener.open(req, timeout=HTTP_TIMEOUT_SECS) as resp:
                raw = resp.read().decode("utf-8")
                return json.loads(raw) if raw else {}
        except urllib.error.HTTPError as err:
            body = err.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"{method} {path} failed ({err.code}): {body}") from err

    def login(self, email: str, password: str) -> None:
        resp = self.request_json(
            "POST",
            "/api/v1/auth/login",
            {"email": email, "password": password},
        )
        token = resp.get("csrf_token")
        if not token:
            raise RuntimeError("login response missing csrf_token")
        self.csrf_token = str(token)

    def create_project(self, slug: str, name: str) -> dict[str, Any]:
        return self.request_json(
            "POST",
            "/api/v1/projects",
            {
                "slug": slug,
                "name": name,
                "platform": "python",
                "rate_limit_per_minute": 6000,
            },
        )

    def list_project_issues(self, project_slug: str) -> list[dict[str, Any]]:
        resp = self.request_json(
            "GET",
            f"/api/v1/projects/{urllib.parse.quote(project_slug)}/issues?per_page=50",
        )
        return list(resp.get("items", []))

    def get_issue_detail(self, issue_id: int) -> dict[str, Any]:
        return self.request_json("GET", f"/api/v1/issues/{issue_id}")


def wait_for_issues(
    api: MaxiLoopApiClient,
    project_slug: str,
    expected_issue_count: int,
    expected_total_events: int,
    timeout_secs: float = 8.0,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout_secs
    last_items: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        items = api.list_project_issues(project_slug)
        total_events = sum(int(i.get("event_count", 0)) for i in items)
        last_items = items
        if len(items) == expected_issue_count and total_events == expected_total_events:
            return items
        time.sleep(0.2)
    raise AssertionError(
        f"timed out waiting for project '{project_slug}' issues: "
        f"expected {expected_issue_count} issues / {expected_total_events} events, "
        f"got {len(last_items)} issues / {sum(int(i.get('event_count', 0)) for i in last_items)} events"
    )


def run_worker_subprocess(
    worker_name: str,
    dsn: str,
    bearer_token: str | None = None,
    cookie_value: str | None = None,
) -> None:
    env = os.environ.copy()
    env["SENTRY_DSN"] = dsn
    if bearer_token is not None:
        env["E2E_BEARER_TOKEN"] = bearer_token
    if cookie_value is not None:
        env["E2E_COOKIE_VALUE"] = cookie_value
    proc = subprocess.run(
        [sys.executable, os.path.abspath(__file__), "--worker", worker_name],
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"worker '{worker_name}' failed (exit {proc.returncode}):\n"
            f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
        )


def verify_issue_quality(
    api: MaxiLoopApiClient,
    issue_summary: dict[str, Any],
    expected_exc_type: str,
    expected_count: int,
    expected_env: str,
    expected_release: str,
    forbidden_secrets: list[str],
) -> None:
    assert issue_summary["exception_type"] == expected_exc_type, (
        f"expected exception_type={expected_exc_type}, got {issue_summary['exception_type']}"
    )
    assert int(issue_summary["event_count"]) == expected_count, (
        f"expected event_count={expected_count}, got {issue_summary['event_count']}"
    )
    assert issue_summary["last_environment"] == expected_env
    assert issue_summary["last_release"] == expected_release

    detail = api.get_issue_detail(int(issue_summary["id"]))
    frames = detail.get("stack_frames", [])
    in_app_frames = [f for f in frames if f.get("in_app") is True]
    assert in_app_frames, f"expected at least one in_app frame for {expected_exc_type}"
    assert any(f.get("context_line") for f in in_app_frames), (
        f"expected source context_line in in_app frames for {expected_exc_type}"
    )

    sample = detail.get("sample_event")
    assert sample is not None, f"expected sample_event for {expected_exc_type}"
    payload_str = json.dumps(sample.get("payload", {}))
    for secret in forbidden_secrets:
        assert secret not in payload_str, f"PII leak detected in persisted payload for {expected_exc_type}"
    assert "[Filtered]" in payload_str, (
        f"expected '[Filtered]' PII marker in scrubbed payload for {expected_exc_type}"
    )

    hourly_sum = sum(int(p.get("event_count", 0)) for p in detail.get("hourly_series", []))
    assert hourly_sum == expected_count, (
        f"hourly_series sum ({hourly_sum}) != event_count ({expected_count})"
    )


def build_tampered_dsn(valid_dsn: str) -> str:
    parsed = urllib.parse.urlparse(valid_dsn)
    netloc = f"00000000000000000000000000000000@{parsed.hostname}"
    if parsed.port:
        netloc = f"{netloc}:{parsed.port}"
    return urllib.parse.urlunparse(parsed._replace(netloc=netloc))


def run_orchestrator() -> None:
    base_url = os.environ.get("MAXI_LOOP_BASE_URL", "http://127.0.0.1:8080")
    admin_email = os.environ.get("MAXI_LOOP_ADMIN_EMAIL")
    admin_password = os.environ.get("MAXI_LOOP_ADMIN_PASSWORD")
    if not admin_email or not admin_password:
        raise SystemExit(
            "MAXI_LOOP_ADMIN_EMAIL and MAXI_LOOP_ADMIN_PASSWORD environment variables are required"
        )

    suffix = secrets.token_hex(4)
    api = MaxiLoopApiClient(base_url)
    api.login(admin_email, admin_password)

    fastapi_proj = api.create_project(
        f"e2e-fastapi-{suffix}", f"E2E FastAPI ({suffix})"
    )
    django_proj = api.create_project(
        f"e2e-django-{suffix}", f"E2E Django ({suffix})"
    )

    # 1. Real official sentry-sdk + FastAPI integration
    fastapi_bearer = f"ephemeral_fa_bearer_{secrets.token_hex(8)}"
    fastapi_cookie = f"ephemeral_fa_cookie_{secrets.token_hex(8)}"
    run_worker_subprocess("fastapi", fastapi_proj["dsn"], fastapi_bearer, fastapi_cookie)
    fastapi_issues = wait_for_issues(
        api, fastapi_proj["slug"], expected_issue_count=2, expected_total_events=4
    )
    fastapi_by_type = {i["exception_type"]: i for i in fastapi_issues}
    assert set(fastapi_by_type.keys()) == {"ZeroDivisionError", "ValueError"}

    fastapi_secrets = [fastapi_bearer, fastapi_cookie]
    verify_issue_quality(
        api,
        fastapi_by_type["ZeroDivisionError"],
        expected_exc_type="ZeroDivisionError",
        expected_count=3,
        expected_env="e2e-fastapi",
        expected_release="e2e-sha-fa1234",
        forbidden_secrets=fastapi_secrets,
    )
    verify_issue_quality(
        api,
        fastapi_by_type["ValueError"],
        expected_exc_type="ValueError",
        expected_count=1,
        expected_env="e2e-fastapi",
        expected_release="e2e-sha-fa1234",
        forbidden_secrets=fastapi_secrets,
    )
    print(
        f"[PASS] FastAPI sentry-sdk E2E: 4 exceptions -> 2 grouped issues "
        f"(ZeroDivisionError x3, ValueError x1), in-app source context present, PII scrubbed"
    )

    # 2. Real official sentry-sdk + Django integration
    django_bearer = f"ephemeral_dj_bearer_{secrets.token_hex(8)}"
    django_cookie = f"ephemeral_dj_cookie_{secrets.token_hex(8)}"
    run_worker_subprocess("django", django_proj["dsn"], django_bearer, django_cookie)
    django_issues = wait_for_issues(
        api, django_proj["slug"], expected_issue_count=2, expected_total_events=3
    )
    django_by_type = {i["exception_type"]: i for i in django_issues}
    assert set(django_by_type.keys()) == {"RuntimeError", "KeyError"}

    django_secrets = [django_bearer, django_cookie]
    verify_issue_quality(
        api,
        django_by_type["RuntimeError"],
        expected_exc_type="RuntimeError",
        expected_count=2,
        expected_env="e2e-django",
        expected_release="e2e-sha-dj5678",
        forbidden_secrets=django_secrets,
    )
    verify_issue_quality(
        api,
        django_by_type["KeyError"],
        expected_exc_type="KeyError",
        expected_count=1,
        expected_env="e2e-django",
        expected_release="e2e-sha-dj5678",
        forbidden_secrets=django_secrets,
    )
    print(
        f"[PASS] Django sentry-sdk E2E: 3 exceptions -> 2 grouped issues "
        f"(RuntimeError x2, KeyError x1), in-app source context present, PII scrubbed"
    )

    # 3. Negative check: tampered DSN public key must NOT ingest any event
    tampered_dsn = build_tampered_dsn(fastapi_proj["dsn"])
    run_worker_subprocess("invalid-dsn", tampered_dsn)
    time.sleep(0.5)
    after_invalid = api.list_project_issues(fastapi_proj["slug"])
    assert len(after_invalid) == 2 and sum(int(i["event_count"]) for i in after_invalid) == 4, (
        "tampered DSN unexpectedly created or incremented an issue"
    )
    print("[PASS] Negative check: invalid DSN key rejected (0 events ingested)")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Official Python sentry-sdk (FastAPI & Django) E2E verifier for Maxi Loop"
    )
    parser.add_argument(
        "--worker",
        choices=("fastapi", "django", "invalid-dsn"),
        default=None,
        help="Internal worker mode for isolated SDK execution",
    )
    args = parser.parse_args()

    if args.worker == "fastapi":
        run_fastapi_worker()
    elif args.worker == "django":
        run_django_worker()
    elif args.worker == "invalid-dsn":
        run_invalid_dsn_worker()
    else:
        run_orchestrator()


if __name__ == "__main__":
    main()
