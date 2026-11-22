use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{
    connect_db, ensure_upcoming_events_partitions, ensure_upcoming_traces_partitions,
    run_migrations,
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn load_test_config() -> Config {
    let _ = dotenvy::dotenv();
    let mut config = Config::from_env().expect("DATABASE_URL must be configured");
    config.cookie_secure = false;
    config.public_base_url = "http://127.0.0.1:8080".to_owned();
    config.trace_slow_threshold_ms = 500;
    config.trace_random_sample_rate = 0.05;
    config
}

async fn login_session(
    app: &Router,
    email: &str,
    password: &str,
    tenant_slug: &str,
) -> (String, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "email": email,
                "password": password,
                "tenant_slug": tenant_slug,
            })
            .to_string(),
        ))
        .expect("login request");

    let resp = app.clone().oneshot(req).await.expect("login response");
    assert_eq!(resp.status(), StatusCode::OK);

    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie header")
        .to_str()
        .expect("cookie utf8")
        .split(';')
        .next()
        .expect("cookie kv")
        .to_owned();

    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .expect("login body");
    let parsed: Value = serde_json::from_slice(&body).expect("login json");
    let csrf = parsed["csrf_token"]
        .as_str()
        .expect("csrf_token")
        .to_owned();
    (set_cookie, csrf)
}

async fn send_authed_json(
    app: &Router,
    method: Method,
    uri: &str,
    cookie: &str,
    csrf: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method.clone())
        .uri(uri)
        .header(header::COOKIE, cookie);
    if method != Method::GET {
        builder = builder.header("x-csrf-token", csrf);
    }
    let req = match body {
        Some(v) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .expect("json req"),
        None => builder.body(Body::empty()).expect("empty req"),
    };

    let resp = app.clone().oneshot(req).await.expect("api response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("resp bytes");
    let json_val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json_val)
}

async fn send_bearer_get(app: &Router, uri: &str, bearer: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::empty())
        .expect("bearer req");
    let resp = app.clone().oneshot(req).await.expect("bearer resp");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("bearer bytes");
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

async fn send_mcp(app: &Router, bearer: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::from(body.to_string()))
        .expect("mcp req");

    let resp = app.clone().oneshot(req).await.expect("mcp resp");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("mcp bytes");
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

