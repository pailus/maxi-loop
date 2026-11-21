use std::io::Write;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::{NaiveDate, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{
    connect_db, ensure_upcoming_events_partitions, ensure_upcoming_traces_partitions,
    run_migrations,
};
use maxi_loop_platform::tenant::begin_worker_tx;
use maxi_loop_retention::model::RetentionConfig;
use maxi_loop_retention::service::run_retention_cycle;
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

async fn send_transaction_envelope(
    app: &Router,
    project_id: i64,
    dsn_key: &str,
    tx_payload: Value,
    gzip: bool,
) -> StatusCode {
    let ev_id = tx_payload["event_id"]
        .as_str()
        .unwrap_or("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let raw = format!(
        "{{\"event_id\":\"{ev_id}\"}}\n{{\"type\":\"transaction\"}}\n{}\n",
        tx_payload
    );

    let (bytes, encoding): (Vec<u8>, Option<&str>) = if gzip {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(raw.as_bytes()).expect("gzip write");
        (enc.finish().expect("gzip finish"), Some("gzip"))
    } else {
        (raw.into_bytes(), None)
    };

    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/{project_id}/envelope/"))
        .header(
            "x-sentry-auth",
            format!("Sentry sentry_version=7, sentry_key={dsn_key}"),
        );
    if let Some(enc) = encoding {
        builder = builder.header(header::CONTENT_ENCODING, enc);
    }

    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(bytes)).expect("env req"))
        .await
        .expect("env resp");
    resp.status()
}

