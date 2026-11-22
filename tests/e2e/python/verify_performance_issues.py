#!/usr/bin/env python3
"""End-to-end verification of Performance Issues (F017) with the official Python sentry-sdk.

A real Django app (sqlite ORM + DjangoIntegration, real WSGI handler, stdlib http.client) is
pointed at Maxi Loop by changing ONLY the DSN. It deliberately produces the span patterns that
Sentry turns into performance issues and checks that Maxi Loop:

1. Creates one issue per pattern with the right `issue_type`:
   N+1 DB, N+1 API calls, Slow DB Query, Consecutive DB, Consecutive HTTP.
2. Does NOT create a performance issue for a clean endpoint, and keeps errors as `error`.
3. Groups repeated occurrences into the same issue (event_count == runs).
4. Always keeps the offending trace (`sample_reason = 'issue'`) even for fast transactions,
   and links it from the issue's sample event.
5. Never stores raw SQL literals (an email in a WHERE clause) in the performance issue.
6. Filters with `issue_type=performance` / `issue_type=error`.
"""

from __future__ import annotations

import json
import os
import secrets
import subprocess
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from verify_sentry_sdk import MaxiLoopApiClient, _invoke_django_wsgi  # noqa: E402

RUNS = 2
LITERAL_EMAIL = "perf.secret.user@example.com"
EXPECTED = {
    "/n_plus_one_db/": "performance_n_plus_one_db",
    "/n_plus_one_literal/": "performance_n_plus_one_db",
    "/n_plus_one_api/": "performance_n_plus_one_api",
    "/slow_db/": "performance_slow_db_query",
    "/consecutive_db/": "performance_consecutive_db",
    "/consecutive_http/": "performance_consecutive_http",
}
# Count rows of a recursive CTE so sqlite really spends time inside the query span.
SLOW_SQL = (
    "WITH RECURSIVE t(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM t WHERE n < %d) "
    "SELECT count(*) FROM t"
)


def run_worker() -> None:
    import django
    from django.conf import settings

    settings.configure(
        DEBUG=False,
        SECRET_KEY="e2e-perf-issues",
        ALLOWED_HOSTS=["*"],
        ROOT_URLCONF=__name__,
        INSTALLED_APPS=["django.contrib.contenttypes"],
        DATABASES={"default": {"ENGINE": "django.db.backends.sqlite3", "NAME": ":memory:"}},
        MIDDLEWARE=[],
        LOGGING_CONFIG=None,
    )
    import sentry_sdk
    from sentry_sdk.integrations.django import DjangoIntegration

    sentry_sdk.init(
        dsn=os.environ["SENTRY_DSN"],
        integrations=[DjangoIntegration()],
        environment="e2e-perf",
        release="e2e-perf-1.0",
        traces_sample_rate=1.0,
    )
    django.setup()

    import http.client

    from django.core.wsgi import get_wsgi_application
    from django.db import connection, models
    from django.http import JsonResponse
    from django.urls import path

    class Author(models.Model):
        name = models.CharField(max_length=50)
        email = models.CharField(max_length=80, default="")

        class Meta:
            app_label = "contenttypes"

    class Book(models.Model):
        title = models.CharField(max_length=50)
        author = models.ForeignKey(Author, on_delete=models.CASCADE)

        class Meta:
            app_label = "contenttypes"

    with connection.schema_editor() as editor:
        editor.create_model(Author)
        editor.create_model(Book)
    for i in range(20):
        author = Author.objects.create(name=f"a{i}", email=f"user{i}@example.com")
        Book.objects.create(title=f"b{i}", author=author)

    # In-memory sqlite answers in microseconds; add a few ms per query so totals resemble a
    # networked database (and cross the N+1 duration threshold, as they would in production).
    def simulated_db_latency(execute, sql, params, many, context):
        time.sleep(0.005)
        return execute(sql, params, many, context)

    class Upstream(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            time.sleep(0.6 if self.path.startswith("/slow/") else 0.05)
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"{}")

        def log_message(self, *_args: Any) -> None:
            pass

    upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    port = upstream.server_address[1]

    def http_get(p: str) -> None:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        conn.request("GET", p)
        conn.getresponse().read()
        conn.close()

    def n_plus_one_db(_r):
        with connection.execute_wrapper(simulated_db_latency):
            names = [b.author.name for b in Book.objects.all()]
        return JsonResponse({"n": len(names)})

    def n_plus_one_literal(_r):
        with connection.execute_wrapper(simulated_db_latency), connection.cursor() as c:
            # 12 x ~5ms: comfortably above the 50ms N+1 duration threshold.
            for _ in range(12):
                c.execute(f"SELECT id FROM contenttypes_author WHERE email = '{LITERAL_EMAIL}'")
        return JsonResponse({})

    def n_plus_one_api(_r):
        workers = [threading.Thread(target=http_get, args=(f"/item/{i}",)) for i in range(6)]
        for w in workers:
            w.start()
        for w in workers:
            w.join()
        for i in range(6):
            http_get(f"/item/{100 + i}")
        return JsonResponse({})

    def slow_db(_r):
        with connection.cursor() as c:
            c.execute(SLOW_SQL % 3_000_000)
        return JsonResponse({})

    def consecutive_db(_r):
        with connection.cursor() as c:
            for limit in (300_000, 310_000, 320_000):
                c.execute(SLOW_SQL % limit)
        return JsonResponse({})

    def consecutive_http(_r):
        for name in ("inventory", "pricing", "shipping"):
            http_get(f"/slow/{name}")
        return JsonResponse({})

    def clean(_r):
        return JsonResponse({"n": Author.objects.count(), "b": Book.objects.count()})

    def crash(_r):
        raise RuntimeError("e2e perf-issues control error")

    global urlpatterns
    views = [n_plus_one_db, n_plus_one_literal, n_plus_one_api, slow_db, consecutive_db,
             consecutive_http, clean, crash]
    urlpatterns = [path(v.__name__ + "/", v) for v in views]

    app = get_wsgi_application()
    for _ in range(RUNS):
        for v in views:
            _invoke_django_wsgi(app, f"/{v.__name__}/", {})
    sentry_sdk.flush(timeout=15)
    upstream.shutdown()


