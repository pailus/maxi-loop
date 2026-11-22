#!/usr/bin/env python3
"""
Live E2E verification for Maxi Loop MCP Server:
  1. Streamable HTTP transport (`POST /mcp` & `DELETE /mcp`)
  2. Stdio bridge subcommand (`maxi-loop mcp` spawned with ZERO DATABASE_URL)

Creates an isolated temporary project + test error/transaction data + scoped MCP tokens
against a running Maxi Loop server (default http://127.0.0.1:8080), verifies both
transports end-to-end (including negative auth/scope/revocation checks), and cleans up
revoked tokens afterward.
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from http.cookiejar import CookieJar
from pathlib import Path


BASE_URL = os.environ.get("MAXI_LOOP_URL", "http://127.0.0.1:8080").rstrip("/")
REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BINARY = REPO_ROOT / "target" / "release" / "maxi-loop"


def bootstrap_ephemeral_tenant(binary: Path, suffix: str) -> tuple[str, str, str]:
    tenant_slug = f"mcp-e2e-{suffix}"
    email = f"mcp-e2e-{suffix}@example.com"
    password = f"E2eMcp!{secrets.token_hex(12)}"
    res = subprocess.run(
        [
            str(binary),
            "bootstrap",
            "--tenant-slug",
            tenant_slug,
            "--tenant-name",
            f"MCP E2E {suffix}",
            "--workspace-slug",
            "default",
            "--workspace-name",
            "Default",
            "--admin-email",
            email,
            "--admin-password",
            password,
            "--admin-name",
            "MCP E2E Admin",
        ],
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
        timeout=30,
    )
    if res.returncode != 0:
        raise RuntimeError(f"bootstrap failed ({res.returncode}): {res.stderr}")
    return tenant_slug, email, password


class ApiClient:
    def __init__(self, base_url: str) -> None:
        self.base_url = base_url
        self.cookies = CookieJar()
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(self.cookies)
        )
        self.csrf_token = ""

    def request(
        self,
        method: str,
        path: str,
        payload: dict | None = None,
        extra_headers: dict[str, str] | None = None,
    ) -> tuple[int, dict[str, str], bytes]:
        headers: dict[str, str] = {}
        data = None
        if payload is not None:
            data = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        if method != "GET" and self.csrf_token:
            headers["X-CSRF-Token"] = self.csrf_token
        if extra_headers:
            headers.update(extra_headers)
        req = urllib.request.Request(
            f"{self.base_url}{path}", data=data, headers=headers, method=method
        )
        try:
            with self.opener.open(req, timeout=15) as resp:
                return resp.status, dict(resp.headers.items()), resp.read()
        except urllib.error.HTTPError as err:
            return err.code, dict(err.headers.items()), err.read()

    def json_request(
        self, method: str, path: str, payload: dict | None = None
    ) -> tuple[int, dict]:
        status, _, body = self.request(method, path, payload)
        parsed = json.loads(body.decode("utf-8")) if body else {}
        return status, parsed

    def login(self, email: str, password: str, tenant_slug: str | None = None) -> None:
        body: dict[str, str] = {"email": email, "password": password}
        if tenant_slug:
            body["tenant_slug"] = tenant_slug
        status, data = self.json_request("POST", "/api/v1/auth/login", body)
        assert status == 200, f"login failed: HTTP {status} {data}"
        self.csrf_token = data["csrf_token"]


def mcp_http_post(
    token: str | None,
    payload: dict,
    session_id: str | None = None,
    protocol_version: str | None = None,
) -> tuple[int, dict[str, str], dict]:
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if token is not None:
        headers["Authorization"] = f"Bearer {token}"
    if session_id:
        headers["Mcp-Session-Id"] = session_id
    if protocol_version:
        headers["MCP-Protocol-Version"] = protocol_version

    req = urllib.request.Request(
        f"{BASE_URL}/mcp",
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            raw = resp.read()
            body = json.loads(raw.decode("utf-8")) if raw else {}
            hdrs = {k.lower(): v for k, v in resp.headers.items()}
            return resp.status, hdrs, body
    except urllib.error.HTTPError as err:
        raw = err.read()
        body = json.loads(raw.decode("utf-8")) if raw else {}
        hdrs = {k.lower(): v for k, v in err.headers.items()}
        return err.code, hdrs, body


def mcp_http_delete(token: str, session_id: str) -> int:
    req = urllib.request.Request(
        f"{BASE_URL}/mcp",
        headers={
            "Authorization": f"Bearer {token}",
            "Mcp-Session-Id": session_id,
        },
        method="DELETE",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status
    except urllib.error.HTTPError as err:
        return err.code


def ingest_envelope(project_id: int, dsn_key: str, item_type: str, payload: dict) -> None:
    item_bytes = json.dumps(payload).encode("utf-8")
    header_line = json.dumps({"event_id": payload["event_id"]}).encode("utf-8")
    item_header = json.dumps({"type": item_type, "length": len(item_bytes)}).encode(
        "utf-8"
    )
    envelope = header_line + b"\n" + item_header + b"\n" + item_bytes + b"\n"
    req = urllib.request.Request(
        f"{BASE_URL}/api/{project_id}/envelope/",
        data=envelope,
        headers={
            "Content-Type": "application/x-sentry-envelope",
            "X-Sentry-Auth": f"Sentry sentry_version=7, sentry_key={dsn_key}",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=15) as resp:
        assert resp.status == 200, f"ingest failed: HTTP {resp.status}"


def run_stdio_session(
    binary: Path,
    messages: list[dict],
    env_overrides: dict[str, str],
) -> tuple[list[dict], str, int]:
    # Deliberately strip DATABASE_URL so we prove `maxi-loop mcp` needs ZERO DB access.
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": os.environ.get("HOME", "/tmp"),
    }
    env.update(env_overrides)

    proc = subprocess.Popen(
        [str(binary), "mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        text=True,
    )
    assert proc.stdin and proc.stdout and proc.stderr
    input_blob = "".join(json.dumps(m) + "\n" for m in messages)
    stdout_text, stderr_text = proc.communicate(input=input_blob, timeout=20)
    responses: list[dict] = []
    for raw_line in stdout_text.splitlines():
        if not raw_line.strip():
            continue
        # Every non-empty stdout line MUST be valid JSON-RPC 2.0 (zero log pollution).
        parsed = json.loads(raw_line)
        responses.append(parsed)
    return responses, stderr_text, proc.returncode


def verify_with_claude_cli(
    label: str,
    mcp_config: dict,
    expected_project_slug: str,
    raw_token: str,
) -> None:
    claude_bin = shutil.which("claude")
    if not claude_bin:
        raise RuntimeError("claude CLI not found on PATH")

    with tempfile.NamedTemporaryFile(
        mode="w", suffix=".json", prefix="maxi-loop-mcp-", delete=False
    ) as tmp:
        json.dump(mcp_config, tmp)
        cfg_path = Path(tmp.name)

    try:
        os.chmod(cfg_path, 0o600)
        prompt = (
            "Call the Maxi Loop MCP tool `list_projects` and then `list_issues` "
            f'(with project_slug="{expected_project_slug}" and status="all"). '
            "Reply with ONLY a compact JSON object: "
            '{"project_slug": "<slug>", "issue_count": <number>, "first_title": "<title>"}'
        )
        res = subprocess.run(
            [
                claude_bin,
                "-p",
                prompt,
                "--mcp-config",
                str(cfg_path),
                "--strict-mcp-config",
                "--allowedTools",
                "mcp__maxi-loop__list_projects,mcp__maxi-loop__list_issues",
                "--output-format",
                "json",
                "--max-turns",
                "4",
            ],
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            timeout=90,
        )
        assert raw_token not in res.stdout and raw_token not in res.stderr, (
            f"{label}: raw MCP token appeared in claude output!"
        )
        assert res.returncode == 0, (
            f"{label}: claude exited {res.returncode}: stdout={res.stdout[:500]} stderr={res.stderr[:500]}"
        )
        parsed = json.loads(res.stdout)
        result_text = str(parsed.get("result", ""))
        assert expected_project_slug in result_text, (
            f"{label}: Claude response did not contain project slug '{expected_project_slug}': {result_text!r}"
        )
        assert "ZeroDivisionError" in result_text, (
            f"{label}: Claude response did not contain issue 'ZeroDivisionError': {result_text!r}"
        )
        print(f"[ok] Real Claude Code CLI ({label}) connected and executed MCP tools: {result_text.strip()}")
    finally:
        cfg_path.unlink(missing_ok=True)


def main() -> int:
    global BASE_URL
    parser = argparse.ArgumentParser(description="Verify Maxi Loop MCP Server (HTTP + stdio)")
    parser.add_argument("--base-url", default=BASE_URL)
    parser.add_argument("--email", default="")
    parser.add_argument("--password", default="")
    parser.add_argument("--tenant-slug", default="")
    parser.add_argument(
        "--test-claude",
        action="store_true",
        help="Also verify live MCP tool calls through real Claude Code CLI (HTTP + stdio)",
    )
    args = parser.parse_args()

    BASE_URL = args.base_url.rstrip("/")

    binary = Path(os.environ.get("MAXI_LOOP_BIN", str(DEFAULT_BINARY)))
    if not binary.exists():
        raise FileNotFoundError(
            f"Maxi Loop binary not found at {binary}; run `cargo build --release -p maxi-loop` first"
        )

    suffix = secrets.token_hex(4)
    if args.email and args.password:
        email = args.email
        password = args.password
        tenant_slug = args.tenant_slug or None
    else:
        tenant_slug, email, password = bootstrap_ephemeral_tenant(binary, suffix)
        print(f"[ok] bootstrapped isolated ephemeral tenant {tenant_slug}")

    api = ApiClient(BASE_URL)
    api.login(email, password, tenant_slug)

    proj_slug = f"mcp-e2e-{suffix}"
    secret_canary = f"sk_live_mcp_secret_{secrets.token_hex(6)}"

    created_token_ids: list[int] = []
    try:
        # 1. Create isolated test project + ingest 1 error event and 1 slow transaction.
        st, proj = api.json_request(
            "POST",
            "/api/v1/projects",
            {"name": f"MCP E2E {suffix}", "slug": proj_slug, "platform": "python"},
        )
        assert st == 201, f"create project failed: HTTP {st} {proj}"
        project_id = proj["id"]
        dsn_key = proj.get("dsn_key") or urllib.parse.urlparse(proj["dsn"]).username
        assert dsn_key, f"could not extract dsn key from project response: {proj}"
        print(f"[ok] created isolated project {proj_slug} (id={project_id})")

        event_id = uuid.uuid4().hex
        ingest_envelope(
            project_id,
            dsn_key,
            "event",
            {
                "event_id": event_id,
                "level": "error",
                "environment": "production",
                "release": "mcp-e2e@1.0.0",
                "exception": {
                    "values": [
                        {
                            "type": "ZeroDivisionError",
                            "value": "division by zero while charging order",
                            "stacktrace": {
                                "frames": [
                                    {
                                        "filename": "app/billing.py",
                                        "function": "charge_customer",
                                        "lineno": 42,
                                        "in_app": True,
                                        "context_line": "    return total / count",
                                    }
                                ]
                            },
                        }
                    ]
                },
                "extra": {
                    "password": secret_canary,
                    "api_key": secret_canary,
                },
                "request": {
                    "url": f"https://shop.example.com/checkout?token={secret_canary}",
                    "headers": {"Authorization": f"Bearer {secret_canary}"},
                },
            },
        )

        now = time.time()
        trace_id = uuid.uuid4().hex
        root_span_id = uuid.uuid4().hex[:16]
        db_span_id = uuid.uuid4().hex[:16]
        ingest_envelope(
            project_id,
            dsn_key,
            "transaction",
            {
                "event_id": uuid.uuid4().hex,
                "type": "transaction",
                "transaction": "POST /api/mcp-e2e/checkout",
                "environment": "production",
                "release": "mcp-e2e@1.0.0",
                "start_timestamp": now - 0.65,
                "timestamp": now,
                "contexts": {
                    "trace": {
                        "trace_id": trace_id,
                        "span_id": root_span_id,
                        "op": "http.server",
                        "status": "ok",
                    }
                },
                "spans": [
                    {
                        "span_id": db_span_id,
                        "parent_span_id": root_span_id,
                        "op": "db.sql.query",
                        "description": "SELECT * FROM orders WHERE user_id = 42 AND status = 'open'",
                        "start_timestamp": now - 0.55,
                        "timestamp": now - 0.05,
                        "status": "ok",
                    }
                ],
            },
        )

        # Wait for the 100ms batch ingest worker to flush.
        issue_id = None
        for _ in range(30):
            st, issues_res = api.json_request(
                "GET", f"/api/v1/issues?project_slug={proj_slug}"
            )
            if st == 200 and issues_res.get("items"):
                issue_id = issues_res["items"][0]["id"]
                break
            time.sleep(0.15)
        assert issue_id is not None, "ingested issue did not flush in time"
        print(f"[ok] ingested error (issue_id={issue_id}) and slow transaction (trace_id={trace_id})")

        # 2. Mint a READ-ONLY token and a TRIAGE token scoped strictly to `proj_slug`.
        st, read_tok = api.json_request(
            "POST",
            "/api/v1/mcp/tokens",
            {
                "name": f"e2e-read-{suffix}",
                "access_scope": "project",
                "project_slug": proj_slug,
                "can_triage": False,
                "rate_limit_per_minute": 120,
            },
        )
        assert st == 201, f"create read token failed: HTTP {st} {read_tok}"
        read_raw = read_tok["raw_token"]
        created_token_ids.append(read_tok["token"]["id"])

        st, triage_tok = api.json_request(
            "POST",
            "/api/v1/mcp/tokens",
            {
                "name": f"e2e-triage-{suffix}",
                "access_scope": "project",
                "project_slug": proj_slug,
                "can_triage": True,
                "rate_limit_per_minute": 120,
            },
        )
        assert st == 201, f"create triage token failed: HTTP {st} {triage_tok}"
        triage_raw = triage_tok["raw_token"]
        created_token_ids.append(triage_tok["token"]["id"])

        # 3. STREAMABLE HTTP E2E
        st, _, _ = mcp_http_post(None, {"jsonrpc": "2.0", "id": 1, "method": "ping"})
        assert st == 401, f"expected 401 unauth, got {st}"

        st, hdrs, init_res = mcp_http_post(
            read_raw,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "clientInfo": {"name": "verify-mcp-e2e", "version": "1.0"},
                    "capabilities": {},
                },
            },
        )
        assert st == 200, f"HTTP initialize failed: {st} {init_res}"
        session_id = hdrs.get("mcp-session-id")
        proto_hdr = hdrs.get("mcp-protocol-version")
        assert session_id, "missing Mcp-Session-Id response header"
        assert proto_hdr == "2025-06-18", f"unexpected MCP-Protocol-Version: {proto_hdr}"
        assert init_res["result"]["protocolVersion"] == "2025-06-18"

        st, _, _ = mcp_http_post(
            read_raw,
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            session_id=session_id,
            protocol_version="2025-06-18",
        )
        assert st == 202, f"expected 202 for notification, got {st}"

        st, _, tools_res = mcp_http_post(
            read_raw,
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            session_id=session_id,
            protocol_version="2025-06-18",
        )
        assert st == 200
        read_tools = [t["name"] for t in tools_res["result"]["tools"]]
        assert len(read_tools) == 9, f"expected 9 read tools, got {read_tools}"

        st, _, prompts_res = mcp_http_post(
            read_raw,
            {"jsonrpc": "2.0", "id": 3, "method": "prompts/list"},
            session_id=session_id,
            protocol_version="2025-06-18",
        )
        assert st == 200
        assert len(prompts_res["result"]["prompts"]) == 4

        st, _, issue_res = mcp_http_post(
            read_raw,
            {
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "get_issue", "arguments": {"issue_id": issue_id}},
            },
            session_id=session_id,
            protocol_version="2025-06-18",
        )
        assert st == 200 and issue_res["result"]["isError"] is False
        dumped_issue = json.dumps(issue_res)
        assert secret_canary not in dumped_issue, "PII secret leaked in MCP get_issue!"
        assert "<<<UNTRUSTED_EVENT_DATA>>>" in dumped_issue

        st, _, slow_res = mcp_http_post(
            read_raw,
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "list_slow_queries",
                    "arguments": {"project_slug": proj_slug},
                },
            },
            session_id=session_id,
            protocol_version="2025-06-18",
        )
        assert st == 200 and slow_res["result"]["isError"] is False
        assert "SELECT * FROM orders WHERE user_id = ? AND status = ?" in json.dumps(
            slow_res
        )

        st, _, trace_res = mcp_http_post(
            read_raw,
            {
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {"name": "get_trace", "arguments": {"trace_id": trace_id}},
            },
            session_id=session_id,
            protocol_version="2025-06-18",
        )
        assert st == 200 and trace_res["result"]["isError"] is False

        # Read-only token MUST be denied triage write.
        st, _, denied_res = mcp_http_post(
            read_raw,
            {
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "add_issue_note",
                    "arguments": {"issue_id": issue_id, "body": "should be denied"},
                },
            },
            session_id=session_id,
            protocol_version="2025-06-18",
        )
        assert st == 200 and denied_res["result"]["isError"] is True

        del_st = mcp_http_delete(read_raw, session_id)
        assert del_st == 204, f"expected 204 DELETE /mcp, got {del_st}"
        print("[ok] Streamable HTTP /mcp E2E passed (auth, headers, 9 tools, 4 prompts, PII scrub, deny, DELETE)")

        # 4. STDIO BRIDGE E2E (`maxi-loop mcp` with ZERO DATABASE_URL)
        stdio_msgs = [
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "clientInfo": {"name": "stdio-e2e", "version": "1.0"},
                    "capabilities": {},
                },
            },
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "ping"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 4, "method": "prompts/list"},
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "prompts/get",
                "params": {
                    "name": "triage_issue",
                    "arguments": {"issue_id": str(issue_id)},
                },
            },
            {
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {"name": "list_projects", "arguments": {}},
            },
            {
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "add_issue_note",
                    "arguments": {
                        "issue_id": issue_id,
                        "body": "Verified via maxi-loop mcp stdio bridge",
                    },
                },
            },
            {
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "update_issue_status",
                    "arguments": {"issue_id": issue_id, "status": "resolved"},
                },
            },
        ]
        responses, stderr_text, rc = run_stdio_session(
            binary,
            stdio_msgs,
            {
                "MAXI_LOOP_URL": BASE_URL,
                "MAXI_LOOP_MCP_TOKEN": triage_raw,
            },
        )
        assert rc == 0, f"maxi-loop mcp exited {rc}: stderr={stderr_text}"
        # 9 messages sent, 1 notification → MUST receive exactly 8 JSON-RPC responses on stdout!
        assert len(responses) == 8, f"expected 8 stdout replies, got {len(responses)}"
        by_id = {r["id"]: r for r in responses}
        assert by_id[1]["result"]["serverInfo"]["name"] == "maxi-loop"
        assert by_id[2]["result"] == {}
        assert len(by_id[3]["result"]["tools"]) == 11
        assert len(by_id[4]["result"]["prompts"]) == 4
        assert by_id[5]["result"]["messages"]
        assert by_id[6]["result"]["isError"] is False
        assert by_id[6]["result"]["structuredContent"]["projects"][0]["slug"] == proj_slug
        assert by_id[7]["result"]["isError"] is False
        assert by_id[8]["result"]["isError"] is False
        assert triage_raw not in stderr_text, "raw MCP token leaked to stderr!"
        print("[ok] Stdio bridge (`maxi-loop mcp`) E2E passed (zero DB env, 11 tools, 4 prompts, triage writes, clean EOF)")

        # 5. Negative stdio test: missing env vars must NOT crash and must report clear tool error.
        unconf_res, _, unconf_rc = run_stdio_session(
            binary,
            [
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
                {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {"name": "list_projects", "arguments": {}},
                },
            ],
            {},
        )
        assert unconf_rc == 0 and len(unconf_res) == 2
        assert unconf_res[1]["result"]["isError"] is True
        assert "MAXI_LOOP_URL" in unconf_res[1]["result"]["content"][0]["text"]

        # 5b. Optional: Real Claude Code CLI verification (both Streamable HTTP and Stdio Bridge!)
        if args.test_claude:
            verify_with_claude_cli(
                "Streamable HTTP",
                {
                    "mcpServers": {
                        "maxi-loop": {
                            "type": "http",
                            "url": f"{BASE_URL}/mcp",
                            "headers": {"Authorization": f"Bearer {read_raw}"},
                        }
                    }
                },
                proj_slug,
                read_raw,
            )
            verify_with_claude_cli(
                "Stdio Bridge (maxi-loop mcp)",
                {
                    "mcpServers": {
                        "maxi-loop": {
                            "command": str(binary),
                            "args": ["mcp"],
                            "env": {
                                "MAXI_LOOP_URL": BASE_URL,
                                "MAXI_LOOP_MCP_TOKEN": read_raw,
                            },
                        }
                    }
                },
                proj_slug,
                read_raw,
            )

        # 6. Revoke both tokens and verify immediate HTTP 401 + stdio auth failure.
        for tok_id in list(created_token_ids):
            st, _ = api.json_request("DELETE", f"/api/v1/mcp/tokens/{tok_id}")
            assert st in (200, 204), f"revoke token {tok_id} failed: HTTP {st}"
            created_token_ids.remove(tok_id)

        st, _, _ = mcp_http_post(read_raw, {"jsonrpc": "2.0", "id": 1, "method": "ping"})
        assert st == 401, f"revoked token still accepted over HTTP: {st}"

        rev_res, _, rev_rc = run_stdio_session(
            binary,
            [{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}],
            {"MAXI_LOOP_URL": BASE_URL, "MAXI_LOOP_MCP_TOKEN": triage_raw},
        )
        assert rev_rc == 0 and rev_res[0]["error"]["code"] == -32001
        assert triage_raw not in json.dumps(rev_res)
        print("[ok] Negative checks passed (missing env, revoked token 401 on HTTP & stdio, zero token leak)")

        print("ALL MCP E2E CHECKS PASSED")
        return 0
    finally:
        for tok_id in created_token_ids:
            try:
                api.json_request("DELETE", f"/api/v1/mcp/tokens/{tok_id}")
            except Exception:
                pass


if __name__ == "__main__":
    sys.exit(main())
