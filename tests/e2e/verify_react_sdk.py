#!/usr/bin/env python3
"""End-to-end verification of Maxi Loop against the official browser `@sentry/react` SDK
running inside real Headless Chrome (`google-chrome --headless=new`) across origins.

Verifies:
1. Non-credentialed CORS preflight & headers on `/api/{project_id}/envelope/`
   (and confirms `/api/v1/*` does NOT emit wildcard CORS).
2. Production Vite build (`dist/assets/*.js` + `.js.map`) + CI Source Map upload
   using a per-project write API token (`Authorization: Bearer mlapi_...`).
3. Minified React error capture (`TypeError` x3, `RangeError` x1) resolved back to
   original TypeScript (`src/checkout.ts`) with `context_line`, `pre_context`,
   `post_context`, and `[Filtered]` PII scrubbing.
4. Real cross-origin Browser (React) -> Backend (Django WSGI) distributed trace
   waterfall (`sentry-trace` + `baggage` propagation via `fetch`).
5. Negative check: tampered DSN in Chrome ingests 0 issues and 0 transactions.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import http.server
import json
import mimetypes
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import types
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

HTTP_TIMEOUT_SECS = 15
ROOT_DIR = Path(__file__).resolve().parents[2]
REACT_DIR = ROOT_DIR / "tests" / "e2e" / "react"
REACT_DIST = REACT_DIR / "dist"


def find_chrome_binary() -> str:
    for candidate in (
        os.environ.get("CHROME_BIN"),
        shutil.which("google-chrome"),
        shutil.which("chromium"),
        shutil.which("chromium-browser"),
    ):
        if candidate and Path(candidate).is_file():
            return candidate
    raise RuntimeError("No headless Chrome/Chromium binary found on host")


def run_backend_worker() -> None:
    """Isolated Django WSGI backend instrumented with official sentry-sdk to receive
    the cross-origin `fetch` + `sentry-trace` + `baggage` headers from Chrome."""
    from wsgiref.simple_server import WSGIRequestHandler, make_server

    import django
    from django.conf import settings
    from django.core.wsgi import get_wsgi_application
    from django.http import HttpRequest, JsonResponse
    from django.urls import path
    import sentry_sdk
    from sentry_sdk.integrations.django import DjangoIntegration

    allowed_origin = os.environ["E2E_FRONTEND_ORIGIN"]
    span_secret = os.environ["E2E_SPAN_SECRET"]

    def checkout_api_view(request: HttpRequest, order_id: int) -> JsonResponse:
        trace_header = request.META.get("HTTP_SENTRY_TRACE", "")
        parts = trace_header.split("-") if trace_header else []
        incoming_trace_id = parts[0] if len(parts) >= 2 else ""
        incoming_parent_span_id = parts[1] if len(parts) >= 2 else ""

        with sentry_sdk.start_span(
            op="db.sql.query", name="SELECT * FROM checkout_orders WHERE id = %s"
        ) as sp:
            sp.set_data("db.system", "postgresql")
            sp.set_data("password", span_secret)
            # Ensure >= 500ms so both browser & backend segments exceed slow threshold.
            time.sleep(0.55)

        return JsonResponse(
            {
                "ok": True,
                "order_id": order_id,
                "trace_id": incoming_trace_id,
                "parent_span_id": incoming_parent_span_id,
            }
        )

    urlconf_mod = types.ModuleType("e2e_react_backend_urls")
    urlconf_mod.urlpatterns = [  # type: ignore[attr-defined]
        path("api/checkout/<int:order_id>", checkout_api_view),
        path("api/checkout/<int:order_id>/", checkout_api_view),
    ]
    sys.modules["e2e_react_backend_urls"] = urlconf_mod

    settings.configure(
        DEBUG=False,
        SECRET_KEY="ephemeral-react-e2e-backend-" + secrets.token_hex(16),
        ALLOWED_HOSTS=["127.0.0.1", "localhost"],
        ROOT_URLCONF="e2e_react_backend_urls",
        MIDDLEWARE=[],
        INSTALLED_APPS=[],
    )
    django.setup()

    sentry_sdk.init(
        dsn=os.environ["SENTRY_DSN"],
        integrations=[DjangoIntegration()],
        environment="e2e-react-backend",
        release=os.environ["E2E_RELEASE"],
        traces_sample_rate=1.0,
        send_default_pii=True,
    )

    django_wsgi = get_wsgi_application()

    def cors_wsgi_app(environ: dict[str, Any], start_response: Any) -> Any:
        cors_headers = [
            ("Access-Control-Allow-Origin", allowed_origin),
            ("Access-Control-Allow-Methods", "GET, OPTIONS"),
            ("Access-Control-Allow-Headers", "Accept, Content-Type, sentry-trace, baggage"),
            ("Vary", "Origin"),
        ]
        if environ.get("REQUEST_METHOD") == "OPTIONS":
            start_response("204 No Content", cors_headers)
            return [b""]

        def wrapped_start_response(
            status: str, response_headers: list[tuple[str, str]], exc_info: Any = None
        ) -> Any:
            return start_response(status, list(response_headers) + cors_headers, exc_info)

        result = django_wsgi(environ, wrapped_start_response)
        try:
            body = b"".join(result)
        finally:
            if hasattr(result, "close"):
                result.close()
        # Flush backend transaction promptly so the waterfall is ready right away.
        sentry_sdk.flush(timeout=5.0)
        return [body]

    class QuietHandler(WSGIRequestHandler):
        def log_message(self, format: str, *args: Any) -> None:
            return

    httpd = make_server("127.0.0.1", 0, cors_wsgi_app, handler_class=QuietHandler)
    port = httpd.server_address[1]
    print(f"BACKEND_PORT={port}", flush=True)
    try:
        httpd.serve_forever()
    finally:
        httpd.server_close()
        sentry_sdk.flush(timeout=5.0)


class MaxiLoopApiClient:
    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(self.jar)
        )
        self.csrf_token: str | None = None

    def request_raw(
        self,
        method: str,
        path: str,
        body: bytes | None = None,
        headers: dict[str, str] | None = None,
        bearer_token: str | None = None,
    ) -> tuple[int, dict[str, str], bytes]:
        url = f"{self.base_url}{path}"
        req_headers = {"Accept": "application/json"}
        if headers:
            req_headers.update(headers)
        if bearer_token:
            req_headers["Authorization"] = f"Bearer {bearer_token}"
        elif method not in ("GET", "HEAD", "OPTIONS") and self.csrf_token:
            req_headers["X-CSRF-Token"] = self.csrf_token

        req = urllib.request.Request(url, data=body, headers=req_headers, method=method)
        try:
            with self.opener.open(req, timeout=HTTP_TIMEOUT_SECS) as resp:
                resp_headers = {k.lower(): v for k, v in resp.headers.items()}
                return resp.status, resp_headers, resp.read()
        except urllib.error.HTTPError as err:
            resp_headers = {k.lower(): v for k, v in err.headers.items()}
            return err.code, resp_headers, err.read()

    def request_json(
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
        bearer_token: str | None = None,
    ) -> dict[str, Any]:
        body: bytes | None = None
        headers: dict[str, str] = {}
        if payload is not None:
            body = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        status, _, raw = self.request_raw(
            method, path, body=body, headers=headers, bearer_token=bearer_token
        )
        if status >= 400:
            raise RuntimeError(
                f"{method} {path} failed ({status}): {raw.decode('utf-8', errors='replace')}"
            )
        return json.loads(raw.decode("utf-8")) if raw else {}

    def login(self, email: str, password: str) -> None:
        resp = self.request_json(
            "POST", "/api/v1/auth/login", {"email": email, "password": password}
        )
        token = resp.get("csrf_token")
        if not token:
            raise RuntimeError("login response missing csrf_token")
        self.csrf_token = str(token)

    def create_project(self, slug: str, name: str, platform: str) -> dict[str, Any]:
        return self.request_json(
            "POST",
            "/api/v1/projects",
            {
                "slug": slug,
                "name": name,
                "platform": platform,
                "rate_limit_per_minute": 6000,
            },
        )

    def create_api_token(
        self, project_slug: str, name: str, can_write: bool
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

    def upload_sourcemap_raw(
        self,
        project_slug: str,
        release_version: str,
        filename: str,
        raw_bytes: bytes,
        write_token: str,
    ) -> dict[str, Any]:
        q_file = urllib.parse.quote(filename, safe="")
        path = (
            f"/api/v1/projects/{urllib.parse.quote(project_slug)}"
            f"/releases/{urllib.parse.quote(release_version)}/sourcemaps?filename={q_file}"
        )
        status, _, raw = self.request_raw(
            "POST",
            path,
            body=raw_bytes,
            headers={"Content-Type": "application/json"},
            bearer_token=write_token,
        )
        if status != 201:
            raise RuntimeError(
                f"sourcemap upload failed ({status}): {raw.decode('utf-8', errors='replace')}"
            )
        return json.loads(raw.decode("utf-8"))


class StaticReactTestServer:
    """Serves `tests/e2e/react/dist/`, dynamic `/e2e-config.js`, and `/e2e-done` callback."""

    def __init__(self, dist_dir: Path) -> None:
        self.dist_dir = dist_dir.resolve()
        self.config_payload: dict[str, Any] = {}
        self.done_event = threading.Event()
        self.done_payload: dict[str, Any] | None = None

        parent = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: Any) -> None:
                return

            def do_GET(self) -> None:
                parsed = urllib.parse.urlparse(self.path)
                req_path = parsed.path
                if req_path == "/e2e-config.js":
                    js = f"window.__MAXI_LOOP_E2E__ = {json.dumps(parent.config_payload)};\n".encode(
                        "utf-8"
                    )
                    self.send_response(200)
                    self.send_header("Content-Type", "application/javascript; charset=utf-8")
                    self.send_header("Content-Length", str(len(js)))
                    self.send_header("Cache-Control", "no-store")
                    self.end_headers()
                    self.wfile.write(js)
                    return

                rel = "index.html" if req_path in ("", "/") else req_path.lstrip("/")
                candidate = (parent.dist_dir / rel).resolve()
                if not candidate.is_relative_to(parent.dist_dir) or not candidate.is_file():
                    self.send_error(404)
                    return

                content = candidate.read_bytes()
                ctype, _ = mimetypes.guess_type(str(candidate))
                self.send_response(200)
                self.send_header("Content-Type", ctype or "application/octet-stream")
                self.send_header("Content-Length", str(len(content)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(content)

            def do_POST(self) -> None:
                parsed = urllib.parse.urlparse(self.path)
                if parsed.path != "/e2e-done":
                    self.send_error(404)
                    return
                length = int(self.headers.get("Content-Length", "0"))
                raw = self.rfile.read(min(length, 65536))
                parent.done_payload = json.loads(raw.decode("utf-8")) if raw else {}
                parent.done_event.set()
                resp = b'{"ok":true}'
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(resp)))
                self.end_headers()
                self.wfile.write(resp)

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.httpd.server_address[1]
        self.origin = f"http://127.0.0.1:{self.port}"
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    def reset_for_run(self, config_payload: dict[str, Any]) -> None:
        self.config_payload = config_payload
        self.done_payload = None
        self.done_event.clear()

    def close(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()


def run_chrome_session(
    chrome_bin: str,
    server: StaticReactTestServer,
    config_payload: dict[str, Any],
    timeout_secs: float = 25.0,
) -> dict[str, Any]:
    server.reset_for_run(config_payload)
    profile_dir = tempfile.mkdtemp(prefix="maxi-loop-chrome-")
    proc = subprocess.Popen(
        [
            chrome_bin,
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            f"--user-data-dir={profile_dir}",
            f"{server.origin}/",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        completed = server.done_event.wait(timeout=timeout_secs)
        if not completed:
            proc.terminate()
            out, err = proc.communicate(timeout=5)
            raise RuntimeError(
                f"Headless Chrome timed out waiting for /e2e-done.\nstdout:\n{out}\nstderr:\n{err}"
            )
        payload = server.done_payload or {}
        if not payload.get("ok"):
            raise RuntimeError(f"React E2E page reported error: {payload}")
        return payload
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
        shutil.rmtree(profile_dir, ignore_errors=True)


def poll_until(desc: str, fn: Any, timeout_secs: float = 12.0) -> Any:
    deadline = time.monotonic() + timeout_secs
    last_val = None
    while time.monotonic() < deadline:
        last_val = fn()
        if last_val:
            return last_val
        time.sleep(0.25)
    raise AssertionError(f"Timed out waiting for {desc}; last={last_val}")


def build_tampered_dsn(valid_dsn: str) -> str:
    parsed = urllib.parse.urlparse(valid_dsn)
    netloc = f"00000000000000000000000000000000@{parsed.hostname}"
    if parsed.port:
        netloc = f"{netloc}:{parsed.port}"
    return urllib.parse.urlunparse(parsed._replace(netloc=netloc))


def run_orchestrator() -> None:
    if not (REACT_DIST / "index.html").is_file():
        raise SystemExit(
            f"React E2E dist not built at {REACT_DIST}. Run `npm --prefix tests/e2e/react run build` first."
        )

    base_url = os.environ.get("MAXI_LOOP_BASE_URL", "http://127.0.0.1:8080")
    admin_email = os.environ.get("MAXI_LOOP_ADMIN_EMAIL")
    admin_password = os.environ.get("MAXI_LOOP_ADMIN_PASSWORD")
    if not admin_email or not admin_password:
        raise SystemExit(
            "MAXI_LOOP_ADMIN_EMAIL and MAXI_LOOP_ADMIN_PASSWORD environment variables are required"
        )

    chrome_bin = find_chrome_binary()
    suffix = secrets.token_hex(4)
    release_version = f"react-e2e-{suffix}"
    pii_secret = f"ephemeral_react_pii_{secrets.token_hex(8)}"
    span_secret = f"ephemeral_react_span_{secrets.token_hex(8)}"

    api = MaxiLoopApiClient(base_url)
    api.login(admin_email, admin_password)

    react_proj = api.create_project(
        f"e2e-react-{suffix}", f"E2E React ({suffix})", "javascript"
    )
    backend_proj = api.create_project(
        f"e2e-react-api-{suffix}", f"E2E React API ({suffix})", "python"
    )
    react_write_tok = api.create_api_token(
        react_proj["slug"], f"e2e-react-ci-{suffix}", can_write=True
    )["raw_token"]

    # 1. Verify live CORS preflight on `/api/{id}/envelope/` and non-CORS on `/api/v1/health`
    preflight_status, preflight_hdrs, _ = api.request_raw(
        "OPTIONS",
        f"/api/{react_proj['id']}/envelope/",
        headers={
            "Origin": "https://react-app.example.com",
            "Access-Control-Request-Method": "POST",
            "Access-Control-Request-Headers": "content-type, x-sentry-auth",
        },
    )
    assert preflight_status == 204, f"expected 204 CORS preflight, got {preflight_status}"
    assert preflight_hdrs.get("access-control-allow-origin") == "*", preflight_hdrs
    assert "x-sentry-auth" in preflight_hdrs.get("access-control-allow-headers", "").lower()
    assert "x-sentry-rate-limits" in preflight_hdrs.get(
        "access-control-expose-headers", ""
    ).lower()
    assert "access-control-allow-credentials" not in preflight_hdrs

    health_status, health_hdrs, _ = api.request_raw("GET", "/api/v1/health")
    assert health_status == 200
    assert "access-control-allow-origin" not in health_hdrs
    print("[PASS] Browser CORS verified on /api/{id}/envelope/ (204 preflight, non-credentialed) and absent on /api/v1/*")

    # 2. Upload real Vite-generated .js.map files via CI raw upload endpoint + write API token
    map_files = sorted((REACT_DIST / "assets").glob("*.js.map"))
    assert map_files, f"no .js.map files found under {REACT_DIST / 'assets'}"
    uploaded_names: list[str] = []
    for map_path in map_files:
        js_filename = f"assets/{map_path.name[:-4]}"
        api.upload_sourcemap_raw(
            react_proj["slug"],
            release_version,
            js_filename,
            map_path.read_bytes(),
            react_write_tok,
        )
        uploaded_names.append(js_filename)
    print(f"[PASS] Uploaded {len(uploaded_names)} Vite source map(s) via mlapi_ write token: {uploaded_names}")

    # 3. Start static React server + Django WSGI backend server
    static_server = StaticReactTestServer(REACT_DIST)
    backend_env = os.environ.copy()
    backend_env["SENTRY_DSN"] = backend_proj["dsn"]
    backend_env["E2E_FRONTEND_ORIGIN"] = static_server.origin
    backend_env["E2E_RELEASE"] = release_version
    backend_env["E2E_SPAN_SECRET"] = span_secret
    backend_proc = subprocess.Popen(
        [sys.executable, os.path.abspath(__file__), "--backend-worker"],
        env=backend_env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        assert backend_proc.stdout is not None
        port_line = backend_proc.stdout.readline().strip()
        if not port_line.startswith("BACKEND_PORT="):
            err_txt = backend_proc.stderr.read() if backend_proc.stderr else ""
            raise RuntimeError(f"Backend worker failed to start: {port_line!r}\n{err_txt}")
        backend_port = int(port_line.split("=", 1)[1])
        backend_url = f"http://127.0.0.1:{backend_port}"

        # 4. Run real Headless Chrome against the production-built React app
        browser_result = run_chrome_session(
            chrome_bin,
            static_server,
            {
                "dsn": react_proj["dsn"],
                "release": release_version,
                "environment": "e2e-react",
                "backendUrl": backend_url,
                "mode": "full",
                "piiSecret": pii_secret,
                "callbackUrl": f"{static_server.origin}/e2e-done",
            },
        )
        browser_trace_id = str(browser_result["browser_trace_id"])
        backend_trace_id = str(browser_result["backend_trace_id"])
        assert browser_trace_id and browser_trace_id == backend_trace_id, (
            f"browser/backend trace_id mismatch: {browser_result}"
        )

        # 5. Verify React issues + real Vite Source Map resolution + PII scrubbing
        def check_react_issues() -> list[dict[str, Any]] | None:
            resp = api.request_json(
                "GET",
                f"/api/v1/projects/{urllib.parse.quote(react_proj['slug'])}/issues?per_page=50",
                bearer_token=react_write_tok,
            )
            items = resp.get("items", [])
            total = sum(int(i.get("event_count", 0)) for i in items)
            if len(items) == 2 and total == 4:
                return items
            return None

        issues = poll_until("React grouped issues", check_react_issues)
        by_type = {i["exception_type"]: i for i in issues}
        assert set(by_type.keys()) == {"TypeError", "RangeError"}, by_type
        assert int(by_type["TypeError"]["event_count"]) == 3
        assert int(by_type["RangeError"]["event_count"]) == 1

        for exc_type, expected_snippet in (
            ("TypeError", "TypeError"),
            ("RangeError", "RangeError"),
        ):
            detail = api.request_json(
                "GET",
                f"/api/v1/issues/{by_type[exc_type]['id']}",
                bearer_token=react_write_tok,
            )
            frames = detail.get("stack_frames", [])
            resolved_checkout_frames = [
                f
                for f in frames
                if str(f.get("filename", "")).endswith("src/checkout.ts")
                and f.get("in_app") is True
            ]
            assert resolved_checkout_frames, (
                f"expected resolved src/checkout.ts frame for {exc_type}, got frames={frames}"
            )
            assert any(
                str(f.get("filename", "")).startswith("node_modules/") and f.get("in_app") is False
                for f in frames
            ), f"expected vendor node_modules/ frames marked in_app=False, got frames={frames}"
            ctx_lines = [str(f.get("context_line", "")) for f in resolved_checkout_frames]
            assert any(expected_snippet in line for line in ctx_lines), (
                f"expected '{expected_snippet}' in resolved context_line for {exc_type}, got {ctx_lines}"
            )
            assert any(len(f.get("pre_context") or []) > 0 for f in resolved_checkout_frames)
            assert any(len(f.get("post_context") or []) > 0 for f in resolved_checkout_frames)

            sample_frames = (
                (detail.get("sample_event") or {})
                .get("payload", {})
                .get("exception", {})
                .get("values", [{}])[0]
                .get("stacktrace", {})
                .get("frames", [])
            )
            assert any(
                str(f.get("filename", "")).endswith("src/checkout.ts")
                and (f.get("data") or {}).get("sourcemap_resolved") is True
                for f in sample_frames
            ), f"expected sourcemap_resolved=True in sample_event frames for {exc_type}"
            serialized = json.dumps(detail)
            assert pii_secret not in serialized, f"PII leak detected in React {exc_type} issue!"
            assert "[Filtered]" in serialized, f"expected [Filtered] PII marker in React {exc_type}"

        print(
            "[PASS] React @sentry/react errors: 4 minified exceptions -> 2 grouped issues "
            "(TypeError x3, RangeError x1), resolved to src/checkout.ts with context_line & [Filtered] PII"
        )

        # 6. Verify React performance histogram + Browser -> Backend distributed trace waterfall
        def check_react_txs() -> list[dict[str, Any]] | None:
            resp = api.request_json(
                "GET",
                f"/api/v1/performance/transactions?project_slug={urllib.parse.quote(react_proj['slug'])}",
                bearer_token=react_write_tok,
            )
            items = resp.get("items", [])
            if any(t["transaction_name"] == "GET /react/checkout" for t in items):
                return items
            return None

        react_txs = poll_until("React performance transaction", check_react_txs)
        checkout_tx = next(t for t in react_txs if t["transaction_name"] == "GET /react/checkout")
        assert float(checkout_tx["p50_ms"]) >= 500.0, checkout_tx

        def check_waterfall() -> dict[str, Any] | None:
            try:
                wf = api.request_json(
                    "GET",
                    f"/api/v1/performance/traces/{urllib.parse.quote(browser_trace_id)}",
                )
            except RuntimeError:
                return None
            seen_projects = {s["project_slug"] for s in wf.get("spans", [])}
            if {react_proj["slug"], backend_proj["slug"]}.issubset(seen_projects):
                return wf
            return None

        waterfall = poll_until(
            f"Browser -> Backend distributed waterfall {browser_trace_id}", check_waterfall
        )
        spans = waterfall["spans"]
        react_spans = [s for s in spans if s["project_slug"] == react_proj["slug"]]
        backend_spans = [s for s in spans if s["project_slug"] == backend_proj["slug"]]

        assert any(
            s["op"] == "ui.checkout.flow" and int(s["depth"]) == 0 for s in react_spans
        ), react_spans
        assert any(s["op"] == "http.client" for s in react_spans), react_spans
        backend_root = next(
            (s for s in backend_spans if s["op"] == "http.server" and s["is_transaction_root"]),
            None,
        )
        assert backend_root is not None, backend_spans
        assert int(backend_root["depth"]) >= 2, (
            f"expected backend root nested under browser http.client span (depth >= 2), got {backend_root}"
        )
        assert any(s["op"] == "db.sql.query" for s in backend_spans), backend_spans

        wf_str = json.dumps(waterfall)
        assert pii_secret not in wf_str and span_secret not in wf_str, "PII leak in distributed trace!"
        assert "[Filtered]" in wf_str, "expected [Filtered] PII marker in distributed trace"
        print(
            f"[PASS] Browser (React) -> Backend (Django) distributed waterfall ({browser_trace_id}): "
            f"{len(spans)} spans across 2 services (React={len(react_spans)}, Backend={len(backend_spans)}), "
            f"browser tx p50={float(checkout_tx['p50_ms']):.1f}ms, parent linkage & [Filtered] PII verified"
        )

        # 7. Negative check: tampered DSN in real Chrome ingests 0 issues and 0 transactions
        run_chrome_session(
            chrome_bin,
            static_server,
            {
                "dsn": build_tampered_dsn(react_proj["dsn"]),
                "release": release_version,
                "environment": "e2e-negative",
                "backendUrl": backend_url,
                "mode": "invalid-dsn",
                "piiSecret": pii_secret,
                "callbackUrl": f"{static_server.origin}/e2e-done",
            },
        )
        time.sleep(0.6)
        after_issues = api.request_json(
            "GET",
            f"/api/v1/projects/{urllib.parse.quote(react_proj['slug'])}/issues?per_page=50",
            bearer_token=react_write_tok,
        ).get("items", [])
        assert len(after_issues) == 2 and sum(int(i["event_count"]) for i in after_issues) == 4
        after_txs = api.request_json(
            "GET",
            f"/api/v1/performance/transactions?project_slug={urllib.parse.quote(react_proj['slug'])}",
            bearer_token=react_write_tok,
        ).get("items", [])
        assert not any(t["transaction_name"] == "GET /react/invalid-dsn" for t in after_txs)
        print("[PASS] Negative check (Chrome): invalid DSN rejected (0 issues & 0 transactions ingested)")
    finally:
        static_server.close()
        backend_proc.terminate()
        try:
            backend_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            backend_proc.kill()
            backend_proc.wait(timeout=5)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Official @sentry/react Headless Chrome E2E verifier for Maxi Loop"
    )
    parser.add_argument(
        "--backend-worker",
        action="store_true",
        help="Internal worker mode for isolated Django WSGI distributed trace backend",
    )
    args = parser.parse_args()
    if args.backend_worker:
        run_backend_worker()
    else:
        run_orchestrator()


if __name__ == "__main__":
    main()
