#!/usr/bin/env python3
"""End-to-end verification of Maxi Loop against the official Python sentry-sdk
with both FastAPI and Django integrations — changing ONLY the Sentry DSN.

Covers BOTH:
1. Error tracking: grouping, in-app stack frames + source context, PII scrubbing.
2. Performance monitoring: transactions, 1-minute duration histograms (p50/p95/p99),
   error rate, sampled zstd traces, span PII scrubbing, and a cross-service
   FastAPI -> Django distributed trace waterfall (`sentry-trace` + `baggage`).
3. Negative auth check: tampered DSN key ingests 0 issues and 0 transactions.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import io
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

HTTP_TIMEOUT_SECS = 15


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
        traces_sample_rate=1.0,
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

    @app.get("/api/fastapi/orders/{order_id}")
    def get_order_traced(order_id: int) -> dict[str, Any]:
        with sentry_sdk.start_span(
            op="db.sql.query", name="SELECT * FROM orders WHERE id = $1"
        ) as db_span:
            db_span.set_data("db.system", "postgresql")
            db_span.set_data("password", os.environ["E2E_SPAN_SECRET"])
            time.sleep(0.03)

        with sentry_sdk.start_span(
            op="http.client", name=f"GET /django/orders/{order_id}/"
        ) as http_span:
            http_span.set_data("http.method", "GET")
            sentry_trace = sentry_sdk.get_traceparent()
            baggage = sentry_sdk.get_baggage()
            # Ensure >= 500ms total transaction duration so slow-trace sampling always triggers.
            time.sleep(0.55)
            return {
                "order_id": order_id,
                "trace_id": http_span.trace_id,
                "http_span_id": http_span.span_id,
                "sentry_trace": sentry_trace,
                "baggage": baggage,
            }

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

    traced_resp = client.get("/api/fastapi/orders/42", headers=sensitive_headers)
    assert traced_resp.status_code == 200, f"expected 200, got {traced_resp.status_code}"
    trace_info = traced_resp.json()
    assert trace_info.get("trace_id"), "missing trace_id from FastAPI span"
    assert trace_info.get("sentry_trace"), "missing sentry-trace header from FastAPI"

    sentry_sdk.flush(timeout=10.0)
    print(f"FASTAPI_TRACE_JSON={json.dumps(trace_info)}")


def _invoke_django_wsgi(
    wsgi_app: Any,
    path_info: str,
    headers: dict[str, str],
) -> tuple[str, bytes]:
    environ: dict[str, Any] = {
        "REQUEST_METHOD": "GET",
        "SCRIPT_NAME": "",
        "PATH_INFO": path_info,
        "QUERY_STRING": "",
        "SERVER_NAME": "localhost",
        "SERVER_PORT": "8000",
        "SERVER_PROTOCOL": "HTTP/1.1",
        "wsgi.version": (1, 0),
        "wsgi.url_scheme": "http",
        "wsgi.input": io.BytesIO(b""),
        "wsgi.errors": io.StringIO(),
        "wsgi.multithread": False,
        "wsgi.multiprocess": False,
        "wsgi.run_once": False,
        "HTTP_HOST": "localhost:8000",
    }
    for k, v in headers.items():
        if v:
            environ[f"HTTP_{k.upper().replace('-', '_')}"] = v

    status_holder: list[str] = []

    def start_response(status: str, _resp_headers: Any, _exc_info: Any = None) -> None:
        status_holder.append(status)

    chunks = wsgi_app(environ, start_response)
    try:
        body = b"".join(chunks)
    finally:
        if hasattr(chunks, "close"):
            chunks.close()
    return (status_holder[0] if status_holder else "500 Internal Server Error"), body


def run_django_worker() -> None:
    import django
    from django.conf import settings
    from django.core.wsgi import get_wsgi_application
    from django.http import HttpRequest, JsonResponse
    from django.urls import path
    import sentry_sdk
    from sentry_sdk.integrations.django import DjangoIntegration

    def django_checkout_view(_request: HttpRequest, order_id: int) -> JsonResponse:
        raise RuntimeError(f"payment gateway timeout for order #{order_id}")

    def django_lookup_view(_request: HttpRequest) -> JsonResponse:
        inventory: dict[str, int] = {}
        return JsonResponse({"qty": inventory["missing_warehouse_key"]})

    def django_orders_view(_request: HttpRequest, order_id: int) -> JsonResponse:
        with sentry_sdk.start_span(
            op="db.sql.query", name="SELECT * FROM django_orders WHERE id = %s"
        ) as db_span:
            db_span.set_data("db.system", "postgresql")
            db_span.set_data("api_key", os.environ["E2E_SPAN_SECRET"])
            # Ensure >= 500ms total transaction duration so slow-trace sampling always triggers.
            time.sleep(0.55)
        return JsonResponse({"order_id": order_id, "status": "ok"})

    urlconf_mod = types.ModuleType("e2e_django_urls")
    urlconf_mod.urlpatterns = [  # type: ignore[attr-defined]
        path("django/checkout/<int:order_id>/", django_checkout_view),
        path("django/lookup/", django_lookup_view),
        path("django/orders/<int:order_id>/", django_orders_view),
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
        traces_sample_rate=1.0,
        send_default_pii=True,
    )

    wsgi_app = get_wsgi_application()
    base_headers = {
        "Authorization": f"Bearer {os.environ['E2E_BEARER_TOKEN']}",
        "Cookie": f"session_id={os.environ['E2E_COOKIE_VALUE']}",
    }

    for order_id in (501, 502):
        status, _ = _invoke_django_wsgi(
            wsgi_app, f"/django/checkout/{order_id}/", base_headers
        )
        assert status.startswith("500"), f"expected 500, got {status}"

    status, _ = _invoke_django_wsgi(wsgi_app, "/django/lookup/", base_headers)
    assert status.startswith("500"), f"expected 500, got {status}"

    # Distributed downstream request continuing the FastAPI trace via sentry-trace + baggage.
    dist_headers = dict(base_headers)
    if os.environ.get("E2E_SENTRY_TRACE"):
        dist_headers["sentry-trace"] = os.environ["E2E_SENTRY_TRACE"]
    if os.environ.get("E2E_BAGGAGE"):
        dist_headers["baggage"] = os.environ["E2E_BAGGAGE"]

    status, body = _invoke_django_wsgi(wsgi_app, "/django/orders/42/", dist_headers)
    assert status.startswith("200"), f"expected 200, got {status}: {body!r}"

    sentry_sdk.flush(timeout=10.0)


def run_invalid_dsn_worker() -> None:
    import sentry_sdk

    dsn = os.environ["SENTRY_DSN"]
    sentry_sdk.init(
        dsn=dsn,
        environment="e2e-negative",
        traces_sample_rate=1.0,
    )
    with sentry_sdk.start_transaction(op="http.server", name="GET /invalid-dsn"):
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
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
        bearer_token: str | None = None,
    ) -> dict[str, Any]:
        url = f"{self.base_url}{path}"
        headers = {"Accept": "application/json"}
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

    def create_api_token(
        self, project_slug: str, name: str, can_write: bool = False
    ) -> dict[str, Any]:
        return self.request_json(
            "POST",
            "/api/v1/api-tokens",
            {
                "project_slug": project_slug,
                "name": name,
                "can_write": can_write,
            },
        )

    def list_project_issues(
        self, project_slug: str, bearer_token: str | None = None
    ) -> list[dict[str, Any]]:
        resp = self.request_json(
            "GET",
            f"/api/v1/projects/{urllib.parse.quote(project_slug)}/issues?per_page=50",
            bearer_token=bearer_token,
        )
        return list(resp.get("items", []))

    def get_issue_detail(
        self, issue_id: int, bearer_token: str | None = None
    ) -> dict[str, Any]:
        return self.request_json(
            "GET", f"/api/v1/issues/{issue_id}", bearer_token=bearer_token
        )

    def list_project_transactions(
        self, project_slug: str, bearer_token: str | None = None
    ) -> list[dict[str, Any]]:
        resp = self.request_json(
            "GET",
            f"/api/v1/performance/transactions?project_slug={urllib.parse.quote(project_slug)}&per_page=50",
            bearer_token=bearer_token,
        )
        return list(resp.get("items", []))

    def get_trace_waterfall(self, trace_id: str) -> dict[str, Any]:
        return self.request_json(
            "GET", f"/api/v1/performance/traces/{urllib.parse.quote(trace_id)}"
        )


def wait_for_issues(
    api: MaxiLoopApiClient,
    project_slug: str,
    expected_issue_count: int,
    expected_total_events: int,
    bearer_token: str | None = None,
    timeout_secs: float = 10.0,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout_secs
    last_items: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        items = api.list_project_issues(project_slug, bearer_token=bearer_token)
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


def wait_for_transactions(
    api: MaxiLoopApiClient,
    project_slug: str,
    expected_names: set[str],
    bearer_token: str | None = None,
    timeout_secs: float = 10.0,
) -> dict[str, dict[str, Any]]:
    deadline = time.monotonic() + timeout_secs
    last_by_name: dict[str, dict[str, Any]] = {}
    while time.monotonic() < deadline:
        items = api.list_project_transactions(project_slug, bearer_token=bearer_token)
        by_name = {str(t["transaction_name"]): t for t in items}
        last_by_name = by_name
        if expected_names.issubset(by_name.keys()):
            return by_name
        time.sleep(0.2)
    raise AssertionError(
        f"timed out waiting for project '{project_slug}' transactions: "
        f"expected {sorted(expected_names)}, got {sorted(last_by_name.keys())}"
    )


def wait_for_distributed_waterfall(
    api: MaxiLoopApiClient,
    trace_id: str,
    expected_projects: set[str],
    timeout_secs: float = 10.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_secs
    last_wf: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        try:
            wf = api.get_trace_waterfall(trace_id)
            last_wf = wf
            spans = wf.get("spans", [])
            seen_projects = {str(s.get("project_slug")) for s in spans}
            if expected_projects.issubset(seen_projects):
                return wf
        except RuntimeError:
            pass
        time.sleep(0.25)
    raise AssertionError(
        f"timed out waiting for distributed trace '{trace_id}' with projects "
        f"{sorted(expected_projects)}; last={last_wf}"
    )


def run_worker_subprocess(
    worker_name: str,
    dsn: str,
    bearer_token: str | None = None,
    cookie_value: str | None = None,
    span_secret: str | None = None,
    sentry_trace: str | None = None,
    baggage: str | None = None,
) -> str:
    env = os.environ.copy()
    env["SENTRY_DSN"] = dsn
    if bearer_token is not None:
        env["E2E_BEARER_TOKEN"] = bearer_token
    if cookie_value is not None:
        env["E2E_COOKIE_VALUE"] = cookie_value
    if span_secret is not None:
        env["E2E_SPAN_SECRET"] = span_secret
    if sentry_trace is not None:
        env["E2E_SENTRY_TRACE"] = sentry_trace
    if baggage is not None:
        env["E2E_BAGGAGE"] = baggage
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
    return proc.stdout


def parse_prefixed_json(stdout: str, prefix: str) -> dict[str, Any]:
    for line in stdout.splitlines():
        if line.startswith(prefix):
            return json.loads(line[len(prefix) :])
    raise RuntimeError(f"missing {prefix} in worker stdout: {stdout!r}")


def verify_issue_quality(
    api: MaxiLoopApiClient,
    issue_summary: dict[str, Any],
    expected_exc_type: str,
    expected_count: int,
    expected_env: str,
    expected_release: str,
    forbidden_secrets: list[str],
    bearer_token: str | None = None,
) -> None:
    assert issue_summary["exception_type"] == expected_exc_type, (
        f"expected exception_type={expected_exc_type}, got {issue_summary['exception_type']}"
    )
    assert int(issue_summary["event_count"]) == expected_count, (
        f"expected event_count={expected_count}, got {issue_summary['event_count']}"
    )
    assert issue_summary["last_environment"] == expected_env
    assert issue_summary["last_release"] == expected_release

    detail = api.get_issue_detail(int(issue_summary["id"]), bearer_token=bearer_token)
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
        assert secret not in payload_str, (
            f"PII leak detected in persisted payload for {expected_exc_type}"
        )
    assert "[Filtered]" in payload_str, (
        f"expected '[Filtered]' PII marker in scrubbed payload for {expected_exc_type}"
    )

    hourly_sum = sum(
        int(p.get("event_count", 0)) for p in detail.get("hourly_series", [])
    )
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
    fastapi_token = api.create_api_token(
        fastapi_proj["slug"], f"e2e-fa-ro-{suffix}", can_write=False
    )["raw_token"]
    django_token = api.create_api_token(
        django_proj["slug"], f"e2e-dj-ro-{suffix}", can_write=False
    )["raw_token"]

    # 1. Real official sentry-sdk + FastAPI integration (errors + performance + distributed root)
    fastapi_bearer = f"ephemeral_fa_bearer_{secrets.token_hex(8)}"
    fastapi_cookie = f"ephemeral_fa_cookie_{secrets.token_hex(8)}"
    fastapi_span_secret = f"ephemeral_fa_span_{secrets.token_hex(8)}"
    fa_stdout = run_worker_subprocess(
        "fastapi",
        fastapi_proj["dsn"],
        bearer_token=fastapi_bearer,
        cookie_value=fastapi_cookie,
        span_secret=fastapi_span_secret,
    )
    fa_trace = parse_prefixed_json(fa_stdout, "FASTAPI_TRACE_JSON=")

    fastapi_issues = wait_for_issues(
        api,
        fastapi_proj["slug"],
        expected_issue_count=2,
        expected_total_events=4,
        bearer_token=fastapi_token,
    )
    fastapi_by_type = {i["exception_type"]: i for i in fastapi_issues}
    assert set(fastapi_by_type.keys()) == {"ZeroDivisionError", "ValueError"}

    fastapi_secrets = [fastapi_bearer, fastapi_cookie, fastapi_span_secret]
    verify_issue_quality(
        api,
        fastapi_by_type["ZeroDivisionError"],
        expected_exc_type="ZeroDivisionError",
        expected_count=3,
        expected_env="e2e-fastapi",
        expected_release="e2e-sha-fa1234",
        forbidden_secrets=fastapi_secrets,
        bearer_token=fastapi_token,
    )
    verify_issue_quality(
        api,
        fastapi_by_type["ValueError"],
        expected_exc_type="ValueError",
        expected_count=1,
        expected_env="e2e-fastapi",
        expected_release="e2e-sha-fa1234",
        forbidden_secrets=fastapi_secrets,
        bearer_token=fastapi_token,
    )

    fa_txs = wait_for_transactions(
        api,
        fastapi_proj["slug"],
        {"/api/fastapi/orders/{order_id}", "/billing/divide/{invoice_id}"},
        bearer_token=fastapi_token,
    )
    fa_ok_tx = fa_txs["/api/fastapi/orders/{order_id}"]
    assert fa_ok_tx["total_count"] == 1 and fa_ok_tx["error_count"] == 0, fa_ok_tx
    assert float(fa_ok_tx["p50_ms"]) >= 500.0, f"expected slow FastAPI tx, got {fa_ok_tx}"
    fa_err_tx = fa_txs["/billing/divide/{invoice_id}"]
    assert fa_err_tx["total_count"] == 3 and fa_err_tx["error_count"] == 3, fa_err_tx
    print(
        f"[PASS] FastAPI sentry-sdk E2E: 4 exceptions -> 2 grouped issues "
        f"(ZeroDivisionError x3, ValueError x1), PII scrubbed; "
        f"perf tx p50={float(fa_ok_tx['p50_ms']):.1f}ms, error tx 3/3 errors"
    )

    # 2. Real official sentry-sdk + Django WSGI integration (errors + performance + distributed child)
    django_bearer = f"ephemeral_dj_bearer_{secrets.token_hex(8)}"
    django_cookie = f"ephemeral_dj_cookie_{secrets.token_hex(8)}"
    django_span_secret = f"ephemeral_dj_span_{secrets.token_hex(8)}"
    run_worker_subprocess(
        "django",
        django_proj["dsn"],
        bearer_token=django_bearer,
        cookie_value=django_cookie,
        span_secret=django_span_secret,
        sentry_trace=fa_trace["sentry_trace"],
        baggage=fa_trace.get("baggage"),
    )

    django_issues = wait_for_issues(
        api,
        django_proj["slug"],
        expected_issue_count=2,
        expected_total_events=3,
        bearer_token=django_token,
    )
    django_by_type = {i["exception_type"]: i for i in django_issues}
    assert set(django_by_type.keys()) == {"RuntimeError", "KeyError"}

    django_secrets = [django_bearer, django_cookie, django_span_secret]
    verify_issue_quality(
        api,
        django_by_type["RuntimeError"],
        expected_exc_type="RuntimeError",
        expected_count=2,
        expected_env="e2e-django",
        expected_release="e2e-sha-dj5678",
        forbidden_secrets=django_secrets,
        bearer_token=django_token,
    )
    verify_issue_quality(
        api,
        django_by_type["KeyError"],
        expected_exc_type="KeyError",
        expected_count=1,
        expected_env="e2e-django",
        expected_release="e2e-sha-dj5678",
        forbidden_secrets=django_secrets,
        bearer_token=django_token,
    )

    dj_txs = wait_for_transactions(
        api,
        django_proj["slug"],
        {"/django/orders/{order_id}/", "/django/checkout/{order_id}/"},
        bearer_token=django_token,
    )
    dj_ok_tx = dj_txs["/django/orders/{order_id}/"]
    assert dj_ok_tx["total_count"] == 1 and dj_ok_tx["error_count"] == 0, dj_ok_tx
    assert float(dj_ok_tx["p50_ms"]) >= 500.0, f"expected slow Django tx, got {dj_ok_tx}"
    dj_err_tx = dj_txs["/django/checkout/{order_id}/"]
    assert dj_err_tx["total_count"] == 2 and dj_err_tx["error_count"] == 2, dj_err_tx
    print(
        f"[PASS] Django sentry-sdk E2E: 3 exceptions -> 2 grouped issues "
        f"(RuntimeError x2, KeyError x1), PII scrubbed; "
        f"perf tx p50={float(dj_ok_tx['p50_ms']):.1f}ms, error tx 2/2 errors"
    )

    # 3. Cross-service distributed trace waterfall (FastAPI -> Django) + trace PII scrubbing
    trace_id = str(fa_trace["trace_id"])
    waterfall = wait_for_distributed_waterfall(
        api,
        trace_id,
        expected_projects={fastapi_proj["slug"], django_proj["slug"]},
    )
    spans = waterfall.get("spans", [])
    fa_spans = [s for s in spans if s["project_slug"] == fastapi_proj["slug"]]
    dj_spans = [s for s in spans if s["project_slug"] == django_proj["slug"]]
    assert any(s["op"] == "http.server" and s["depth"] == 0 for s in fa_spans), fa_spans
    assert any(s["op"] == "db.sql.query" for s in fa_spans), fa_spans
    assert any(s["op"] == "http.client" for s in fa_spans), fa_spans
    dj_root = next(
        (s for s in dj_spans if s["op"] == "http.server" and s["is_transaction_root"]),
        None,
    )
    assert dj_root is not None, f"missing Django root span in waterfall: {dj_spans}"
    assert dj_root["parent_span_id"] == fa_trace["http_span_id"], (
        f"Django root parent_span_id ({dj_root['parent_span_id']}) != FastAPI http.client span ({fa_trace['http_span_id']})"
    )
    assert int(dj_root["depth"]) >= 2, f"expected nested Django depth >= 2, got {dj_root}"
    assert any(s["op"] == "db.sql.query" for s in dj_spans), dj_spans

    wf_serialized = json.dumps(waterfall)
    for secret in fastapi_secrets + django_secrets:
        assert secret not in wf_serialized, "PII leak detected in sampled trace waterfall!"
    assert "[Filtered]" in wf_serialized, "expected [Filtered] PII marker in trace waterfall"
    print(
        f"[PASS] Distributed FastAPI -> Django waterfall ({trace_id}): "
        f"{len(spans)} spans across 2 services (FastAPI={len(fa_spans)}, Django={len(dj_spans)}), "
        f"parent linkage & [Filtered] PII verified"
    )

    # 4. Negative check: tampered DSN public key must NOT ingest any event or transaction
    tampered_dsn = build_tampered_dsn(fastapi_proj["dsn"])
    run_worker_subprocess("invalid-dsn", tampered_dsn)
    time.sleep(0.5)
    after_invalid_issues = api.list_project_issues(
        fastapi_proj["slug"], bearer_token=fastapi_token
    )
    assert len(after_invalid_issues) == 2 and sum(
        int(i["event_count"]) for i in after_invalid_issues
    ) == 4, "tampered DSN unexpectedly created or incremented an issue"
    after_invalid_txs = api.list_project_transactions(
        fastapi_proj["slug"], bearer_token=fastapi_token
    )
    assert not any(
        t["transaction_name"] == "GET /invalid-dsn" for t in after_invalid_txs
    ), "tampered DSN unexpectedly ingested a performance transaction"
    print("[PASS] Negative check: invalid DSN key rejected (0 issues & 0 transactions ingested)")


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
