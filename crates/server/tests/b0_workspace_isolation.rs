//! B0: users whose access is workspace-scoped must not reach other workspaces through
//! issues, performance, MCP tokens, alerts, PII rules, releases or source maps.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_ingest::model::QueuedEvent;
use maxi_loop_ingest::worker::flush_queued_events;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{
    connect_db, ensure_upcoming_events_partitions, ensure_upcoming_traces_partitions,
    run_migrations,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

fn load_test_config() -> Config {
    let _ = dotenvy::dotenv();
    let mut config = Config::from_env().expect("DATABASE_URL must be configured");
    config.cookie_secure = false;
    config.public_base_url = "http://127.0.0.1:8080".to_owned();
    config
}

type Auth = (String, String);

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    auth: Option<&Auth>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    if let Some((cookie, csrf)) = auth {
        builder = builder.header(header::COOKIE, cookie);
        if method != Method::GET {
            builder = builder.header("x-csrf-token", csrf);
        }
    }
    let req = match body {
        Some(v) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .expect("json req"),
        None => builder.body(Body::empty()).expect("empty req"),
    };
    let resp = app.clone().oneshot(req).await.expect("resp");
    let status = resp.status();
    let cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(ToOwned::to_owned);
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("bytes");
    let mut body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    if let Some(c) = cookie
        && body.is_object()
    {
        body["__cookie"] = Value::String(c);
    }
    (status, body)
}

async fn login(app: &Router, email: &str, pw: &str, tenant: &str) -> Auth {
    let (st, body) = send(
        app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({ "email": email, "password": pw, "tenant_slug": tenant })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "login {email}: {body}");
    (
        body["__cookie"].as_str().expect("cookie").to_owned(),
        body["csrf_token"].as_str().expect("csrf").to_owned(),
    )
}

async fn mcp_status(app: &Router, token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "list_issues", "arguments": {} }
            })
            .to_string(),
        ))
        .expect("mcp req");
    let resp = app.clone().oneshot(req).await.expect("mcp resp");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("mcp body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn queued(tenant_id: i64, workspace_id: i64, project_id: i64, exc_type: &str) -> QueuedEvent {
    let seed = format!(
        "{exc_type}-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let event_id = hex::encode(&Sha256::digest(seed.as_bytes())[..16]);
    let payload = json!({
        "event_id": event_id,
        "level": "error",
        "platform": "python",
        "exception": {"values": [{"type": exc_type, "value": "isolation"}]}
    });
    QueuedEvent {
        tenant_id,
        workspace_id,
        project_id,
        event_id,
        received_at: Utc::now(),
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: None,
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&payload).expect("payload"),
    }
}