#[tokio::test]
async fn phase3_performance_histograms_zstd_traces_distributed_waterfall_mcp_and_isolation() {
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

    let run_id = Utc::now().timestamp_nanos_opt().unwrap_or(99) % 100_000_000;
    let tenant_a_slug = format!("perf-a-{run_id}");
    let tenant_b_slug = format!("perf-b-{run_id}");
    let email_a = format!("perf-a-{run_id}@example.com");
    let email_b = format!("perf-b-{run_id}@example.com");
    let password = "perf_test_password_12345";

    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_a_slug.clone(),
            tenant_name: "Perf Tenant A".to_owned(),
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            retention_days: 30,
            admin_user: Some(AdminUserSpec {
                email: email_a.clone(),
                name: "Alice Perf".to_owned(),
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
            tenant_name: "Perf Tenant B".to_owned(),
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            retention_days: 30,
            admin_user: Some(AdminUserSpec {
                email: email_b.clone(),
                name: "Bob Perf".to_owned(),
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
    let app = build_router(state.clone());

    let (cookie_a, csrf_a) = login_session(&app, &email_a, password, &tenant_a_slug).await;
    let (cookie_b, csrf_b) = login_session(&app, &email_b, password, &tenant_b_slug).await;

    // Create 2 projects in Tenant A (`web-frontend`, `orders-api`) and 1 in Tenant B (`beta-api`).
    let (st, proj_web) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_a,
        &csrf_a,
        Some(json!({ "slug": "web-frontend", "name": "Web Frontend", "platform": "javascript" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let web_id = proj_web["id"].as_i64().unwrap();
    let web_key = proj_web["dsn_public_key"].as_str().unwrap().to_owned();

    let (st, proj_api) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_a,
        &csrf_a,
        Some(json!({ "slug": "orders-api", "name": "Orders API", "platform": "python" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let api_id = proj_api["id"].as_i64().unwrap();
    let api_key = proj_api["dsn_public_key"].as_str().unwrap().to_owned();

    let (st, proj_b) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_b,
        &csrf_b,
        Some(json!({ "slug": "beta-api", "name": "Beta API", "platform": "go" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let beta_id = proj_b["id"].as_i64().unwrap();
    let beta_key = proj_b["dsn_public_key"].as_str().unwrap().to_owned();

    // 1. Send 10 `GET /api/orders` transactions on `orders-api` with varying latencies & 2 errors.
    //    Include baseline PII in request/span to verify zstd trace blob scrubbing.
    let base_epoch = Utc::now().timestamp() as f64 - 30.0;
    let durations_and_status: &[(f64, &str)] = &[
        (15.0, "ok"),
        (25.0, "ok"),
        (40.0, "ok"),
        (60.0, "ok"),
        (90.0, "ok"),
        (140.0, "ok"),
        (210.0, "ok"),
        (650.0, "ok"),                // slow -> sampled
        (880.0, "internal_error"),    // error -> sampled
        (120.0, "deadline_exceeded"), // error -> sampled
    ];

    for (idx, &(dur_ms, tx_status)) in durations_and_status.iter().enumerate() {
        let start = base_epoch + (idx as f64) * 0.5;
        let end = start + (dur_ms / 1000.0);
        let st = send_transaction_envelope(
            &app,
            api_id,
            &api_key,
            json!({
                "event_id": format!("ord{run_id:08}{idx:04}0000000000000000"),
                "type": "transaction",
                "transaction": "GET /api/orders",
                "start_timestamp": start,
                "timestamp": end,
                "environment": "production",
                "release": "deadbee",
                "platform": "python",
                "contexts": {
                    "trace": {
                        "trace_id": format!("{run_id:016x}{idx:016x}"),
                        "span_id": format!("{idx:016x}"),
                        "op": "http.server",
                        "status": tx_status
                    }
                },
                "request": {
                    "url": "https://api.example.com/api/orders?api_key=secret_q_key_999",
                    "headers": { "Authorization": "Bearer secret_tx_token_999" }
                },
                "spans": [
                    {
                        "span_id": format!("ab{idx:014x}"),
                        "parent_span_id": format!("{idx:016x}"),
                        "op": "db.sql.query",
                        "description": "SELECT * FROM orders WHERE tenant = $1",
                        "start_timestamp": start + 0.005,
                        "timestamp": end - 0.002,
                        "status": tx_status,
                        "data": { "password": "secret_db_pw_999", "custom_internal": "vault-internal-42" }
                    }
                ]
            }),
            idx % 2 == 0,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }

    // 2. Send a distributed 2-service trace (`web-frontend` -> `orders-api`) sharing one `trace_id`.
    let dist_trace_id = format!("d157{run_id:028x}");
    let t0 = Utc::now().timestamp() as f64 - 10.0;
    let st = send_transaction_envelope(
        &app,
        web_id,
        &web_key,
        json!({
            "event_id": format!("web{run_id:08}00000000000000000001"),
            "type": "transaction",
            "transaction": "/checkout",
            "start_timestamp": t0,
            "timestamp": t0 + 0.800,
            "environment": "production",
            "release": "web@3.0.0",
            "platform": "javascript",
            "contexts": {
                "trace": {
                    "trace_id": dist_trace_id,
                    "span_id": "1000000000000001",
                    "op": "pageload",
                    "status": "ok"
                }
            },
            "spans": [
                {
                    "span_id": "1000000000000002",
                    "parent_span_id": "1000000000000001",
                    "op": "http.client",
                    "description": "POST /api/checkout",
                    "start_timestamp": t0 + 0.050,
                    "timestamp": t0 + 0.720,
                    "status": "ok"
                }
            ]
        }),
        true,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let st = send_transaction_envelope(
        &app,
        api_id,
        &api_key,
        json!({
            "event_id": format!("api{run_id:08}00000000000000000002"),
            "type": "transaction",
            "transaction": "POST /api/checkout",
            "start_timestamp": t0 + 0.080,
            "timestamp": t0 + 0.700,
            "environment": "production",
            "release": "deadbee",
            "platform": "python",
            "contexts": {
                "trace": {
                    "trace_id": dist_trace_id,
                    "span_id": "2000000000000001",
                    "parent_span_id": "1000000000000002",
                    "op": "http.server",
                    "status": "ok"
                }
            },
            "spans": [
                {
                    "span_id": "2000000000000002",
                    "parent_span_id": "2000000000000001",
                    "op": "db.sql.query",
                    "description": "SELECT * FROM checkout_locks FOR UPDATE -- vault-internal-42",
                    "start_timestamp": t0 + 0.120,
                    "timestamp": t0 + 0.630,
                    "status": "ok"
                },
                {
                    "span_id": "2000000000000003",
                    "parent_span_id": "2000000000000001",
                    "op": "cache.get",
                    "description": "redis GET cart:42",
                    "start_timestamp": t0 + 0.090,
                    "timestamp": t0 + 0.095,
                    "status": "ok"
                }
            ]
        }),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 3. Send a transaction on Tenant B (`beta-api`).
    let beta_trace_id = format!("be7a{run_id:028x}");
    let st = send_transaction_envelope(
        &app,
        beta_id,
        &beta_key,
        json!({
            "event_id": format!("bet{run_id:08}00000000000000000001"),
            "type": "transaction",
            "transaction": "POST /tenant-b-secret",
            "start_timestamp": t0,
            "timestamp": t0 + 0.750,
            "environment": "production",
            "platform": "go",
            "contexts": {
                "trace": {
                    "trace_id": beta_trace_id,
                    "span_id": "9000000000000001",
                    "op": "http.server",
                    "status": "ok"
                }
            }
        }),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Wait for async ingest worker batch flush (poll up to 3s).
    let mut orders_tx = Value::Null;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (st, body) = send_authed_json(
            &app,
            Method::GET,
            "/api/v1/performance/transactions?project_slug=orders-api&hours=1",
            &cookie_a,
            "",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        if let Some(items) = body["items"].as_array()
            && let Some(found) = items
                .iter()
                .find(|it| it["transaction_name"] == "GET /api/orders" && it["total_count"] == 10)
        {
            orders_tx = found.clone();
            break;
        }
    }
    assert!(
        !orders_tx.is_null(),
        "expected GET /api/orders with total_count=10 after ingest worker flush"
    );

    // 4. Verify 1-minute duration histogram p50/p95/p99, throughput, and error rate.
    assert_eq!(orders_tx["total_count"], 10);
    assert_eq!(orders_tx["error_count"], 2);
    let err_rate = orders_tx["error_rate"].as_f64().unwrap();
    assert!((err_rate - 0.2).abs() < 0.01, "error_rate={err_rate}");
    assert!(orders_tx["throughput_rpm"].as_f64().unwrap() > 0.0);
    let p50 = orders_tx["p50_ms"].as_f64().unwrap();
    let p95 = orders_tx["p95_ms"].as_f64().unwrap();
    let p99 = orders_tx["p99_ms"].as_f64().unwrap();
    assert!(p50 <= p95 && p95 <= p99, "p50={p50} p95={p95} p99={p99}");
    assert!(p95 >= 500.0, "expected p95 >= 500ms, got {p95}");

    // 5. Verify transaction detail endpoint returns series + recent sampled traces.
    let (st, detail) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/transactions/detail?project_slug=orders-api&transaction_name=GET%20%2Fapi%2Forders&hours=1",
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(detail["summary"]["total_count"], 10);
    assert!(!detail["series"].as_array().unwrap().is_empty());
    let recent_traces = detail["recent_traces"].as_array().unwrap();
    assert!(
        recent_traces.len() >= 3 && recent_traces.len() < 10,
        "expected sampled traces (2 errors + 1 slow + ~5% random), got {}",
        recent_traces.len()
    );

    // 6. Verify zstd compression & PII scrubbing directly in PostgreSQL `traces.spans_zstd`.
    {
        let mut tx = begin_worker_tx(&pool).await.expect("worker tx");
        let blobs: Vec<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT sample_reason, spans_zstd FROM traces WHERE project_id = $1 AND transaction_name = 'GET /api/orders'",
        )
        .bind(api_id)
        .fetch_all(&mut *tx)
        .await
        .expect("select blobs");
        tx.commit().await.expect("commit");

        assert!(blobs.len() >= 3);
        let reasons: Vec<&str> = blobs.iter().map(|(r, _)| r.as_str()).collect();
        assert!(reasons.contains(&"slow"));
        assert!(reasons.contains(&"error"));

        for (_, zstd_bytes) in &blobs {
            // Standard zstd frame magic bytes: 0x28 0xB5 0x2F 0xFD
            assert!(
                zstd_bytes.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]),
                "expected zstd frame magic header"
            );
            let decoded =
                maxi_loop_performance::codec::decompress_trace_blob(zstd_bytes).expect("decode ok");
            let text = serde_json::to_string(&decoded).unwrap();
            assert!(!text.contains("secret_tx_token_999"));
            assert!(!text.contains("secret_db_pw_999"));
            assert!(!text.contains("secret_q_key_999"));
            assert!(text.contains("[Filtered]"));
        }
    }

    // 7. Verify distributed cross-service waterfall (`web-frontend` -> `orders-api`).
    let (st, wf) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/performance/traces/{dist_trace_id}"),
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(wf["services_count"], 2);
    assert_eq!(wf["span_count"], 5);
    assert_eq!(wf["projects"], json!(["orders-api", "web-frontend"]));
    let spans = wf["spans"].as_array().unwrap();
    assert_eq!(spans[0]["span_id"], "1000000000000001");
    assert_eq!(spans[0]["depth"], 0);
    assert_eq!(spans[1]["span_id"], "1000000000000002");
    assert_eq!(spans[1]["depth"], 1);
    assert_eq!(spans[2]["span_id"], "2000000000000001");
    assert_eq!(spans[2]["depth"], 2);
    // Slowest child span is the 510ms `db.sql.query` span (`2000000000000002`)
    let db_span = spans
        .iter()
        .find(|s| s["span_id"] == "2000000000000002")
        .expect("db span present");
    assert_eq!(db_span["depth"], 3);
    assert_eq!(db_span["is_slow"], true);
    assert!(!wf["slowest_spans"].as_array().unwrap().is_empty());

    // 8. Add a post-ingest custom PII rule on Tenant A so we verify MCP output PII scrubbing too!
    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Mask internal vault label",
            "rule_type": "value_substring",
            "pattern": "vault-internal-42",
            "replacement": "[VaultRedacted]"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 9. Mint tenant-scoped MCP token & project-scoped (`orders-api`) MCP token for Tenant A,
    //    plus a tenant-scoped MCP token for Tenant B.
    let (st, tok_a_tenant) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        &cookie_a,
        &csrf_a,
        Some(json!({ "name": "Perf Tenant Token", "access_scope": "tenant" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bearer_a_tenant = tok_a_tenant["raw_token"].as_str().unwrap().to_owned();

    let (st, tok_a_proj) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Perf Project Token",
            "access_scope": "project",
            "workspace_slug": "core",
            "project_slug": "orders-api"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bearer_a_proj = tok_a_proj["raw_token"].as_str().unwrap().to_owned();

    let (st, tok_b) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        &cookie_b,
        &csrf_b,
        Some(json!({ "name": "Perf B Token", "access_scope": "tenant" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bearer_b = tok_b["raw_token"].as_str().unwrap().to_owned();

    // 9a. MCP `list_transactions` returns p50/p95/p99, throughput, error_rate, untrusted wrapping & ui_url.
    let (st, mcp_txs) = send_mcp(
        &app,
        &bearer_a_tenant,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "list_transactions",
                "arguments": { "project_slug": "orders-api", "hours": 1, "sort": "p95" }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mcp_txs["result"]["isError"], false);
    let structured = &mcp_txs["result"]["structuredContent"];
    assert!(
        structured["security_notice"]
            .as_str()
            .unwrap()
            .contains("SECURITY NOTICE")
    );
    let mcp_items = structured["items"].as_array().unwrap();
    let mcp_orders = mcp_items
        .iter()
        .find(|i| {
            i["untrusted_event_data"]["transaction_name"]
                .as_str()
                .unwrap_or("")
                .contains("GET /api/orders")
        })
        .expect("GET /api/orders in MCP list_transactions");
    assert_eq!(mcp_orders["total_count"], 10);
    assert_eq!(mcp_orders["error_count"], 2);
    assert!(
        mcp_orders["untrusted_event_data"]["transaction_name"]
            .as_str()
            .unwrap()
            .contains("<<<UNTRUSTED_EVENT_DATA>>>")
    );
    assert!(
        mcp_orders["ui_url"]
            .as_str()
            .unwrap()
            .contains("view=performance")
    );

    // 9b. MCP `get_trace` returns compact distributed waterfall, slow spans flagged, custom PII scrubbed.
    let (st, mcp_trace) = send_mcp(
        &app,
        &bearer_a_tenant,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "get_trace",
                "arguments": { "trace_id": dist_trace_id }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mcp_trace["result"]["isError"], false);
    let trace_sc = &mcp_trace["result"]["structuredContent"];
    assert_eq!(trace_sc["trace"]["services_count"], 2);
    assert!(!trace_sc["slowest_spans"].as_array().unwrap().is_empty());
    let trace_str = trace_sc.to_string();
    assert!(!trace_str.contains("vault-internal-42"));
    assert!(trace_str.contains("[VaultRedacted]"));
    assert!(trace_str.contains("<<<UNTRUSTED_EVENT_DATA>>>"));

    // 9c. Project-scoped MCP token (`orders-api` only) cannot query `web-frontend`, and `get_trace`
    //     only exposes `orders-api` segments (`services_count == 1`).
    let (st, denied_txs) = send_mcp(
        &app,
        &bearer_a_proj,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "list_transactions",
                "arguments": { "project_slug": "web-frontend" }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(denied_txs["result"]["isError"], true);

    let (st, scoped_trace) = send_mcp(
        &app,
        &bearer_a_proj,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "get_trace",
                "arguments": { "trace_id": dist_trace_id }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(scoped_trace["result"]["isError"], false);
    assert_eq!(
        scoped_trace["result"]["structuredContent"]["trace"]["services_count"],
        1
    );
    assert_eq!(
        scoped_trace["result"]["structuredContent"]["trace"]["projects"],
        json!(["orders-api"])
    );

    // 10. Negative security tests: cross-tenant isolation (UI API + MCP) & invalid input validation.
    let (st, _) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/performance/traces/{beta_trace_id}"),
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/performance/traces/{dist_trace_id}"),
        &cookie_b,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, mcp_cross) = send_mcp(
        &app,
        &bearer_b,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "get_trace",
                "arguments": { "trace_id": dist_trace_id }
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mcp_cross["result"]["isError"], true);

    let (st, _) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/traces/not-valid-hex!",
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/performance/transactions?sort=drop_table",
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 11. Retention: verify expired daily `traces_YYYY_MM_DD` partition is dropped cleanly.
    let old_date = NaiveDate::from_ymd_opt(2025, 1, 10).unwrap();
    {
        let mut tx = begin_worker_tx(&pool).await.expect("worker tx");
        sqlx::query("SELECT ensure_traces_partition($1)")
            .bind(old_date)
            .execute(&mut *tx)
            .await
            .expect("create expired trace partition");
        tx.commit().await.expect("commit");
    }

    let report = run_retention_cycle(&pool, &RetentionConfig::from_config(&config), Utc::now())
        .await
        .expect("retention cycle ok");
    assert!(
        report
            .dropped_trace_partitions
            .contains(&"traces_2025_01_10".to_owned()),
        "expected traces_2025_01_10 to be dropped, got {:?}",
        report.dropped_trace_partitions
    );
}
