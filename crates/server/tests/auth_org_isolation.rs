use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::model::AuthSessionView;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_auth::token::{CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use maxi_loop_org::model::{ProjectView, TenantOverview, WorkspaceView};
use maxi_loop_platform::bootstrap::{
    AdminUserSpec, BootstrapSpec, bootstrap_defaults, hash_password_argon2id,
};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{connect_db, run_migrations};
use maxi_loop_platform::tenant::begin_worker_tx;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn setup_test_env() -> (Config, PgPool) {
    let _ = dotenvy::dotenv();
    let mut config = Config::from_env().expect("DATABASE_URL must be set for integration tests");
    config.public_base_url = "http://127.0.0.1:8080".to_owned();
    config.single_tenant_mode = true;
    config.cookie_secure = false;
    config.retention_default_days = 30;

    let pool = connect_db(&config)
        .await
        .expect("connect to test Postgres 16");
    run_migrations(&pool)
        .await
        .expect("apply migrations cleanly");
    (config, pool)
}

async fn send_json(
    app: &Router,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE_NAME}={c}"));
    }
    if let Some(token) = csrf {
        builder = builder.header(CSRF_HEADER_NAME, token);
    }
    let req_body = match body {
        Some(val) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&val).expect("serialize request json"))
        }
        None => Body::empty(),
    };

    let response = app
        .clone()
        .oneshot(builder.body(req_body).expect("build request"))
        .await
        .expect("router response");

    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body bytes")
        .to_vec();
    (status, headers, bytes)
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> T {
    serde_json::from_slice(bytes).expect("parse response json")
}

fn extract_session_token_from_set_cookie(headers: &axum::http::HeaderMap) -> String {
    let set_cookie = headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("Set-Cookie header must be present");
    assert!(
        set_cookie.contains("HttpOnly"),
        "session cookie must be HttpOnly"
    );
    assert!(
        set_cookie.contains("SameSite=Lax"),
        "session cookie must set SameSite=Lax"
    );

    let first_pair = set_cookie.split(';').next().expect("cookie key=value");
    let (name, val) = first_pair.split_once('=').expect("cookie split =");
    assert_eq!(name.trim(), SESSION_COOKIE_NAME);
    val.trim().to_owned()
}