def wait_for(predicate, what: str, timeout_secs: float = 20.0) -> Any:
    deadline = time.monotonic() + timeout_secs
    last: Any = None
    while time.monotonic() < deadline:
        ok, last = predicate()
        if ok:
            return last
        time.sleep(0.3)
    raise AssertionError(f"timed out waiting for {what}; last={last!r}")


def load_credentials() -> tuple[str, str]:
    email = os.environ.get("MAXI_LOOP_ADMIN_EMAIL")
    password = os.environ.get("MAXI_LOOP_ADMIN_PASSWORD")
    if email and password:
        return email, password
    raise SystemExit("set MAXI_LOOP_ADMIN_EMAIL and MAXI_LOOP_ADMIN_PASSWORD")


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "--worker":
        run_worker()
        return

    base_url = os.environ.get("MAXI_LOOP_BASE_URL", "http://127.0.0.1:8080")
    api = MaxiLoopApiClient(base_url)
    api.login(*load_credentials())
    slug = "e2e-perf-" + secrets.token_hex(3)
    project = api.create_project(slug, f"E2E perf issues {slug}")
    dsn = project["dsn"]

    env = dict(os.environ, SENTRY_DSN=dsn)
    proc = subprocess.run(
        [sys.executable, os.path.abspath(__file__), "--worker"],
        env=env, capture_output=True, text=True, timeout=180, check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"worker failed:\n{proc.stdout}\n{proc.stderr[-3000:]}")

    q = urllib.parse.quote

    def list_issues(issue_type: str | None = None) -> list[dict[str, Any]]:
        extra = f"&issue_type={q(issue_type)}" if issue_type else ""
        resp = api.request_json(
            "GET", f"/api/v1/projects/{q(slug)}/issues?per_page=50{extra}"
        )
        return list(resp.get("items", []))

    def all_ingested():
        items = list_issues()
        perf = [i for i in items if i["issue_type"] != "error"]
        done = len(perf) == len(EXPECTED) and all(i["event_count"] == RUNS for i in items)
        return done, [(i["issue_type"], i["culprit"], i["event_count"]) for i in items]

    wait_for(all_ingested, "all performance issues")
    issues = list_issues()
    by_culprit = {i["culprit"]: i for i in issues if i["issue_type"] != "error"}

    # 1 + 2: one issue per pattern, right type, nothing for the clean endpoint.
    for culprit, expected_type in EXPECTED.items():
        issue = by_culprit.get(culprit)
        assert issue, f"missing performance issue for {culprit}: {sorted(by_culprit)}"
        assert issue["issue_type"] == expected_type, (culprit, issue["issue_type"])
        assert issue["level"] == "warning", issue
        # 3: grouping across runs
        assert issue["event_count"] == RUNS, (culprit, issue["event_count"])
    assert "/clean/" not in by_culprit and "/crash/" not in by_culprit
    errors = [i for i in issues if i["issue_type"] == "error"]
    assert len(errors) == 1 and errors[0]["exception_type"] == "RuntimeError", errors
    print(f"[ok] {len(by_culprit)} performance issues with expected types; 1 error issue")

    # 6: issue_type filter
    assert len(list_issues("performance")) == len(EXPECTED)
    assert [i["id"] for i in list_issues("error")] == [errors[0]["id"]]
    assert len(list_issues("performance_n_plus_one_db")) == 2
    print("[ok] issue_type filter (performance / error / exact type)")

    # 4 + 5: sample event carries the problem, the trace is kept, literals are not stored.
    for culprit in ("/n_plus_one_db/", "/n_plus_one_literal/"):
        detail = api.get_issue_detail(by_culprit[culprit]["id"])
        pp = detail["sample_event"]["payload"]["performance_problem"]
        problem = pp["problem"]
        assert problem["offending_span_count"] >= 5, problem
        assert problem["offending_spans_total_ms"] >= 50, problem
        trace_id = pp["trace_id"]
        wf = api.get_trace_waterfall(trace_id)
        span_ids = {s.get("span_id") for s in wf.get("spans", [])}
        assert all(s["span_id"] in span_ids for s in problem["offending_spans"]), culprit

    literal_issue = by_culprit["/n_plus_one_literal/"]
    literal_detail = api.get_issue_detail(literal_issue["id"])
    blob = json.dumps(literal_detail) + json.dumps(literal_issue)
    assert LITERAL_EMAIL not in blob, "raw SQL literal leaked into performance issue"
    assert "email = ?" in literal_issue["title"], literal_issue["title"]
    print("[ok] offending spans resolvable in kept trace; SQL literals normalized away")

    traces = api.request_json(
        "GET",
        f"/api/v1/performance/traces?project_slug={q(slug)}&sample_reason=issue&per_page=50",
    )
    trace_items = traces if isinstance(traces, list) else traces.get("items", [])
    kept = {t["transaction_name"] for t in trace_items}
    assert "/n_plus_one_db/" in kept, kept
    assert "/clean/" not in kept, kept
    print(f"[ok] fast problem transactions kept with sample_reason=issue: {sorted(kept)}")

    print(f"PASS performance issues E2E (project {slug})")


if __name__ == "__main__":
    main()
