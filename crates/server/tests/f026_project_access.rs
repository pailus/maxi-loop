//! F026 (Stage B): workspace member/viewer memberships and invitations limited to projects.

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
use maxi_loop_platform::tenant::begin_worker_tx;
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

fn session_from(body: &Value) -> Auth {
    (
        body["__cookie"].as_str().expect("cookie").to_owned(),
        body["csrf_token"].as_str().expect("csrf").to_owned(),
    )
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
    session_from(&body)
}

async fn visible_projects(app: &Router, auth: &Auth) -> Vec<String> {
    let (st, body) = send(app, Method::GET, "/api/v1/projects", Some(auth), None).await;
    assert_eq!(st, StatusCode::OK);
    let mut slugs: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["slug"].as_str().unwrap().to_owned())
        .collect();
    slugs.sort();
    slugs
}

fn queued(tenant_id: i64, workspace_id: i64, project_id: i64, exc_type: &str) -> QueuedEvent {
    let seed = format!(
        "{exc_type}-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let event_id = hex::encode(&Sha256::digest(seed.as_bytes())[..16]);
    let payload = json!({
        "event_id": event_id, "level": "error", "platform": "python",
        "exception": {"values": [{"type": exc_type, "value": "limits"}]}
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
async fn project_limited_memberships_and_invitations() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect db");
    run_migrations(&pool).await.expect("migrations");
    ensure_upcoming_events_partitions(&pool, Utc::now().date_naive(), 2)
        .await
        .expect("partitions");

    let sfx = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tenant = format!("f026-{sfx}");
    let owner = format!("f026-owner-{sfx}@example.com");
    let pw = "F026-Strong-Password-2026!";
    let outcome = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant.clone(),
            tenant_name: "F026".to_owned(),
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
    let app = build_router(AppState::with_login_limiter(
        config.clone(),
        pool.clone(),
        LoginRateLimiter::new(200, Duration::from_secs(60), 1000),
    ));
    let owner_auth = login(&app, &owner, pw, &tenant).await;

    send(
        &app,
        Method::POST,
        "/api/v1/workspaces",
        Some(&owner_auth),
        Some(json!({"slug": "ops", "name": "Ops"})),
    )
    .await;
    let mut p = std::collections::HashMap::new();
    for (slug, ws) in [("shop", "core"), ("blog", "core"), ("opsapi", "ops")] {
        let (st, proj) = send(
            &app,
            Method::POST,
            "/api/v1/projects",
            Some(&owner_auth),
            Some(json!({"slug": slug, "name": slug, "platform": "python", "workspace_slug": ws})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{proj}");
        p.insert(slug, proj);
    }
    let pid = |s: &str| p[s]["id"].as_i64().unwrap();
    let wid = |s: &str| p[s]["workspace_id"].as_i64().unwrap();

    // --- Direct creation with limits + validation ---
    let lim_email = format!("lim-{sfx}@example.com");
    let create = |email: &str, role: &str, ws: Option<&str>, ids: Vec<i64>| {
        json!({"email": email, "name": "Lim", "password": pw, "role": role,
               "workspace_slug": ws, "project_ids": ids})
    };
    for (body, label) in [
        (
            create(&lim_email, "member", Some("core"), vec![pid("opsapi")]),
            "project of another workspace",
        ),
        (
            create(
                &lim_email,
                "workspace_admin",
                Some("core"),
                vec![pid("shop")],
            ),
            "limited admin",
        ),
        (
            create(&lim_email, "member", None, vec![pid("shop")]),
            "limited tenant role",
        ),
    ] {
        let (st, _) = send(
            &app,
            Method::POST,
            "/api/v1/members",
            Some(&owner_auth),
            Some(body),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{label}");
    }
    let (st, lim) = send(
        &app,
        Method::POST,
        "/api/v1/members",
        Some(&owner_auth),
        Some(create(
            &lim_email,
            "member",
            Some("core"),
            vec![pid("shop")],
        )),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{lim}");
    assert_eq!(lim["project_limited"], true);
    assert_eq!(lim["projects"][0]["slug"], "shop");
    let lim_membership = lim["membership_id"].as_i64().unwrap();
    let lim_user = lim["user_id"].as_i64().unwrap();
    let lim_auth = login(&app, &lim_email, pw, &tenant).await;

    // --- Visibility follows the limits everywhere ---
    assert_eq!(visible_projects(&app, &lim_auth).await, vec!["shop"]);
    flush_queued_events(
        &pool,
        vec![
            queued(outcome.tenant_id, wid("shop"), pid("shop"), "ShopError"),
            queued(outcome.tenant_id, wid("blog"), pid("blog"), "BlogError"),
        ],
        10,
    )
    .await
    .expect("flush");
    let (_, all) = send(
        &app,
        Method::GET,
        "/api/v1/issues?workspace_slug=core",
        Some(&owner_auth),
        None,
    )
    .await;
    let issue_of = |t: &str| {
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["exception_type"] == t)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    let (shop_issue, blog_issue) = (issue_of("ShopError"), issue_of("BlogError"));
    let (_, mine) = send(&app, Method::GET, "/api/v1/issues", Some(&lim_auth), None).await;
    assert_eq!(mine["total"], 1);
    assert_eq!(mine["items"][0]["project_slug"], "shop");
    let (st, _) = send(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{blog_issue}"),
        Some(&lim_auth),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = send(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{blog_issue}/status"),
        Some(&lim_auth),
        Some(json!({"status": "resolved"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // --- Assignment eligibility follows the limits ---
    let assignable = |issue: i64| format!("/api/v1/issues/{issue}/assignable-members");
    let ids = |v: &Value| -> Vec<i64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|m| m["user_id"].as_i64().unwrap())
            .collect()
    };
    let (_, a) = send(
        &app,
        Method::GET,
        &assignable(shop_issue),
        Some(&owner_auth),
        None,
    )
    .await;
    assert!(ids(&a).contains(&lim_user));
    let (_, a) = send(
        &app,
        Method::GET,
        &assignable(blog_issue),
        Some(&owner_auth),
        None,
    )
    .await;
    assert!(!ids(&a).contains(&lim_user));
    let (st, _) = send(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{blog_issue}/assignee"),
        Some(&owner_auth),
        Some(json!({"assignee_user_id": lim_user})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{shop_issue}/assignee"),
        Some(&owner_auth),
        Some(json!({"assignee_user_id": lim_user})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // --- Editing limits ---
    let limits_uri = format!("/api/v1/members/{lim_membership}/projects");
    let (st, _) = send(
        &app,
        Method::PUT,
        &limits_uri,
        Some(&lim_auth),
        Some(json!({"project_ids": []})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "members cannot widen themselves");
    let (st, v) = send(
        &app,
        Method::PUT,
        &limits_uri,
        Some(&owner_auth),
        Some(json!({"project_ids": []})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["project_limited"], false);
    assert_eq!(
        visible_projects(&app, &lim_auth).await,
        vec!["blog", "shop"]
    );
    let (_, v) = send(
        &app,
        Method::PUT,
        &limits_uri,
        Some(&owner_auth),
        Some(json!({"project_ids": [pid("blog")]})),
    )
    .await;
    assert_eq!(v["projects"][0]["slug"], "blog");
    assert_eq!(visible_projects(&app, &lim_auth).await, vec!["blog"]);
    // Losing `shop` drops the shop assignment.
    let (_, d) = send(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{shop_issue}"),
        Some(&owner_auth),
        None,
    )
    .await;
    assert!(d["issue"]["assignee"].is_null(), "{}", d["issue"]);
    let (_, members) = send(
        &app,
        Method::GET,
        "/api/v1/members",
        Some(&owner_auth),
        None,
    )
    .await;
    let row = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["membership_id"] == lim_membership)
        .unwrap();
    assert_eq!(row["project_limited"], true);
    assert_eq!(row["projects"].as_array().unwrap().len(), 1);

    // --- MCP: no workspace-wide token for a limited member; project tokens only where allowed
    let mcp = |body: Value| async {
        send(
            &app,
            Method::POST,
            "/api/v1/mcp/tokens",
            Some(&lim_auth),
            Some(body),
        )
        .await
        .0
    };
    assert_eq!(
        mcp(json!({"name": "w", "access_scope": "workspace", "workspace_slug": "core"})).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        mcp(json!({"name": "s", "access_scope": "project", "workspace_slug": "core", "project_slug": "shop"})).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        mcp(json!({"name": "b", "access_scope": "project", "workspace_slug": "core", "project_slug": "blog"})).await,
        StatusCode::CREATED
    );

    // --- Deleting the last allowed project must not widen access to the whole workspace ---
    {
        let mut tx = begin_worker_tx(&pool).await.expect("worker tx");
        sqlx::query("DELETE FROM projects WHERE tenant_id = $1 AND id = $2")
            .bind(outcome.tenant_id)
            .bind(pid("blog"))
            .execute(&mut *tx)
            .await
            .expect("delete blog");
        tx.commit().await.expect("commit");
    }
    assert_eq!(
        visible_projects(&app, &lim_auth).await,
        Vec::<String>::new()
    );
    let (_, mine) = send(&app, Method::GET, "/api/v1/issues", Some(&lim_auth), None).await;
    assert_eq!(mine["total"], 0);

    // --- Promotion to workspace admin clears limits ---
    let (st, v) = send(
        &app,
        Method::PATCH,
        &format!("/api/v1/members/{lim_membership}"),
        Some(&owner_auth),
        Some(json!({"role": "workspace_admin"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["project_limited"], false);
    assert_eq!(visible_projects(&app, &lim_auth).await, vec!["shop"]);

    // --- Invitations carry the limits ---
    let inv_email = format!("inv-{sfx}@example.com");
    let (st, _) = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        Some(&owner_auth),
        Some(json!({"email": inv_email, "role": "viewer", "workspace_slug": "core", "project_ids": [pid("opsapi")]})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "invite with foreign project");
    let (st, created) = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        Some(&owner_auth),
        Some(json!({"email": inv_email, "role": "viewer", "workspace_slug": "core", "project_ids": [pid("shop")]})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert_eq!(created["invitation"]["project_limited"], true);
    let token = created["invite_url"]
        .as_str()
        .unwrap()
        .split("#invite=")
        .nth(1)
        .unwrap()
        .to_owned();
    let (_, listed) = send(
        &app,
        Method::GET,
        "/api/v1/invitations",
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(listed[0]["projects"][0]["slug"], "shop");
    let (_, preview) = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/preview",
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(preview["project_limited"], true);
    assert_eq!(preview["projects"][0]["slug"], "shop");
    let (st, accepted) = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({"token": token, "name": "Invitee", "password": pw})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{accepted}");
    let inv_auth = session_from(&accepted);
    assert_eq!(visible_projects(&app, &inv_auth).await, vec!["shop"]);
}