#[tokio::test]
async fn auth_csrf_rate_limit_org_dsn_and_cross_tenant_isolation() {
    let (config, pool) = setup_test_env().await;

    // Clean up any test fixtures from prior runs.
    let mut cleanup_tx = begin_worker_tx(&pool).await.expect("begin cleanup tx");
    sqlx::query("DELETE FROM tenants WHERE slug IN ('f003-tenant-a', 'f003-tenant-b')")
        .execute(&mut *cleanup_tx)
        .await
        .expect("clean test tenants");
    sqlx::query(
        "DELETE FROM users WHERE email IN (
            'owner-a@f003.local',
            'viewer-a@f003.local',
            'owner-b@f003.local',
            'ratelimit-test@f003.local'
        )",
    )
    .execute(&mut *cleanup_tx)
    .await
    .expect("clean test users");
    cleanup_tx.commit().await.expect("commit cleanup");

    // Seed Tenant A (via bootstrap_defaults) and Tenant B (plus a viewer in Tenant A).
    let spec_a = BootstrapSpec {
        tenant_slug: "f003-tenant-a".to_owned(),
        tenant_name: "F003 Tenant A".to_owned(),
        retention_days: 30,
        workspace_slug: "core-a".to_owned(),
        workspace_name: "Core A".to_owned(),
        admin_user: Some(AdminUserSpec {
            email: "owner-a@f003.local".to_owned(),
            password: "correct_password_tenant_a_123".to_owned(),
            name: "Owner A".to_owned(),
        }),
    };
    let outcome_a = bootstrap_defaults(&pool, &spec_a)
        .await
        .expect("bootstrap tenant a");

    let mut seed_tx = begin_worker_tx(&pool).await.expect("begin seed tx");
    let viewer_pw_hash =
        hash_password_argon2id("viewer_password_tenant_a_123").expect("hash viewer pw");
    let viewer_a_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, name, password_hash, is_active)
         VALUES ('viewer-a@f003.local', 'Viewer A', $1, TRUE)
         RETURNING id",
    )
    .bind(&viewer_pw_hash)
    .fetch_one(&mut *seed_tx)
    .await
    .expect("insert viewer a");

    sqlx::query(
        "INSERT INTO memberships (tenant_id, workspace_id, user_id, role)
         VALUES ($1, NULL, $2, 'viewer')",
    )
    .bind(outcome_a.tenant_id)
    .bind(viewer_a_id)
    .execute(&mut *seed_tx)
    .await
    .expect("insert viewer membership");

    // Seed Tenant B, its default workspace, and Owner B.
    let tenant_b_id: i64 = sqlx::query_scalar(
        "INSERT INTO tenants (slug, name, retention_days, is_default)
         VALUES ('f003-tenant-b', 'F003 Tenant B', 30, FALSE)
         RETURNING id",
    )
    .fetch_one(&mut *seed_tx)
    .await
    .expect("insert tenant b");

    sqlx::query(
        "INSERT INTO workspaces (tenant_id, slug, name, is_default)
         VALUES ($1, 'core-b', 'Core B', TRUE)",
    )
    .bind(tenant_b_id)
    .execute(&mut *seed_tx)
    .await
    .expect("insert default workspace b");

    let owner_b_pw_hash =
        hash_password_argon2id("correct_password_tenant_b_123").expect("hash owner b pw");
    let owner_b_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, name, password_hash, is_active)
         VALUES ('owner-b@f003.local', 'Owner B', $1, TRUE)
         RETURNING id",
    )
    .bind(&owner_b_pw_hash)
    .fetch_one(&mut *seed_tx)
    .await
    .expect("insert owner b");

    sqlx::query(
        "INSERT INTO memberships (tenant_id, workspace_id, user_id, role)
         VALUES ($1, NULL, $2, 'tenant_owner')",
    )
    .bind(tenant_b_id)
    .bind(owner_b_id)
    .execute(&mut *seed_tx)
    .await
    .expect("insert owner b membership");

    seed_tx.commit().await.expect("commit seed tx");

    let limiter = LoginRateLimiter::new(4, Duration::from_secs(60), 128);
    let app = build_router(AppState::with_login_limiter(config, pool.clone(), limiter));

    // 1. Unauthenticated /api/v1/auth/me must return 401.
    let (status, _, _) = send_json(&app, Method::GET, "/api/v1/auth/me", None, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 2. Wrong password and unknown user must both return identical 401 message without user enumeration.
    let (status_wrong_pw, _, body_wrong_pw) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner-a@f003.local",
            "password": "wrong_password_attempt_123"
        })),
    )
    .await;
    assert_eq!(status_wrong_pw, StatusCode::UNAUTHORIZED);

    let (status_unknown, _, body_unknown) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "nonexistent@f003.local",
            "password": "wrong_password_attempt_123"
        })),
    )
    .await;
    assert_eq!(status_unknown, StatusCode::UNAUTHORIZED);
    assert_eq!(body_wrong_pw, body_unknown);

    // 3. Login rate limiting: after 4 attempts on the same email, the 5th attempt returns 429 + Retry-After.
    for _ in 0..4 {
        let (st, _, _) = send_json(
            &app,
            Method::POST,
            "/api/v1/auth/login",
            None,
            None,
            Some(json!({
                "email": "ratelimit-test@f003.local",
                "password": "some_password_attempt_123"
            })),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    let (st_limited, headers_limited, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "ratelimit-test@f003.local",
            "password": "some_password_attempt_123"
        })),
    )
    .await;
    assert_eq!(st_limited, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers_limited.contains_key(header::RETRY_AFTER));

    // 4. Owner A logs in (omitting tenant_slug resolves Owner A's tenant seamlessly).
    let (status_login_a, headers_login_a, body_login_a) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner-a@f003.local",
            "password": "correct_password_tenant_a_123"
        })),
    )
    .await;
    assert_eq!(status_login_a, StatusCode::OK);
    let cookie_a = extract_session_token_from_set_cookie(&headers_login_a);
    let session_a: AuthSessionView = parse_json(&body_login_a);
    assert_eq!(session_a.user.email, "owner-a@f003.local");
    assert_eq!(session_a.tenant.slug, "f003-tenant-a");
    assert_eq!(session_a.default_workspace.slug, "core-a");
    assert!(session_a.single_tenant_mode);
    assert_eq!(session_a.csrf_token.len(), 64);

    // Verify raw session token is NEVER stored in the `sessions` table in PostgreSQL.
    let mut check_tx = begin_worker_tx(&pool).await.expect("worker tx");
    let raw_in_db: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE token_hash = $1")
        .bind(&cookie_a)
        .fetch_one(&mut *check_tx)
        .await
        .expect("count raw token in db");
    assert_eq!(
        raw_in_db, 0,
        "raw session token must never be stored in PostgreSQL"
    );
    check_tx.commit().await.expect("commit check tx");

    // 5. GET /api/v1/auth/me and GET /api/v1/tenants/current work with cookie_a.
    let (status_me, _, body_me) = send_json(
        &app,
        Method::GET,
        "/api/v1/auth/me",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(status_me, StatusCode::OK);
    let me_view: AuthSessionView = parse_json(&body_me);
    assert_eq!(me_view.tenant.slug, "f003-tenant-a");

    let (status_curr, _, body_curr) = send_json(
        &app,
        Method::GET,
        "/api/v1/tenants/current",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(status_curr, StatusCode::OK);
    let overview_a: TenantOverview = parse_json(&body_curr);
    assert_eq!(overview_a.tenant.slug, "f003-tenant-a");
    assert_eq!(overview_a.default_workspace.slug, "core-a");
    assert!(overview_a.single_tenant_mode);

    // 6. CSRF enforcement: POST without X-CSRF-Token or with wrong X-CSRF-Token MUST return 403 Forbidden.
    let (status_no_csrf, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/workspaces",
        Some(&cookie_a),
        None,
        Some(json!({ "slug": "payments-a", "name": "Payments A" })),
    )
    .await;
    assert_eq!(status_no_csrf, StatusCode::FORBIDDEN);

    let (status_bad_csrf, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/workspaces",
        Some(&cookie_a),
        Some("0000000000000000000000000000000000000000000000000000000000000000"),
        Some(json!({ "slug": "payments-a", "name": "Payments A" })),
    )
    .await;
    assert_eq!(status_bad_csrf, StatusCode::FORBIDDEN);

    // 7. Create workspace with valid CSRF token; test retention override validation (must be <= global 30 days) and duplicate slug (409).
    let (status_invalid_ret, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/workspaces",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        Some(json!({
            "slug": "payments-a",
            "name": "Payments A",
            "retention_days_override": 45
        })),
    )
    .await;
    assert_eq!(
        status_invalid_ret,
        StatusCode::BAD_REQUEST,
        "retention override > global (30d) must be rejected"
    );

    let (status_ws_create, _, body_ws_create) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/workspaces",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        Some(json!({
            "slug": "payments-a",
            "name": "Payments A",
            "retention_days_override": 14
        })),
    )
    .await;
    assert_eq!(status_ws_create, StatusCode::CREATED);
    let ws_payments_a: WorkspaceView = parse_json(&body_ws_create);
    assert_eq!(ws_payments_a.slug, "payments-a");
    assert_eq!(ws_payments_a.retention_days_override, Some(14));
    assert_eq!(ws_payments_a.effective_retention_days, 14);

    let (status_ws_dup, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/workspaces",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        Some(json!({
            "slug": "payments-a",
            "name": "Payments Duplicate"
        })),
    )
    .await;
    assert_eq!(status_ws_dup, StatusCode::CONFLICT);

    // 8. Create projects via both slug-based route and single-tenant convenience route, then rotate DSN.
    let (status_proj_a1, _, body_proj_a1) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/w/payments-a/projects",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        Some(json!({
            "slug": "checkout-api",
            "name": "Checkout FastAPI",
            "platform": "python-fastapi",
            "retention_days_override": 7,
            "rate_limit_per_minute": 600
        })),
    )
    .await;
    assert_eq!(status_proj_a1, StatusCode::CREATED);
    let proj_a1: ProjectView = parse_json(&body_proj_a1);
    assert_eq!(proj_a1.tenant_slug, "f003-tenant-a");
    assert_eq!(proj_a1.workspace_slug, "payments-a");
    assert_eq!(proj_a1.slug, "checkout-api");
    assert_eq!(proj_a1.effective_retention_days, 7);
    assert_eq!(proj_a1.dsn_public_key.len(), 32);
    assert_eq!(
        proj_a1.dsn,
        format!(
            "http://{}@127.0.0.1:8080/{}",
            proj_a1.dsn_public_key, proj_a1.id
        )
    );
    assert_eq!(
        proj_a1.ingest_endpoint,
        format!("http://127.0.0.1:8080/api/{}/envelope/", proj_a1.id)
    );

    // Rotate DSN via slug route and verify key changes while project ID stays constant.
    let (status_rotate, _, body_rotate) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/w/payments-a/p/checkout-api/dsn/rotate",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        None,
    )
    .await;
    assert_eq!(status_rotate, StatusCode::OK);
    let rotated_a1: ProjectView = parse_json(&body_rotate);
    assert_eq!(rotated_a1.id, proj_a1.id);
    assert_ne!(rotated_a1.dsn_public_key, proj_a1.dsn_public_key);
    assert_ne!(rotated_a1.dsn, proj_a1.dsn);

    // Create a project via single-tenant convenience route `/api/v1/projects` (defaults to `core-a` workspace).
    let (status_proj_default, _, body_proj_default) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        Some(json!({
            "slug": "web-frontend",
            "name": "React Frontend",
            "platform": "javascript-react"
        })),
    )
    .await;
    assert_eq!(status_proj_default, StatusCode::CREATED);
    let proj_default: ProjectView = parse_json(&body_proj_default);
    assert_eq!(proj_default.workspace_slug, "core-a");

    // 9. Role-based authorization: Viewer A can read workspaces/projects, but cannot create or rotate DSNs (403).
    let (status_login_viewer, headers_viewer, body_viewer) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "viewer-a@f003.local",
            "password": "viewer_password_tenant_a_123",
            "tenant_slug": "f003-tenant-a"
        })),
    )
    .await;
    assert_eq!(status_login_viewer, StatusCode::OK);
    let cookie_viewer = extract_session_token_from_set_cookie(&headers_viewer);
    let session_viewer: AuthSessionView = parse_json(&body_viewer);

    let (status_viewer_list, _, body_viewer_list) = send_json(
        &app,
        Method::GET,
        "/api/v1/projects",
        Some(&cookie_viewer),
        None,
        None,
    )
    .await;
    assert_eq!(status_viewer_list, StatusCode::OK);
    let viewer_projects: Vec<ProjectView> = parse_json(&body_viewer_list);
    assert_eq!(viewer_projects.len(), 2);

    let (status_viewer_create_proj, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&cookie_viewer),
        Some(&session_viewer.csrf_token),
        Some(json!({ "slug": "unauthorized-proj", "name": "Unauthorized" })),
    )
    .await;
    assert_eq!(status_viewer_create_proj, StatusCode::FORBIDDEN);

    let (status_viewer_rotate, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-a/w/payments-a/p/checkout-api/dsn/rotate",
        Some(&cookie_viewer),
        Some(&session_viewer.csrf_token),
        None,
    )
    .await;
    assert_eq!(status_viewer_rotate, StatusCode::FORBIDDEN);

    // 10. Cross-tenant isolation verification: Owner B logs into Tenant B and creates a project.
    // First verify Owner A cannot log into Tenant B.
    let (status_cross_login, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner-a@f003.local",
            "password": "correct_password_tenant_a_123",
            "tenant_slug": "f003-tenant-b"
        })),
    )
    .await;
    assert_eq!(status_cross_login, StatusCode::UNAUTHORIZED);

    let (status_login_b, headers_login_b, body_login_b) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner-b@f003.local",
            "password": "correct_password_tenant_b_123",
            "tenant_slug": "f003-tenant-b"
        })),
    )
    .await;
    assert_eq!(status_login_b, StatusCode::OK);
    let cookie_b = extract_session_token_from_set_cookie(&headers_login_b);
    let session_b: AuthSessionView = parse_json(&body_login_b);
    assert_eq!(session_b.tenant.slug, "f003-tenant-b");

    let (status_proj_b, _, body_proj_b) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-b/w/core-b/projects",
        Some(&cookie_b),
        Some(&session_b.csrf_token),
        Some(json!({
            "slug": "secret-service-b",
            "name": "Secret Service B",
            "platform": "go"
        })),
    )
    .await;
    assert_eq!(status_proj_b, StatusCode::CREATED);
    let proj_b: ProjectView = parse_json(&body_proj_b);

    // Tenant A MUST NOT be able to read or mutate any Tenant B resource via slug routes.
    let (st_a_read_b_tenant, _, _) = send_json(
        &app,
        Method::GET,
        "/api/v1/t/f003-tenant-b",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_a_read_b_tenant, StatusCode::NOT_FOUND);

    let (st_a_list_b_ws, _, _) = send_json(
        &app,
        Method::GET,
        "/api/v1/t/f003-tenant-b/workspaces",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_a_list_b_ws, StatusCode::NOT_FOUND);

    let (st_a_create_b_ws, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-b/workspaces",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        Some(json!({ "slug": "forged-ws", "name": "Forged" })),
    )
    .await;
    assert_eq!(st_a_create_b_ws, StatusCode::NOT_FOUND);

    let (st_a_read_b_proj, _, _) = send_json(
        &app,
        Method::GET,
        "/api/v1/t/f003-tenant-b/w/core-b/p/secret-service-b",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_a_read_b_proj, StatusCode::NOT_FOUND);

    let (st_a_rotate_b_dsn, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f003-tenant-b/w/core-b/p/secret-service-b/dsn/rotate",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        None,
    )
    .await;
    assert_eq!(st_a_rotate_b_dsn, StatusCode::NOT_FOUND);

    // Verify Tenant B's DSN key was NOT modified by Tenant A's attempt.
    let (st_b_verify, _, body_b_verify) = send_json(
        &app,
        Method::GET,
        "/api/v1/t/f003-tenant-b/w/core-b/p/secret-service-b",
        Some(&cookie_b),
        None,
        None,
    )
    .await;
    assert_eq!(st_b_verify, StatusCode::OK);
    let proj_b_after: ProjectView = parse_json(&body_b_verify);
    assert_eq!(proj_b_after.dsn_public_key, proj_b.dsn_public_key);

    // Verify Tenant B's `/api/v1/projects` only lists Tenant B's project, never Tenant A's.
    let (st_b_list, _, body_b_list) = send_json(
        &app,
        Method::GET,
        "/api/v1/projects",
        Some(&cookie_b),
        None,
        None,
    )
    .await;
    assert_eq!(st_b_list, StatusCode::OK);
    let b_projects: Vec<ProjectView> = parse_json(&body_b_list);
    assert_eq!(b_projects.len(), 1);
    assert_eq!(b_projects[0].slug, "secret-service-b");

    // 11. Logout requires CSRF token, invalidates session in DB, and clears cookie.
    let (st_logout_no_csrf, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/logout",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_logout_no_csrf, StatusCode::FORBIDDEN);

    let (st_logout_ok, headers_logout, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/logout",
        Some(&cookie_a),
        Some(&session_a.csrf_token),
        None,
    )
    .await;
    assert_eq!(st_logout_ok, StatusCode::OK);
    let clear_cookie = headers_logout
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("Set-Cookie on logout");
    assert!(clear_cookie.contains("Max-Age=0"));

    // Subsequent request with the logged-out cookie must fail with 401.
    let (st_after_logout, _, _) = send_json(
        &app,
        Method::GET,
        "/api/v1/auth/me",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_after_logout, StatusCode::UNAUTHORIZED);
}
