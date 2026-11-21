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
    config
}

async fn login_session(
    app: &Router,
    email: &str,
    password: &str,
    tenant_slug: &str,
) -> (StatusCode, Option<(String, String)>) {
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
        .expect("login req");

    let resp = app.clone().oneshot(req).await.expect("login resp");
    let status = resp.status();
    if status != StatusCode::OK {
        return (status, None);
    }

    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("utf8")
        .split(';')
        .next()
        .expect("cookie kv")
        .to_owned();

    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .expect("body");
    let parsed: Value = serde_json::from_slice(&body).expect("json");
    let csrf = parsed["csrf_token"]
        .as_str()
        .expect("csrf_token")
        .to_owned();
    (status, Some((set_cookie, csrf)))
}

async fn send_cookie_json(
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

    let resp = app.clone().oneshot(req).await.expect("resp");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("bytes");
    let json_val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json_val)
}

async fn send_bearer_json(
    app: &Router,
    method: Method,
    uri: &str,
    bearer_token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer_token}"));
    let req = match body {
        Some(v) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .expect("json req"),
        None => builder.body(Body::empty()).expect("empty req"),
    };

    let resp = app.clone().oneshot(req).await.expect("resp");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("bytes");
    let json_val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json_val)
}

async fn send_error_envelope_with_headers(
    app: &Router,
    project_id: i64,
    dsn_key: &str,
    event_id: &str,
    message: &str,
) -> (StatusCode, Option<String>, Option<String>) {
    let payload = json!({
        "event_id": event_id,
        "level": "error",
        "environment": "production",
        "platform": "go",
        "message": message,
    });
    let raw = format!("{{\"event_id\":\"{event_id}\"}}\n{{\"type\":\"event\"}}\n{payload}\n");

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/{project_id}/envelope/"))
        .header(
            "x-sentry-auth",
            format!("Sentry sentry_version=7, sentry_key={dsn_key}"),
        )
        .body(Body::from(raw))
        .expect("env req");
    let resp = app.clone().oneshot(req).await.expect("env resp");
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    let sentry_limits = resp
        .headers()
        .get("x-sentry-rate-limits")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    (status, retry_after, sentry_limits)
}