async fn send_envelope(
    app: &Router,
    project_id: i64,
    dsn_key: &str,
    item_type: &str,
    payload: Value,
) {
    let ev_id = payload["event_id"]
        .as_str()
        .expect("event_id in payload")
        .to_owned();
    let raw = format!(
        "{{\"event_id\":\"{ev_id}\"}}\n{{\"type\":\"{item_type}\"}}\n{}\n",
        payload
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/{project_id}/envelope/"))
        .header(
            "x-sentry-auth",
            format!("Sentry sentry_version=7, sentry_key={dsn_key}"),
        )
        .body(Body::from(raw))
        .expect("envelope req");

    let resp = app.clone().oneshot(req).await.expect("envelope resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn p2_slow_queries_compare_releases_mcp_tools_prompts_and_isolation() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect db");
    run_migrations(&pool).await.expect("run migrations");
    let today = Utc::now().date_naive();
    ensure_upcoming_events_partitions(&pool, today, 3)
        .await
        .expect("events partitions");
    ensure_upcoming_traces_partitions(&pool, today, 3)
        .await
        .expect("traces partitions");

    let run_id = Utc::now().timestamp_nanos_opt().unwrap_or(77) % 100_000_000;
    let tenant_a_slug = format!("p2a-{run_id}");
    let tenant_b_slug = format!("p2b-{run_id}");
    let email_a = format!("p2a-{run_id}@example.com");
    let email_b = format!("p2b-{run_id}@example.com");
    let password = "P2PerfMcpPassword2026!";

    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_a_slug.clone(),
            tenant_name: "P2 Tenant A".to_owned(),
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            retention_days: 30,
            admin_user: Some(AdminUserSpec {
                email: email_a.clone(),
                name: "Alice P2".to_owned(),
                password: password.to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant A");

    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_b_slug.clone(),
            tenant_name: "P2 Tenant B".to_owned(),
            workspace_slug: "core".to_owned(),
            workspace_name: "Core B".to_owned(),
            retention_days: 30,
            admin_user: Some(AdminUserSpec {
                email: email_b.clone(),
                name: "Bob P2".to_owned(),
                password: password.to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant B");

    let state =
        AppState::with_login_limiter(config.clone(), pool.clone(), LoginRateLimiter::default());
    state.dsn_cache.reload_from_db(&pool).await.expect("dsn");
    state
        .scrub_cache
        .reload_from_db(&pool)
        .await
        .expect("scrub");
    let app = build_router(state);

    let (cookie_a, csrf_a) = login_session(&app, &email_a, password, &tenant_a_slug).await;
    let (cookie_b, csrf_b) = login_session(&app, &email_b, password, &tenant_b_slug).await;

    // Create two projects in Tenant A (`checkout-api`, `billing-api`) and one in Tenant B (`secret-b`).
    let (st, proj_a1) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_a,
        &csrf_a,
        Some(json!({ "slug": "checkout-api", "name": "Checkout API", "platform": "python" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_a1_id = proj_a1["id"].as_i64().unwrap();
    let proj_a1_key = proj_a1["dsn_public_key"].as_str().unwrap().to_owned();

    let (st, _proj_a2) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_a,
        &csrf_a,
        Some(json!({ "slug": "billing-api", "name": "Billing API", "platform": "go" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, proj_b) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_b,
        &csrf_b,
        Some(json!({ "slug": "secret-b", "name": "Secret B", "platform": "python" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_b_id = proj_b["id"].as_i64().unwrap();
    let proj_b_key = proj_b["dsn_public_key"].as_str().unwrap().to_owned();

    // Register explicit commit SHAs for `v1.0.0` (base) and `v2.0.0` (target) on `checkout-api`.
    for (ver, sha) in [("v1.0.0", "1111111aaaa"), ("v2.0.0", "2222222bbbb")] {
        let (st, _) = send_authed_json(
            &app,
            Method::POST,
            "/api/v1/releases",
            &cookie_a,
            &csrf_a,
            Some(json!({
                "project_slug": "checkout-api",
                "version": ver,
                "commit_sha": sha,
                "environment": "production"
            })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
    }

    // 1. Ingest `v1.0.0` (fast baseline: 5x ~60ms transactions + fast DB query + HTTP client call).
    let base_epoch = Utc::now().timestamp() as f64 - 120.0;
    for i in 0..5 {
        let start = base_epoch + (i as f64) * 1.0;
        let tx_dur = 0.060;
        send_envelope(
            &app,
            proj_a1_id,
            &proj_a1_key,
            "transaction",
            json!({
                "event_id": format!("v1a{run_id:08}{i:04}00000000000000000"),
                "type": "transaction",
                "transaction": "POST /api/checkout",
                "start_timestamp": start,
                "timestamp": start + tx_dur,
                "environment": "production",
                "release": "v1.0.0",
                "platform": "python",
                "contexts": {
                    "trace": {
                        "trace_id": format!("1111{run_id:016x}{i:012x}"),
                        "span_id": format!("100000000000{i:04x}"),
                        "op": "http.server",
                        "status": "ok"
                    }
                },
                "spans": [
                    {
                        "span_id": format!("110000000000{i:04x}"),
                        "parent_span_id": format!("100000000000{i:04x}"),
                        "op": "db.sql.query",
                        "description": format!("SELECT * FROM orders WHERE user_id = {} AND status = 'open'", 100 + i),
                        "start_timestamp": start + 0.005,
                        "timestamp": start + 0.025,
                        "status": "ok"
                    },
                    {
                        "span_id": format!("120000000000{i:04x}"),
                        "parent_span_id": format!("100000000000{i:04x}"),
                        "op": "http.client",
                        "description": format!("POST https://stripe.internal/v1/charges/{}?api_key=leak_me", 9000 + i),
                        "start_timestamp": start + 0.026,
                        "timestamp": start + 0.050,
                        "status": "ok"
                    }
                ]
            }),
        )
        .await;
    }

    // 2. Ingest `v2.0.0` (regressed target: 5x ~620ms transactions with 1 error, slow DB query with different literals).
    let target_epoch = Utc::now().timestamp() as f64 - 40.0;
    for i in 0..5 {
        let start = target_epoch + (i as f64) * 1.0;
        let is_err = i == 4;
        let status = if is_err { "internal_error" } else { "ok" };
        send_envelope(
            &app,
            proj_a1_id,
            &proj_a1_key,
            "transaction",
            json!({
                "event_id": format!("v2a{run_id:08}{i:04}00000000000000000"),
                "type": "transaction",
                "transaction": "POST /api/checkout",
                "start_timestamp": start,
                "timestamp": start + 0.620,
                "environment": "production",
                "release": "v2.0.0",
                "platform": "python",
                "contexts": {
                    "trace": {
                        "trace_id": format!("2222{run_id:016x}{i:012x}"),
                        "span_id": format!("200000000000{i:04x}"),
                        "op": "http.server",
                        "status": status
                    }
                },
                "spans": [
                    {
                        "span_id": format!("210000000000{i:04x}"),
                        "parent_span_id": format!("200000000000{i:04x}"),
                        "op": "db.sql.query",
                        "description": format!("SELECT * FROM orders WHERE user_id = $1 AND status = 'pending' -- req {}", i),
                        "start_timestamp": start + 0.010,
                        "timestamp": start + 0.540,
                        "status": status
                    }
                ]
            }),
        )
        .await;
    }

    // Also ingest a newly introduced error issue in `v2.0.0` (absent in `v1.0.0`).
    send_envelope(
        &app,
        proj_a1_id,
        &proj_a1_key,
        "event",
        json!({
            "event_id": format!("err{run_id:08}000000000000000000001"),
            "level": "error",
            "platform": "python",
            "environment": "production",
            "release": "v2.0.0",
            "exception": {
                "values": [{
                    "type": "TimeoutError",
                    "value": "lock wait timeout on orders table (secret-token-xyz)"
                }]
            }
        }),
    )
    .await;

    // 3. Ingest a slow query in Tenant B to test strict cross-tenant RLS isolation.
    send_envelope(
        &app,
        proj_b_id,
        &proj_b_key,
        "transaction",
        json!({
            "event_id": format!("tnb{run_id:08}000000000000000000001"),
            "type": "transaction",
            "transaction": "GET /tenant-b-only",
            "start_timestamp": target_epoch,
            "timestamp": target_epoch + 0.800,
            "environment": "production",
            "release": "v2.0.0",
            "platform": "python",
            "contexts": {
                "trace": {
                    "trace_id": format!("bbbb{run_id:028x}"),
                    "span_id": "9000000000000001",
                    "op": "http.server",
                    "status": "ok"
                }
            },
            "spans": [
                {
                    "span_id": "9000000000000002",
                    "parent_span_id": "9000000000000001",
                    "op": "db.sql.query",
                    "description": "SELECT * FROM tenant_b_secret_ledger WHERE id = 777",
                    "start_timestamp": target_epoch + 0.010,
                    "timestamp": target_epoch + 0.700,
                    "status": "ok"
                }
            ]
        }),
    )
    .await;

    // Wait for async ingest worker batch flush (poll slow-operations until count == 10 across v1+v2).
    let mut normalized_db_op = Value::Null;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (st, body) = send_authed_json(
            &app,
            Method::GET,
            "/api/v1/performance/slow-operations?project_slug=checkout-api&db_only=true&hours=1",
            &cookie_a,
            "",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        if let Some(items) = body["items"].as_array()
            && let Some(found) = items.iter().find(|it| it["total_count"] == 10)
        {
            normalized_db_op = found.clone();
            break;
        }
    }
    assert!(
        !normalized_db_op.is_null(),
        "expected normalized SQL query with total_count=10 (5 from v1 + 5 from v2)"
    );

    // 4. Verify `F-PRF-7` Slow DB queries & operations normalization + aggregation.
    assert_eq!(normalized_db_op["op"], "db.sql.query");
    assert_eq!(
        normalized_db_op["normalized_description"],
        "SELECT * FROM orders WHERE user_id = ? AND status = ?"
    );
    assert_eq!(normalized_db_op["total_count"], 10);
    assert_eq!(normalized_db_op["error_count"], 1);
    assert!(normalized_db_op["p95_ms"].as_f64().unwrap() >= 450.0);
    assert!(normalized_db_op["total_ms"].as_f64().unwrap() >= 2500.0);
    assert!(
        normalized_db_op["sample_trace_id"]
            .as_str()
            .unwrap_or("")
            .starts_with("2222")
    );

    // With `db_only=false`, HTTP client spans also appear with normalized `:id` and stripped query params!
    let (st, all_ops) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/slow-operations?project_slug=checkout-api&db_only=false&hours=1",
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let http_op = all_ops["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|it| it["op"] == "http.client")
        .expect("normalized http.client op present");
    assert_eq!(
        http_op["normalized_description"],
        "POST https://stripe.internal/v1/charges/:id"
    );
    assert_eq!(http_op["total_count"], 5);
    assert!(!all_ops.to_string().contains("leak_me"));
    assert!(!all_ops.to_string().contains("tenant_b_secret_ledger"));

    // 5. Verify `F-PRF-6` Compare Releases HTTP API (`v1.0.0` vs `v2.0.0`).
    let (st, cmp) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/compare-releases?project_slug=checkout-api&base_release=v1.0.0&target_release=v2.0.0&hours=24",
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cmp["verdict"], "regressed");
    assert_eq!(cmp["base"]["commit_sha"], "1111111aaaa");
    assert_eq!(cmp["target"]["commit_sha"], "2222222bbbb");
    assert_eq!(cmp["base"]["performance"]["total_count"], 5);
    assert_eq!(cmp["target"]["performance"]["total_count"], 5);
    assert_eq!(cmp["target"]["performance"]["error_count"], 1);
    assert!(cmp["delta"]["p95_delta_ms"].as_f64().unwrap() >= 450.0);
    assert!(cmp["delta"]["error_rate_delta"].as_f64().unwrap() > 0.15);
    assert_eq!(cmp["base"]["issues"]["linked_issues_count"], 0);
    assert_eq!(cmp["target"]["issues"]["new_issues_count"], 1);
    assert_eq!(cmp["new_issues_in_target"].as_array().unwrap().len(), 1);
    let tx_rows = cmp["transactions"].as_array().unwrap();
    assert_eq!(tx_rows.len(), 1);
    assert_eq!(tx_rows[0]["transaction_name"], "POST /api/checkout");
    assert_eq!(tx_rows[0]["status"], "regressed");

    // Negative validation: comparing identical releases returns 400.
    let (st, _) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/compare-releases?project_slug=checkout-api&base_release=v1.0.0&target_release=v1.0.0",
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Cross-tenant isolation: Tenant B gets 404 comparing Tenant A's project, and sees 0 Tenant A queries.
    let (st, _) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/compare-releases?project_slug=checkout-api&base_release=v1.0.0&target_release=v2.0.0",
        &cookie_b,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, b_ops) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/slow-operations?db_only=true&hours=1",
        &cookie_b,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(b_ops.to_string().contains("tenant_b_secret_ledger"));
    assert!(!b_ops.to_string().contains("FROM orders"));

    // Per-project API token scoping: token scoped to `billing-api` is forbidden on `checkout-api`.
    let (st, api_tok_resp) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/api-tokens",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Billing Read Token",
            "project_slug": "billing-api",
            "can_write": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let billing_api_tok = api_tok_resp["raw_token"].as_str().unwrap().to_owned();
    let (st, _) = send_bearer_get(
        &app,
        "/api/v1/performance/compare-releases?project_slug=checkout-api&base_release=v1.0.0&target_release=v2.0.0",
        &billing_api_tok,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // 6. Add a post-ingest custom PII rule so MCP output scrubbing is verified on both new tools.
    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Scrub secret token substring",
            "rule_type": "value_substring",
            "pattern": "secret-token-xyz",
            "replacement": "[CustomScrubbed]"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 7. Mint Tenant A project-scoped (`checkout-api`) MCP token & Tenant B MCP token.
    let (st, tok_a) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Checkout P2 Agent",
            "access_scope": "project",
            "workspace_slug": "core",
            "project_slug": "checkout-api"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let mcp_a = tok_a["raw_token"].as_str().unwrap().to_owned();

    let (st, tok_b) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        &cookie_b,
        &csrf_b,
        Some(json!({ "name": "Tenant B Agent", "access_scope": "tenant" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let mcp_b = tok_b["raw_token"].as_str().unwrap().to_owned();

    // 7a. `initialize` advertises `prompts` capability.
    let (st, init_resp) = send_mcp(
        &app,
        &mcp_a,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(init_resp["result"]["capabilities"]["prompts"].is_object());

    // 7b. MCP `list_slow_queries` (`F-MCP-14`).
    let (st, mcp_slow) = send_mcp(
        &app,
        &mcp_a,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "list_slow_queries",
                "arguments": { "hours": 1, "sort": "p95" }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mcp_slow["result"]["isError"], false);
    let slow_sc = &mcp_slow["result"]["structuredContent"];
    assert_eq!(slow_sc["db_only"], true);
    let slow_items = slow_sc["items"].as_array().unwrap();
    assert_eq!(slow_items.len(), 1);
    assert_eq!(slow_items[0]["total_count"], 10);
    assert!(
        slow_items[0]["untrusted_event_data"]["normalized_description"]
            .as_str()
            .unwrap()
            .contains("<<<UNTRUSTED_EVENT_DATA>>>")
    );
    assert!(
        slow_items[0]["ui_url"]
            .as_str()
            .unwrap()
            .contains("tab=slow-queries")
    );

    // 7c. MCP `compare_releases` (`F-MCP-14`) — defaults `project_slug` for project-scoped token & scrubs custom PII!
    let (st, mcp_cmp) = send_mcp(
        &app,
        &mcp_a,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "compare_releases",
                "arguments": { "base_release": "v1.0.0", "target_release": "v2.0.0" }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mcp_cmp["result"]["isError"], false);
    let cmp_sc = &mcp_cmp["result"]["structuredContent"];
    assert_eq!(cmp_sc["verdict"], "regressed");
    assert_eq!(cmp_sc["project_slug"], "checkout-api");
    let cmp_dump = cmp_sc.to_string();
    assert!(!cmp_dump.contains("secret-token-xyz"));
    assert!(cmp_dump.contains("[CustomScrubbed]"));
    assert!(cmp_dump.contains("<<<UNTRUSTED_EVENT_DATA>>>"));

    // Project-scoped MCP token is denied when asking for sibling project `billing-api`.
    let (st, denied_cmp) = send_mcp(
        &app,
        &mcp_a,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "compare_releases",
                "arguments": {
                    "project_slug": "billing-api",
                    "base_release": "v1.0.0",
                    "target_release": "v2.0.0"
                }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(denied_cmp["result"]["isError"], true);

    // Cross-tenant MCP isolation: Tenant B cannot compare Tenant A's `checkout-api` or see its queries.
    let (st, b_cmp) = send_mcp(
        &app,
        &mcp_b,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "compare_releases",
                "arguments": {
                    "project_slug": "checkout-api",
                    "base_release": "v1.0.0",
                    "target_release": "v2.0.0"
                }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_cmp["result"]["isError"], true);

    // 7d. MCP Prompt Templates (`prompts/list` & `prompts/get`, `F-MCP-15`).
    let (st, prompts_list) = send_mcp(
        &app,
        &mcp_a,
        json!({ "jsonrpc": "2.0", "id": 6, "method": "prompts/list" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let prompt_names: Vec<&str> = prompts_list["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        prompt_names,
        vec![
            "triage_issue",
            "fix_issue",
            "triage_release_regression",
            "investigate_slow_queries"
        ]
    );

    let (st, prompt_fix) = send_mcp(
        &app,
        &mcp_a,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "prompts/get",
            "params": {
                "name": "fix_issue",
                "arguments": { "issue_id": "123" }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let prompt_text = prompt_fix["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(prompt_text.contains("issue #123"));
    assert!(prompt_text.contains("SECURITY NOTICE"));

    // Negative prompt tests: project-scoped token rejected for sibling project & invalid issue_id rejected.
    let (st, prompt_denied) = send_mcp(
        &app,
        &mcp_a,
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "prompts/get",
            "params": {
                "name": "triage_release_regression",
                "arguments": {
                    "project_slug": "billing-api",
                    "base_release": "v1.0.0",
                    "target_release": "v2.0.0"
                }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(prompt_denied["error"].is_object());

    let (st, prompt_bad_arg) = send_mcp(
        &app,
        &mcp_a,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "prompts/get",
            "params": {
                "name": "triage_issue",
                "arguments": { "issue_id": "not-a-number" }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(prompt_bad_arg["error"].is_object());
}