#[tokio::test]
async fn workspace_scoped_users_cannot_reach_other_workspaces() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect db");
    run_migrations(&pool).await.expect("migrations");
    let today = Utc::now().date_naive();
    ensure_upcoming_events_partitions(&pool, today, 2)
        .await
        .expect("events partitions");
    ensure_upcoming_traces_partitions(&pool, today, 2)
        .await
        .expect("traces partitions");

    let sfx = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tenant = format!("b0-{sfx}");
    let owner = format!("b0-owner-{sfx}@example.com");
    let pw = "B0-Strong-Password-2026!";
    let outcome = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant.clone(),
            tenant_name: "B0".to_owned(),
            retention_days: 30,
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: owner.clone(),
                password: pw.to_owned(),
                name: "Owner".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap");

    let state = AppState::with_login_limiter(
        config.clone(),
        pool.clone(),
        LoginRateLimiter::new(200, Duration::from_secs(60), 1000),
    );
    let app = build_router(state.clone());
    let owner_auth = login(&app, &owner, pw, &tenant).await;

    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/workspaces",
        Some(&owner_auth),
        Some(json!({ "slug": "ops", "name": "Ops" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let mut projects = std::collections::HashMap::new();
    for (slug, ws) in [("shop", "core"), ("opsapi", "ops")] {
        let (st, p) = send(
            &app,
            Method::POST,
            "/api/v1/projects",
            Some(&owner_auth),
            Some(json!({"slug": slug, "name": slug, "platform": "python", "workspace_slug": ws})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{p}");
        projects.insert(slug, p);
    }
    state
        .dsn_cache
        .reload_from_db(&pool)
        .await
        .expect("dsn reload");
    let pid = |s: &str| projects[s]["id"].as_i64().unwrap();
    let wid = |s: &str| projects[s]["workspace_id"].as_i64().unwrap();

    // Workspace-scoped users of `core`.
    for (key, role) in [("coremem", "member"), ("coreadmin", "workspace_admin")] {
        let (st, m) = send(
            &app,
            Method::POST,
            "/api/v1/members",
            Some(&owner_auth),
            Some(json!({
                "email": format!("{key}-{sfx}@example.com"), "name": key, "password": pw,
                "role": role, "workspace_slug": "core"
            })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{m}");
    }
    let member = login(&app, &format!("coremem-{sfx}@example.com"), pw, &tenant).await;
    let wadmin = login(&app, &format!("coreadmin-{sfx}@example.com"), pw, &tenant).await;

    // --- Issues: lists are filtered in SQL, single-issue reads are checked ---
    flush_queued_events(
        &pool,
        vec![
            queued(outcome.tenant_id, wid("shop"), pid("shop"), "ShopError"),
            queued(outcome.tenant_id, wid("opsapi"), pid("opsapi"), "OpsError"),
        ],
        10,
    )
    .await
    .expect("flush");
    let (_, all) = send(&app, Method::GET, "/api/v1/issues", Some(&owner_auth), None).await;
    let ops_issue = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["exception_type"] == "OpsError")
        .expect("ops issue")["id"]
        .as_i64()
        .unwrap();
    let (_, mine) = send(&app, Method::GET, "/api/v1/issues", Some(&member), None).await;
    assert_eq!(mine["total"], 1, "total must only count visible projects");
    assert_eq!(mine["items"][0]["project_slug"], "shop");
    for suffix in ["", "/stats", "/events", "/notes", "/assignable-members"] {
        let (st, _) = send(
            &app,
            Method::GET,
            &format!("/api/v1/issues/{ops_issue}{suffix}"),
            Some(&member),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "GET issue{suffix}");
    }

    // --- Performance: other workspace's transactions/traces are invisible ---
    let ops_key = projects["opsapi"]["dsn_public_key"].as_str().unwrap();
    let start = Utc::now().timestamp() as f64 - 10.0;
    let tx_payload = json!({
        "event_id": format!("{:032x}", sfx as u128),
        "type": "transaction",
        "transaction": "GET /ops",
        "start_timestamp": start,
        "timestamp": start + 0.9,
        "release": "abcdef1",
        "contexts": {"trace": {"trace_id": format!("{:032x}", (sfx as u128) + 1), "span_id": "1234567890abcdef", "op": "http.server", "status": "internal_error"}}
    });
    let env_body = format!(
        "{{\"event_id\":\"{}\"}}\n{{\"type\":\"transaction\"}}\n{tx_payload}\n",
        tx_payload["event_id"].as_str().unwrap()
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/{}/envelope/", pid("opsapi")))
                .header(
                    "x-sentry-auth",
                    format!("Sentry sentry_version=7, sentry_key={ops_key}"),
                )
                .body(Body::from(env_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut seen = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (_, body) = send(
            &app,
            Method::GET,
            "/api/v1/performance/transactions?hours=1",
            Some(&owner_auth),
            None,
        )
        .await;
        if body["items"].as_array().is_some_and(|a| !a.is_empty()) {
            seen = true;
            break;
        }
    }
    assert!(seen, "owner should see the ops transaction");
    let (st, body) = send(
        &app,
        Method::GET,
        "/api/v1/performance/transactions?hours=1",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 0, "{body}");
    let (_, traces) = send(
        &app,
        Method::GET,
        "/api/v1/performance/traces?hours=1",
        Some(&member),
        None,
    )
    .await;
    assert!(
        traces
            .as_array()
            .or_else(|| traces["items"].as_array())
            .is_none_or(|a| a.is_empty()),
        "{traces}"
    );
    let (st, _) = send(
        &app,
        Method::GET,
        "/api/v1/performance/transactions/detail?project_slug=opsapi&transaction_name=GET%20%2Fops&hours=1",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(
        &app,
        Method::GET,
        "/api/v1/performance/compare-releases?project_slug=opsapi&base_release=a&target_release=b",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // --- MCP tokens can never reach beyond their creator ---
    for (body, label) in [
        (json!({"name": "t", "access_scope": "tenant"}), "tenant"),
        (
            json!({"name": "w", "access_scope": "workspace", "workspace_slug": "ops"}),
            "other workspace",
        ),
        (
            json!({"name": "p", "access_scope": "project", "project_slug": "opsapi", "workspace_slug": "ops"}),
            "other project",
        ),
    ] {
        let (st, _) = send(
            &app,
            Method::POST,
            "/api/v1/mcp/tokens",
            Some(&member),
            Some(body),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "mcp token for {label}");
    }
    let (st, tok) = send(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        Some(&member),
        Some(json!({"name": "mine", "access_scope": "workspace", "workspace_slug": "core", "can_triage": true})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{tok}");
    let member_token = tok["raw_token"].as_str().unwrap().to_owned();
    let (st, res) = mcp_status(&app, &member_token).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(res["result"]["isError"], false, "{res}");

    // A workspace admin only sees their own MCP tokens.
    let (_, owner_tok) = send(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        Some(&owner_auth),
        Some(json!({"name": "owner tok", "access_scope": "tenant"})),
    )
    .await;
    let owner_token_id = owner_tok["token"]["id"].as_i64().unwrap();
    let (_, listed) = send(&app, Method::GET, "/api/v1/mcp/tokens", Some(&wadmin), None).await;
    assert_eq!(listed.as_array().unwrap().len(), 0, "{listed}");
    let (st, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/mcp/tokens/{owner_token_id}"),
        Some(&wadmin),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // --- Alerts: scope-based management ---
    let hook = json!({"url": "https://hooks.example.com/b0"});
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/channels",
        Some(&wadmin),
        Some(json!({"name": "t", "channel_type": "webhook", "config": hook})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "tenant-wide channel");
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/channels",
        Some(&wadmin),
        Some(json!({"name": "o", "channel_type": "webhook", "config": hook, "workspace_slug": "ops"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "other workspace channel");
    let (st, core_ch) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/channels",
        Some(&wadmin),
        Some(json!({"name": "c", "channel_type": "webhook", "config": hook, "workspace_slug": "core"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{core_ch}");
    let (_, tenant_ch) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/channels",
        Some(&owner_auth),
        Some(json!({"name": "tenant", "channel_type": "webhook", "config": hook})),
    )
    .await;
    let tenant_ch_id = tenant_ch["id"].as_i64().unwrap();
    let (st, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/alerts/channels/{tenant_ch_id}"),
        Some(&wadmin),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let rule = |extra: Value| {
        let mut r = json!({"name": "r", "channel_id": tenant_ch_id, "trigger_type": "new_issue"});
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
    };
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        Some(&wadmin),
        Some(rule(
            json!({"workspace_slug": "ops", "project_slug": "opsapi"}),
        )),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "rule in other workspace");
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        Some(&wadmin),
        Some(rule(json!({}))),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "tenant-wide rule");
    let (st, r) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        Some(&wadmin),
        Some(rule(json!({"workspace_slug": "core"}))),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{r}");
    let (_, ops_rule) = send(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        Some(&owner_auth),
        Some(rule(json!({"workspace_slug": "ops"}))),
    )
    .await;
    let (_, rules) = send(
        &app,
        Method::GET,
        "/api/v1/alerts/rules",
        Some(&wadmin),
        None,
    )
    .await;
    assert!(
        !rules
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == ops_rule["id"]),
        "other workspace's rule must be hidden"
    );
    let (st, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/alerts/rules/{}", ops_rule["id"]),
        Some(&wadmin),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // --- PII rules ---
    let pii = |extra: Value| {
        let mut r = json!({"name": "p", "rule_type": "field_name", "pattern": "ssn"});
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
    };
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        Some(&wadmin),
        Some(pii(json!({}))),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "tenant-wide pii rule");
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        Some(&wadmin),
        Some(pii(json!({"workspace_slug": "ops"}))),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "other workspace pii rule");
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        Some(&wadmin),
        Some(pii(json!({"workspace_slug": "core"}))),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // --- Releases and source maps: triage rights in the target project ---
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/releases",
        Some(&member),
        Some(json!({"project_slug": "opsapi", "workspace_slug": "ops", "version": "1.0.0"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "release in other workspace");
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/releases",
        Some(&member),
        Some(json!({"project_slug": "shop", "workspace_slug": "core", "version": "1.0.0"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let map =
        json!({"version": 3, "sources": ["a.js"], "names": [], "mappings": "AAAA"}).to_string();
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/sourcemaps",
        Some(&member),
        Some(json!({"project_slug": "opsapi", "workspace_slug": "ops", "release_version": "1.0.0", "filename": "a.js.map", "content": map})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "source map in other workspace");
    let (st, ops_map) = send(
        &app,
        Method::POST,
        "/api/v1/sourcemaps",
        Some(&owner_auth),
        Some(json!({"project_slug": "opsapi", "workspace_slug": "ops", "release_version": "1.0.0", "filename": "a.js.map", "content": map})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{ops_map}");
    let (st, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/sourcemaps/{}", ops_map["id"]),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (_, maps) = send(
        &app,
        Method::GET,
        "/api/v1/sourcemaps",
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(
        maps.as_array().unwrap().len(),
        1,
        "forbidden delete rolled back"
    );

    // --- An MCP token dies with its owner's access ---
    let (_, members) = send(
        &app,
        Method::GET,
        "/api/v1/members",
        Some(&owner_auth),
        None,
    )
    .await;
    let coremem_membership = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["email"] == format!("coremem-{sfx}@example.com"))
        .unwrap()["membership_id"]
        .as_i64()
        .unwrap();
    let (st, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/members/{coremem_membership}"),
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = mcp_status(&app, &member_token).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}