#[tokio::test]
async fn phase5_multi_user_rbac_inherited_settings_api_tokens_and_aggregate_limits() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect db");
    run_migrations(&pool).await.expect("run migrations");
    let today = Utc::now().date_naive();
    ensure_upcoming_events_partitions(&pool, today, 2)
        .await
        .expect("events partitions");
    ensure_upcoming_traces_partitions(&pool, today, 2)
        .await
        .expect("traces partitions");

    let suffix = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tenant_a_slug = format!("p5-ta-{suffix}");
    let tenant_b_slug = format!("p5-tb-{suffix}");
    let owner_a_email = format!("owner-a-{suffix}@example.com");
    let owner_b_email = format!("owner-b-{suffix}@example.com");
    let pw = "Phase5-Strong-Password-2026!";

    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_a_slug.clone(),
            tenant_name: "Phase 5 Tenant A".to_owned(),
            retention_days: 30,
            workspace_slug: "default".to_owned(),
            workspace_name: "Default Workspace".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: owner_a_email.clone(),
                password: pw.to_owned(),
                name: "Owner A".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant A");

    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_b_slug.clone(),
            tenant_name: "Phase 5 Tenant B".to_owned(),
            retention_days: 30,
            workspace_slug: "default".to_owned(),
            workspace_name: "Default Workspace B".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: owner_b_email.clone(),
                password: pw.to_owned(),
                name: "Owner B".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant B");

    let state = AppState::with_login_limiter(
        config.clone(),
        pool.clone(),
        LoginRateLimiter::new(100, Duration::from_secs(60), 1000),
    );
    state
        .dsn_cache
        .reload_from_db(&pool)
        .await
        .expect("reload dsn cache");
    let app = build_router(state.clone());

    let (st, sess) = login_session(&app, &owner_a_email, pw, &tenant_a_slug).await;
    assert_eq!(st, StatusCode::OK);
    let (owner_cookie, owner_csrf) = sess.expect("owner A session");

    let (st, sess) = login_session(&app, &owner_b_email, pw, &tenant_b_slug).await;
    assert_eq!(st, StatusCode::OK);
    let (owner_b_cookie, owner_b_csrf) = sess.expect("owner B session");

    // Create a second workspace `ws-beta` in Tenant A to test workspace-scoped RBAC.
    let (st, ws_beta) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/workspaces",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "slug": "ws-beta",
            "name": "Workspace Beta",
            "retention_days_override": 20
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create ws-beta: {ws_beta}");

    // =========================================================================
    // 1. MULTI-USER RBAC (F-ADM-5): all 5 roles, workspace scoping, escalation guard, last-owner guard
    // =========================================================================
    let tadmin_email = format!("tadmin-{suffix}@example.com");
    let wsadmin_email = format!("wsadmin-{suffix}@example.com");
    let member_email = format!("member-{suffix}@example.com");
    let viewer_email = format!("viewer-{suffix}@example.com");

    let (st, tadmin_m) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/members",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "email": tadmin_email,
            "name": "Tenant Admin",
            "password": pw,
            "role": "tenant_admin"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create tenant_admin: {tadmin_m}");

    // Workspace admin ONLY on `ws-beta` (no access to `default` workspace!).
    let (st, wsadmin_m) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/members",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "email": wsadmin_email,
            "name": "Workspace Beta Admin",
            "password": pw,
            "role": "workspace_admin",
            "workspace_slug": "ws-beta"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create ws_admin: {wsadmin_m}");
    let wsadmin_membership_id = wsadmin_m["membership_id"].as_i64().unwrap();

    let (st, member_m) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/members",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "email": member_email,
            "name": "Member User",
            "password": pw,
            "role": "member"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create member: {member_m}");
    let member_membership_id = member_m["membership_id"].as_i64().unwrap();

    let (st, _) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/members",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "email": viewer_email,
            "name": "Viewer User",
            "password": pw,
            "role": "viewer"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // Owner cannot demote or remove the sole remaining `tenant_owner`.
    let (st, all_members) = send_cookie_json(
        &app,
        Method::GET,
        "/api/v1/members",
        &owner_cookie,
        &owner_csrf,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let owner_membership_id = all_members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["email"] == owner_a_email)
        .unwrap()["membership_id"]
        .as_i64()
        .unwrap();

    let (st, _) = send_cookie_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/members/{owner_membership_id}"),
        &owner_cookie,
        &owner_csrf,
        Some(json!({ "role": "tenant_admin" })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "cannot demote last tenant_owner"
    );

    let (st, _) = send_cookie_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/members/{owner_membership_id}"),
        &owner_cookie,
        &owner_csrf,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "cannot remove last tenant_owner"
    );

    // `tenant_admin` CANNOT grant `tenant_owner` or modify the `tenant_owner` (privilege escalation guard).
    let (_, sess) = login_session(&app, &tadmin_email, pw, &tenant_a_slug).await;
    let (tadmin_cookie, tadmin_csrf) = sess.expect("tadmin session");

    let (st, _) = send_cookie_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/members/{member_membership_id}"),
        &tadmin_cookie,
        &tadmin_csrf,
        Some(json!({ "role": "tenant_owner" })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "tenant_admin cannot escalate to tenant_owner"
    );

    let (st, _) = send_cookie_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/members/{owner_membership_id}"),
        &tadmin_cookie,
        &tadmin_csrf,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "tenant_admin cannot remove tenant_owner"
    );

    // `workspace_admin` of `ws-beta`:
    //   - CAN create a project inside `ws-beta` (`201`)
    //   - CANNOT create a project inside `default` workspace (`403`)
    //   - CANNOT update tenant settings (`403`)
    let (_, sess) = login_session(&app, &wsadmin_email, pw, &tenant_a_slug).await;
    let (wsadmin_cookie, wsadmin_csrf) = sess.expect("wsadmin session");

    let (st, _) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &wsadmin_cookie,
        &wsadmin_csrf,
        Some(json!({
            "slug": "beta-svc",
            "name": "Beta Service",
            "platform": "go",
            "workspace_slug": "ws-beta"
        })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "ws-beta admin creates project in ws-beta"
    );

    let (st, _) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &wsadmin_cookie,
        &wsadmin_csrf,
        Some(json!({
            "slug": "unauth-default-proj",
            "name": "Unauth Default Project",
            "platform": "go",
            "workspace_slug": "default"
        })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "ws-beta admin rejected in default workspace"
    );

    let (st, _) = send_cookie_json(
        &app,
        Method::PATCH,
        "/api/v1/tenants/current/settings",
        &wsadmin_cookie,
        &wsadmin_csrf,
        Some(json!({ "default_rate_limit_per_minute": 999 })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "ws_admin cannot update tenant settings"
    );

    // `member` CANNOT add members or mint project API tokens (`403`).
    let (_, sess) = login_session(&app, &member_email, pw, &tenant_a_slug).await;
    let (member_cookie, member_csrf) = sess.expect("member session");
    let (st, _) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/members",
        &member_cookie,
        &member_csrf,
        Some(json!({ "email": "x@example.com", "name": "X", "password": pw, "role": "viewer" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Removing `wsadmin`'s sole membership immediately invalidates their active session!
    let (st, _) = send_cookie_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/members/{wsadmin_membership_id}"),
        &owner_cookie,
        &owner_csrf,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _) = send_cookie_json(
        &app,
        Method::GET,
        "/api/v1/auth/me",
        &wsadmin_cookie,
        &wsadmin_csrf,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "removed member session immediately revoked"
    );

    // Cross-tenant isolation: Tenant B cannot see or mutate Tenant A's memberships.
    let (st, tb_members) = send_cookie_json(
        &app,
        Method::GET,
        "/api/v1/members",
        &owner_b_cookie,
        &owner_b_csrf,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!tb_members.to_string().contains(&member_email));
    let (st, _) = send_cookie_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/members/{member_membership_id}"),
        &owner_b_cookie,
        &owner_b_csrf,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "tenant B cannot delete tenant A membership"
    );

    // =========================================================================
    // 2. INHERITED SETTINGS (F-ADM-8) & AGGREGATE TENANT RATE LIMITS (F-ADM-9)
    // =========================================================================
    // Create two projects in Tenant A's `default` workspace with NO explicit rate limit (so they inherit).
    let (st, p1) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "slug": "go-orders",
            "name": "Go Orders",
            "platform": "go"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create p1: {p1}");
    assert_eq!(p1["effective_rate_limit_per_minute"], 1200);
    assert!(p1["rate_limit_per_minute_override"].is_null());
    let p1_id = p1["id"].as_i64().unwrap();
    let p1_key = p1["dsn_public_key"].as_str().unwrap().to_owned();

    let (st, p2) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "slug": "rust-billing",
            "name": "Rust Billing",
            "platform": "rust"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create p2: {p2}");
    let p2_id = p2["id"].as_i64().unwrap();
    let p2_key = p2["dsn_public_key"].as_str().unwrap().to_owned();

    // Create a Tenant B project to prove Tenant A's aggregate limit does not affect Tenant B.
    let (st, pb) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &owner_b_cookie,
        &owner_b_csrf,
        Some(json!({
            "slug": "tb-svc",
            "name": "Tenant B Svc",
            "platform": "go"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let pb_id = pb["id"].as_i64().unwrap();
    let pb_key = pb["dsn_public_key"].as_str().unwrap().to_owned();

    // Step 2a: Update Tenant A default rate limit to 500 and aggregate cap to 5/min.
    let (st, t_upd) = send_cookie_json(
        &app,
        Method::PATCH,
        "/api/v1/tenants/current/settings",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "retention_days": 21,
            "default_rate_limit_per_minute": 500,
            "aggregate_rate_limit_per_minute": 5
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "update tenant settings: {t_upd}");
    assert_eq!(t_upd["tenant"]["retention_days"], 21);
    assert_eq!(t_upd["tenant"]["default_rate_limit_per_minute"], 500);
    assert_eq!(t_upd["tenant"]["aggregate_rate_limit_per_minute"], 5);

    // Verify p1 now inherits 21d retention and 500/min rate limit!
    let (st, p1_check) = send_cookie_json(
        &app,
        Method::GET,
        "/api/v1/projects/go-orders",
        &owner_cookie,
        &owner_csrf,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p1_check["effective_retention_days"], 21);
    assert_eq!(p1_check["effective_rate_limit_per_minute"], 500);

    // Step 2b: Override default workspace to 14d retention and 100/min rate limit.
    let (st, ws_upd) = send_cookie_json(
        &app,
        Method::PATCH,
        "/api/v1/workspaces/default/settings",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "retention_days_override": 14,
            "default_rate_limit_per_minute_override": 100
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "update ws settings: {ws_upd}");
    assert_eq!(ws_upd["effective_retention_days"], 14);
    assert_eq!(ws_upd["effective_rate_limit_per_minute"], 100);

    let (st, p1_check2) = send_cookie_json(
        &app,
        Method::GET,
        "/api/v1/projects/go-orders",
        &owner_cookie,
        &owner_csrf,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p1_check2["effective_retention_days"], 14);
    assert_eq!(p1_check2["effective_rate_limit_per_minute"], 100);

    // Step 2c: Override p1 explicitly to 7d retention and 10/min rate limit, then test reset to null.
    let (st, p1_upd) = send_cookie_json(
        &app,
        Method::PATCH,
        "/api/v1/projects/go-orders/settings",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "retention_days_override": 7,
            "rate_limit_per_minute_override": 10
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p1_upd["effective_retention_days"], 7);
    assert_eq!(p1_upd["effective_rate_limit_per_minute"], 10);

    // Clearing project override (`null`) falls back to workspace (100/min & 14d).
    let (st, p1_reset) = send_cookie_json(
        &app,
        Method::PATCH,
        "/api/v1/projects/go-orders/settings",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "retention_days_override": null,
            "rate_limit_per_minute_override": null
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p1_reset["effective_retention_days"], 14);
    assert_eq!(p1_reset["effective_rate_limit_per_minute"], 100);
    assert!(p1_reset["rate_limit_per_minute_override"].is_null());

    // Step 2d: Test live AGGREGATE TENANT RATE LIMIT (5/min across p1 + p2, while each project has 100/min).
    for i in 0..3 {
        let (st, _, _) = send_error_envelope_with_headers(
            &app,
            p1_id,
            &p1_key,
            &format!("a100000000000000000000000000000{i}"),
            "p1 error under tenant cap",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "p1 event {i} should pass");
    }
    for i in 0..2 {
        let (st, _, _) = send_error_envelope_with_headers(
            &app,
            p2_id,
            &p2_key,
            &format!("a200000000000000000000000000000{i}"),
            "p2 error filling tenant cap",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "p2 event {i} should pass");
    }
    // 6th event across Tenant A (on p2, still way under p2's 100/min) MUST return 429 with `organization` scope!
    let (st, retry_after, sentry_limits) = send_error_envelope_with_headers(
        &app,
        p2_id,
        &p2_key,
        "a2000000000000000000000000000099",
        "p2 event exceeding tenant aggregate cap",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::TOO_MANY_REQUESTS,
        "tenant aggregate cap must reject 6th event"
    );
    assert!(retry_after.is_some());
    assert!(
        sentry_limits
            .as_deref()
            .unwrap_or("")
            .ends_with(":error:organization"),
        "expected organization rate limit header, got: {sentry_limits:?}"
    );

    // Tenant B is completely unaffected by Tenant A's exhausted aggregate quota.
    let (st, _, _) = send_error_envelope_with_headers(
        &app,
        pb_id,
        &pb_key,
        "b1000000000000000000000000000001",
        "tenant b unaffected",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "tenant B must not be throttled by tenant A"
    );

    // Wait for worker batch flush so p1 & p2 issues exist in DB for API token tests.
    tokio::time::sleep(Duration::from_millis(450)).await;

    // =========================================================================
    // 3. PER-PROJECT API TOKENS (F-ADM-4): read vs write, strict project isolation, admin denial, revocation
    // =========================================================================
    // Member cannot mint API tokens (403).
    let (st, _) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/api-tokens",
        &member_cookie,
        &member_csrf,
        Some(json!({
            "project_slug": "go-orders",
            "name": "unauthorized-member-tok",
            "can_write": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Owner mints READ-ONLY token for `go-orders` (p1).
    let (st, ro_created) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/api-tokens",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "project_slug": "go-orders",
            "name": "ci-readonly",
            "can_write": false,
            "expires_in_days": 30
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create ro token: {ro_created}");
    let ro_raw = ro_created["raw_token"].as_str().unwrap().to_owned();
    assert!(ro_raw.starts_with("mlapi_"));

    // Owner mints READ+WRITE token for `go-orders` (p1).
    let (st, rw_created) = send_cookie_json(
        &app,
        Method::POST,
        "/api/v1/api-tokens",
        &owner_cookie,
        &owner_csrf,
        Some(json!({
            "project_slug": "go-orders",
            "name": "ci-sourcemaps-write",
            "can_write": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create rw token: {rw_created}");
    let rw_id = rw_created["token"]["id"].as_i64().unwrap();
    let rw_raw = rw_created["raw_token"].as_str().unwrap().to_owned();

    // Read-only token lists ONLY `go-orders` projects & issues (`200`), NOT `rust-billing`!
    let (st, tok_projs) =
        send_bearer_json(&app, Method::GET, "/api/v1/projects", &ro_raw, None).await;
    assert_eq!(st, StatusCode::OK);
    let projs_arr = tok_projs.as_array().unwrap();
    assert_eq!(projs_arr.len(), 1);
    assert_eq!(projs_arr[0]["slug"], "go-orders");

    let (st, tok_issues) =
        send_bearer_json(&app, Method::GET, "/api/v1/issues", &ro_raw, None).await;
    assert_eq!(st, StatusCode::OK);
    let p1_issues = tok_issues["items"].as_array().unwrap();
    assert!(!p1_issues.is_empty());
    assert!(p1_issues.iter().all(|i| i["project_slug"] == "go-orders"));
    let p1_issue_id = p1_issues[0]["id"].as_i64().unwrap();

    // Read-only token CANNOT read `rust-billing` (`403`)!
    let (st, _) = send_bearer_json(
        &app,
        Method::GET,
        "/api/v1/projects/rust-billing",
        &ro_raw,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = send_bearer_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=rust-billing",
        &ro_raw,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Read-only token CANNOT triage issues (`403`) or upload source maps (`403`).
    let (st, _) = send_bearer_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{p1_issue_id}/status"),
        &ro_raw,
        Some(json!({ "status": "resolved" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Even READ+WRITE token CANNOT access admin/settings/members/alerts/mcp/api-tokens endpoints (`403`)!
    for admin_uri in [
        "/api/v1/members",
        "/api/v1/api-tokens",
        "/api/v1/mcp/tokens",
        "/api/v1/alerts/channels",
        "/api/v1/pii-rules",
        "/api/v1/tenants/current",
    ] {
        let (st, _) = send_bearer_json(&app, Method::GET, admin_uri, &rw_raw, None).await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "api token must be rejected on admin route {admin_uri}"
        );
    }

    // Read+Write token CAN upload a source map via raw CI endpoint without CSRF (`201`),
    // register a release (`201`), and resolve its own project's issue (`200`)!
    let minimal_map = json!({
        "version": 3,
        "file": "app.js",
        "sources": ["src/app.tsx"],
        "sourcesContent": ["export const x = 1;"],
        "names": [],
        "mappings": "AAAA"
    });
    let sm_req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/projects/go-orders/releases/v1.2.3/sourcemaps?filename=assets/app.js")
        .header(header::AUTHORIZATION, format!("Bearer {rw_raw}"))
        .body(Body::from(minimal_map.to_string()))
        .expect("sm req");
    let sm_resp = app.clone().oneshot(sm_req).await.expect("sm resp");
    assert_eq!(sm_resp.status(), StatusCode::CREATED);

    // Read+Write token CANNOT upload a source map to sibling project `rust-billing` (`403`).
    let sm_cross_req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/projects/rust-billing/releases/v1.2.3/sourcemaps?filename=assets/app.js")
        .header(header::AUTHORIZATION, format!("Bearer {rw_raw}"))
        .body(Body::from(minimal_map.to_string()))
        .expect("sm cross req");
    let sm_cross_resp = app
        .clone()
        .oneshot(sm_cross_req)
        .await
        .expect("sm cross resp");
    assert_eq!(sm_cross_resp.status(), StatusCode::FORBIDDEN);

    let (st, resolved) = send_bearer_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{p1_issue_id}/status"),
        &rw_raw,
        Some(json!({ "status": "resolved", "note": "triaged via project API token" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "rw token triage: {resolved}");
    assert_eq!(resolved["status"], "resolved");

    // Revoking the token immediately returns `401 Unauthorized`.
    let (st, _) = send_cookie_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/api-tokens/{rw_id}"),
        &owner_cookie,
        &owner_csrf,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _) = send_bearer_json(&app, Method::GET, "/api/v1/issues", &rw_raw, None).await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "revoked api token must return 401"
    );
}
