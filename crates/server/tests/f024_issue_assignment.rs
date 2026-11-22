//! F024: issue assignment over HTTP and MCP, eligibility rules and auto-unassign.

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
use maxi_loop_platform::db::{connect_db, ensure_upcoming_events_partitions, run_migrations};
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

async fn mcp(app: &Router, token: &str, id: i64, tool: &str, args: Value) -> Value {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": tool, "arguments": args }
            })
            .to_string(),
        ))
        .expect("mcp req");
    let resp = app.clone().oneshot(req).await.expect("mcp resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("mcp body");
    serde_json::from_slice(&bytes).expect("mcp json")
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
        "exception": {"values": [{
            "type": exc_type,
            "value": "assignment test",
            "stacktrace": {"frames": [{"filename": "app/x.py", "function": "f", "lineno": 1, "in_app": true}]}
        }]}
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

fn ids(list: &Value) -> Vec<i64> {
    list.as_array()
        .expect("array")
        .iter()
        .map(|m| m["user_id"].as_i64().expect("user_id"))
        .collect()
}

#[tokio::test]
async fn issue_assignment_http_mcp_and_auto_unassign() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect db");
    run_migrations(&pool).await.expect("migrations");
    ensure_upcoming_events_partitions(&pool, Utc::now().date_naive(), 2)
        .await
        .expect("partitions");

    let sfx = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tenant = format!("f024-{sfx}");
    let tenant_b = format!("f024b-{sfx}");
    let owner = format!("owner-{sfx}@example.com");
    let owner_b = format!("ownerb-{sfx}@example.com");
    let pw = "F024-Strong-Password-2026!";

    let outcome = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant.clone(),
            tenant_name: "F024".to_owned(),
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
    .expect("bootstrap A");
    let outcome_b = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_b.clone(),
            tenant_name: "F024 B".to_owned(),
            retention_days: 30,
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: owner_b.clone(),
                password: pw.to_owned(),
                name: "Owner B".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap B");
    let owner_b_id = outcome_b.admin_user_id.expect("owner b id");

    let app = build_router(AppState::with_login_limiter(
        config.clone(),
        pool.clone(),
        LoginRateLimiter::new(200, Duration::from_secs(60), 1000),
    ));
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
    let (st, project) = send(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&owner_auth),
        Some(
            json!({"slug": "shop", "name": "Shop", "platform": "python", "workspace_slug": "core"}),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{project}");
    let project_id = project["id"].as_i64().unwrap();
    let workspace_id = project["workspace_id"].as_i64().unwrap();

    // Members: tenant member (eligible), tenant viewer (not), ops workspace admin (other
    // workspace: not), core workspace member (eligible).
    let mut members = std::collections::HashMap::new();
    for (key, role, ws) in [
        ("dev", "member", None),
        ("viewer", "viewer", None),
        ("opsadmin", "workspace_admin", Some("ops")),
        ("wsdev", "member", Some("core")),
    ] {
        let email = format!("{key}-{sfx}@example.com");
        let (st, m) = send(
            &app,
            Method::POST,
            "/api/v1/members",
            Some(&owner_auth),
            Some(json!({
                "email": email, "name": key, "password": pw, "role": role, "workspace_slug": ws
            })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{m}");
        members.insert(
            key,
            (
                email,
                m["user_id"].as_i64().unwrap(),
                m["membership_id"].as_i64().unwrap(),
            ),
        );
    }
    let owner_id = outcome.admin_user_id.expect("owner id");
    let dev_id = members["dev"].1;
    let wsdev_id = members["wsdev"].1;

    flush_queued_events(
        &pool,
        vec![
            queued(outcome.tenant_id, workspace_id, project_id, "AssignMeError"),
            queued(outcome.tenant_id, workspace_id, project_id, "OtherError"),
        ],
        10,
    )
    .await
    .expect("flush");
    let (st, list) = send(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=shop",
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let issue_id = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["exception_type"] == "AssignMeError")
        .expect("issue")["id"]
        .as_i64()
        .unwrap();
    assert!(list["items"][0]["assignee"].is_null());
    let assignee_uri = format!("/api/v1/issues/{issue_id}/assignee");
    let members_uri = format!("/api/v1/issues/{issue_id}/assignable-members");

    // --- Eligible assignees ---
    let (st, eligible) = send(&app, Method::GET, &members_uri, Some(&owner_auth), None).await;
    assert_eq!(st, StatusCode::OK);
    let eligible = ids(&eligible);
    for id in [owner_id, dev_id, wsdev_id] {
        assert!(eligible.contains(&id), "{id} should be assignable");
    }
    for id in [members["viewer"].1, members["opsadmin"].1, owner_b_id] {
        assert!(!eligible.contains(&id), "{id} must not be assignable");
    }

    // --- Viewers can neither list nor assign ---
    let viewer_auth = login(&app, &members["viewer"].0, pw, &tenant).await;
    let (st, _) = send(&app, Method::GET, &members_uri, Some(&viewer_auth), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = send(
        &app,
        Method::PATCH,
        &assignee_uri,
        Some(&viewer_auth),
        Some(json!({ "assignee_user_id": dev_id })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // --- Assign, no-op, invalid targets ---
    let (st, res) = send(
        &app,
        Method::PATCH,
        &assignee_uri,
        Some(&owner_auth),
        Some(json!({ "assignee_user_id": dev_id, "note": "You touched this last" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{res}");
    assert_eq!(res["changed"], true);
    assert_eq!(res["issue"]["assignee"]["user_id"], dev_id);
    let note = res["note"]["body"].as_str().unwrap();
    assert!(note.starts_with("Assigned to dev <") && note.ends_with("You touched this last"));

    let (_, res) = send(
        &app,
        Method::PATCH,
        &assignee_uri,
        Some(&owner_auth),
        Some(json!({ "assignee_user_id": dev_id })),
    )
    .await;
    assert_eq!(res["changed"], false);
    assert!(res["note"].is_null());

    for bad in [
        members["viewer"].1,
        members["opsadmin"].1,
        owner_b_id,
        999_999_999,
    ] {
        let (st, _) = send(
            &app,
            Method::PATCH,
            &assignee_uri,
            Some(&owner_auth),
            Some(json!({ "assignee_user_id": bad })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "assignee {bad}");
    }

    // Tenant B cannot touch tenant A's issue.
    let owner_b_auth = login(&app, &owner_b, pw, &tenant_b).await;
    let (st, _) = send(
        &app,
        Method::PATCH,
        &assignee_uri,
        Some(&owner_b_auth),
        Some(json!({ "assignee_user_id": owner_b_id })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // --- assigned_to filters ---
    let dev_auth = login(&app, &members["dev"].0, pw, &tenant).await;
    let count = |v: &Value| v["total"].as_i64().unwrap();
    let (_, mine) = send(
        &app,
        Method::GET,
        "/api/v1/issues?assigned_to=me",
        Some(&dev_auth),
        None,
    )
    .await;
    assert_eq!(count(&mine), 1);
    assert_eq!(mine["items"][0]["id"], issue_id);
    let (_, owner_mine) = send(
        &app,
        Method::GET,
        "/api/v1/issues?assigned_to=me",
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(count(&owner_mine), 0);
    let (_, unassigned) = send(
        &app,
        Method::GET,
        "/api/v1/issues?assigned_to=none&project_slug=shop",
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(count(&unassigned), 1);
    let (_, by_email) = send(
        &app,
        Method::GET,
        &format!("/api/v1/issues?assigned_to={}", members["dev"].0),
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(count(&by_email), 1);
    let (st, _) = send(
        &app,
        Method::GET,
        "/api/v1/issues?assigned_to=not%20valid",
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A member can reassign; the timeline records both changes.
    let (st, res) = send(
        &app,
        Method::PATCH,
        &assignee_uri,
        Some(&dev_auth),
        Some(json!({ "assignee_user_id": wsdev_id })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(res["issue"]["assignee"]["user_id"], wsdev_id);
    let (_, notes) = send(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{issue_id}/notes"),
        Some(&owner_auth),
        None,
    )
    .await;
    let bodies: Vec<&str> = notes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        bodies
            .iter()
            .filter(|b| b.starts_with("Assigned to"))
            .count(),
        2
    );

    // --- Auto-unassign when the membership goes away ---
    let (st, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/members/{}", members["wsdev"].2),
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, detail) = send(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{issue_id}"),
        Some(&owner_auth),
        None,
    )
    .await;
    assert!(detail["issue"]["assignee"].is_null(), "{}", detail["issue"]);

    // ... and when the role is downgraded to viewer.
    send(
        &app,
        Method::PATCH,
        &assignee_uri,
        Some(&owner_auth),
        Some(json!({ "assignee_user_id": dev_id })),
    )
    .await;
    let (st, _) = send(
        &app,
        Method::PATCH,
        &format!("/api/v1/members/{}", members["dev"].2),
        Some(&owner_auth),
        Some(json!({ "role": "viewer" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, detail) = send(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{issue_id}"),
        Some(&owner_auth),
        None,
    )
    .await;
    assert!(detail["issue"]["assignee"].is_null());

    // --- MCP ---
    let (st, tok) = send(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        Some(&owner_auth),
        Some(json!({ "name": "Triage agent", "access_scope": "tenant", "can_triage": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{tok}");
    let triage_tok = tok["raw_token"].as_str().unwrap().to_owned();
    let (_, tok) = send(
        &app,
        Method::POST,
        "/api/v1/mcp/tokens",
        Some(&owner_auth),
        Some(json!({ "name": "Reader", "access_scope": "tenant" })),
    )
    .await;
    let read_tok = tok["raw_token"].as_str().unwrap().to_owned();

    let res = mcp(
        &app,
        &triage_tok,
        1,
        "list_assignable_members",
        json!({ "issue_id": issue_id }),
    )
    .await;
    assert_eq!(res["result"]["isError"], false, "{res}");
    let emails: Vec<&str> = res["result"]["structuredContent"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["email"].as_str().unwrap())
        .collect();
    assert!(emails.contains(&owner.as_str()));
    assert!(!emails.contains(&members["viewer"].0.as_str()));

    let res = mcp(
        &app,
        &triage_tok,
        2,
        "assign_issue",
        json!({ "issue_id": issue_id, "assignee": "me", "note": "Agent will fix" }),
    )
    .await;
    assert_eq!(res["result"]["isError"], false, "{res}");
    let sc = &res["result"]["structuredContent"];
    assert_eq!(sc["assignee"]["email"], owner.as_str());
    assert_eq!(sc["note"]["source"], "mcp");

    let res = mcp(
        &app,
        &triage_tok,
        3,
        "list_issues",
        json!({ "assigned_to": "me" }),
    )
    .await;
    let items = res["result"]["structuredContent"]["items"]
        .as_array()
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["assignee"]["email"], owner.as_str());

    let res = mcp(
        &app,
        &triage_tok,
        4,
        "assign_issue",
        json!({ "issue_id": issue_id, "assignee": members["viewer"].0 }),
    )
    .await;
    assert_eq!(res["result"]["isError"], true);

    let res = mcp(
        &app,
        &triage_tok,
        5,
        "assign_issue",
        json!({ "issue_id": issue_id, "assignee": null }),
    )
    .await;
    assert_eq!(res["result"]["isError"], false);
    assert!(res["result"]["structuredContent"]["assignee"].is_null());

    // Read-only tokens cannot assign.
    let res = mcp(
        &app,
        &read_tok,
        6,
        "assign_issue",
        json!({ "issue_id": issue_id, "assignee": "me" }),
    )
    .await;
    assert!(
        res["result"]["isError"] == true || res.get("error").is_some(),
        "{res}"
    );
}
