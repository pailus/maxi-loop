use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::{Duration, Utc};
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::model::AuthSessionView;
use maxi_loop_auth::token::{CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use maxi_loop_ingest::model::QueuedEvent;
use maxi_loop_ingest::worker::flush_queued_events;
use maxi_loop_issues::model::{
    IssueDetailView, IssueEventView, IssueStatsResponse, IssueSummaryView, PaginatedIssuesResponse,
};
use maxi_loop_org::model::ProjectView;
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
    config.single_tenant_mode = false;
    config.cookie_secure = false;
    config.retention_default_days = 30;
    config.max_full_events_per_issue = 50;

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

fn extract_cookie(headers: &axum::http::HeaderMap) -> String {
    let set_cookie = headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("Set-Cookie header");
    let first = set_cookie.split(';').next().unwrap();
    let (_, val) = first.split_once('=').unwrap();
    val.trim().to_owned()
}

#[tokio::test]
async fn issues_list_filters_pagination_detail_stats_triage_and_cross_tenant_isolation() {
    let (config, pool) = setup_test_env().await;

    // Clean up prior test fixtures.
    let mut cleanup_tx = begin_worker_tx(&pool).await.expect("cleanup tx");
    sqlx::query("DELETE FROM tenants WHERE slug IN ('f006-tenant-a', 'f006-tenant-b')")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f006 tenants");
    sqlx::query(
        "DELETE FROM users WHERE email IN ('owner-a@f006.local', 'viewer-a@f006.local', 'owner-b@f006.local')",
    )
    .execute(&mut *cleanup_tx)
    .await
    .expect("delete old f006 users");
    cleanup_tx.commit().await.expect("commit cleanup");

    let spec_a = BootstrapSpec {
        tenant_slug: "f006-tenant-a".to_owned(),
        tenant_name: "F006 Tenant A".to_owned(),
        retention_days: 30,
        workspace_slug: "core-a".to_owned(),
        workspace_name: "Core A".to_owned(),
        admin_user: Some(AdminUserSpec {
            email: "owner-a@f006.local".to_owned(),
            password: "f006_owner_a_password_123".to_owned(),
            name: "Owner A".to_owned(),
        }),
    };
    let spec_b = BootstrapSpec {
        tenant_slug: "f006-tenant-b".to_owned(),
        tenant_name: "F006 Tenant B".to_owned(),
        retention_days: 30,
        workspace_slug: "core-b".to_owned(),
        workspace_name: "Core B".to_owned(),
        admin_user: Some(AdminUserSpec {
            email: "owner-b@f006.local".to_owned(),
            password: "f006_owner_b_password_123".to_owned(),
            name: "Owner B".to_owned(),
        }),
    };
    let out_a = bootstrap_defaults(&pool, &spec_a)
        .await
        .expect("bootstrap tenant A");
    bootstrap_defaults(&pool, &spec_b)
        .await
        .expect("bootstrap tenant B");

    // Seed a read-only `viewer` in Tenant A to verify server-side triage authorization.
    let viewer_pw_hash =
        hash_password_argon2id("f006_viewer_a_password_123").expect("hash viewer pw");
    let mut seed_tx = begin_worker_tx(&pool).await.expect("seed viewer tx");
    let viewer_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, name, password_hash, is_active) \
         VALUES ('viewer-a@f006.local', 'Viewer A', $1, TRUE) RETURNING id",
    )
    .bind(&viewer_pw_hash)
    .fetch_one(&mut *seed_tx)
    .await
    .expect("insert viewer user");
    sqlx::query(
        "INSERT INTO memberships (tenant_id, workspace_id, user_id, role) \
         VALUES ($1, NULL, $2, 'viewer')",
    )
    .bind(out_a.tenant_id)
    .bind(viewer_id)
    .execute(&mut *seed_tx)
    .await
    .expect("insert viewer membership");
    seed_tx.commit().await.expect("commit viewer seed");

    let app = build_router(AppState::new(config, pool.clone()));

    // Login as Owner A, Viewer A, and Owner B.
    let (st, h_owner_a, b_owner_a) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner-a@f006.local",
            "password": "f006_owner_a_password_123",
            "tenant_slug": "f006-tenant-a"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cookie_a = extract_cookie(&h_owner_a);
    let sess_a: AuthSessionView = parse_json(&b_owner_a);

    let (st, h_viewer_a, b_viewer_a) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "viewer-a@f006.local",
            "password": "f006_viewer_a_password_123",
            "tenant_slug": "f006-tenant-a"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cookie_viewer_a = extract_cookie(&h_viewer_a);
    let sess_viewer_a: AuthSessionView = parse_json(&b_viewer_a);

    let (st, h_owner_b, b_owner_b) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner-b@f006.local",
            "password": "f006_owner_b_password_123",
            "tenant_slug": "f006-tenant-b"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cookie_b = extract_cookie(&h_owner_b);
    let sess_b: AuthSessionView = parse_json(&b_owner_b);

    // Create 2 projects in Tenant A (`backend-api` and `web-ui`) and 1 project in Tenant B (`secret-b`).
    let (st, _, b_proj_a1) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f006-tenant-a/w/core-a/projects",
        Some(&cookie_a),
        Some(&sess_a.csrf_token),
        Some(json!({
            "slug": "backend-api",
            "name": "Backend API",
            "platform": "python-fastapi"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_a1: ProjectView = parse_json(&b_proj_a1);

    let (st, _, b_proj_a2) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f006-tenant-a/w/core-a/projects",
        Some(&cookie_a),
        Some(&sess_a.csrf_token),
        Some(json!({
            "slug": "web-ui",
            "name": "Web UI",
            "platform": "javascript-react"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_a2: ProjectView = parse_json(&b_proj_a2);

    let (st, _, b_proj_b1) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f006-tenant-b/w/core-b/projects",
        Some(&cookie_b),
        Some(&sess_b.csrf_token),
        Some(json!({
            "slug": "secret-b",
            "name": "Secret B Service",
            "platform": "go"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_b1: ProjectView = parse_json(&b_proj_b1);

    // Seed events across two distinct hours for Tenant A (`proj_a1` and `proj_a2`) and Tenant B (`proj_b1`).
    let now = Utc::now();
    let prev_hour = now - Duration::hours(2);

    let ev_a1_h1 = QueuedEvent {
        tenant_id: proj_a1.tenant_id,
        workspace_id: proj_a1.workspace_id,
        project_id: proj_a1.id,
        event_id: "f006a111111111111111111111111111".to_owned(),
        received_at: prev_hour,
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: Some("v1.0.0".to_owned()),
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "event_id": "f006a111111111111111111111111111",
            "exception": {
                "values": [{
                    "type": "KeyError",
                    "value": "'customer_tier'",
                    "stacktrace": {
                        "frames": [{
                            "filename": "app/orders/pricing.py",
                            "module": "app.orders.pricing",
                            "function": "apply_tier_discount",
                            "lineno": 77,
                            "in_app": true,
                            "pre_context": ["def apply_tier_discount(order):"],
                            "context_line": "    tier = order['customer_tier']",
                            "post_context": ["    return tier"]
                        }]
                    }
                }]
            },
            "tags": {
                "region": "ap-southeast-1",
                "endpoint": "/v1/orders"
            }
        }))
        .unwrap(),
    };

    let ev_a1_h2 = QueuedEvent {
        received_at: now,
        event_id: "f006a222222222222222222222222222".to_owned(),
        ..ev_a1_h1.clone()
    };

    let ev_a1_staging = QueuedEvent {
        tenant_id: proj_a1.tenant_id,
        workspace_id: proj_a1.workspace_id,
        project_id: proj_a1.id,
        event_id: "f006a333333333333333333333333333".to_owned(),
        received_at: now - Duration::minutes(10),
        level: "warning".to_owned(),
        environment: "staging".to_owned(),
        release: Some("v1.1.0-rc1".to_owned()),
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "event_id": "f006a333333333333333333333333333",
            "exception": {
                "values": [{
                    "type": "TimeoutError",
                    "value": "redis cache read timed out",
                    "stacktrace": {
                        "frames": [{
                            "filename": "app/cache/redis.py",
                            "module": "app.cache.redis",
                            "function": "get_cached_profile",
                            "lineno": 19,
                            "in_app": true
                        }]
                    }
                }]
            },
            "tags": {
                "cache": "redis-staging"
            }
        }))
        .unwrap(),
    };

    let ev_a2_ui = QueuedEvent {
        tenant_id: proj_a2.tenant_id,
        workspace_id: proj_a2.workspace_id,
        project_id: proj_a2.id,
        event_id: "f006a444444444444444444444444444".to_owned(),
        received_at: now - Duration::minutes(5),
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: Some("v2.0.0".to_owned()),
        platform: "javascript".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "event_id": "f006a444444444444444444444444444",
            "exception": {
                "values": [{
                    "type": "TypeError",
                    "value": "Cannot read properties of undefined (reading 'map')",
                    "stacktrace": {
                        "frames": [{
                            "filename": "src/components/IssueTable.tsx",
                            "function": "IssueTable",
                            "lineno": 32,
                            "in_app": true
                        }]
                    }
                }]
            }
        }))
        .unwrap(),
    };

    let ev_b1_secret = QueuedEvent {
        tenant_id: proj_b1.tenant_id,
        workspace_id: proj_b1.workspace_id,
        project_id: proj_b1.id,
        event_id: "f006b999999999999999999999999999".to_owned(),
        received_at: now,
        level: "fatal".to_owned(),
        environment: "production".to_owned(),
        release: Some("v9.9.9".to_owned()),
        platform: "go".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "event_id": "f006b999999999999999999999999999",
            "message": "TENANT_B_CONFIDENTIAL_PANIC_DO_NOT_LEAK"
        }))
        .unwrap(),
    };

    flush_queued_events(
        &pool,
        vec![ev_a1_h1, ev_a1_h2, ev_a1_staging, ev_a2_ui, ev_b1_secret],
        50,
    )
    .await
    .expect("flush test events");

    // 1. List issues with pagination and filters (project, environment, release, status):
    // 1a. List all issues in Tenant A (should be 3 issues; Tenant B's issue must not appear)
    let (st_all_a, _, body_all_a) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues?per_page=2&page=1&sort=events",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_all_a, StatusCode::OK);
    let page1_a: PaginatedIssuesResponse = parse_json(&body_all_a);
    assert_eq!(page1_a.total, 3);
    assert_eq!(page1_a.page, 1);
    assert_eq!(page1_a.per_page, 2);
    assert_eq!(page1_a.total_pages, 2);
    assert_eq!(page1_a.items.len(), 2);
    // First item sorted by `events` DESC is the KeyError with 2 occurrences.
    assert_eq!(page1_a.items[0].exception_type, "KeyError");
    assert_eq!(page1_a.items[0].event_count, 2);
    let key_error_issue_id = page1_a.items[0].id;

    let (st_page2_a, _, body_page2_a) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues?per_page=2&page=2&sort=events",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_page2_a, StatusCode::OK);
    let page2_a: PaginatedIssuesResponse = parse_json(&body_page2_a);
    assert_eq!(page2_a.items.len(), 1);

    // 1b. Filter by project (`project_slug=backend-api`), environment (`staging`), and release (`v1.1.0-rc1`)
    let (st_filt, _, body_filt) = send_json(
        &app,
        Method::GET,
        "/api/v1/t/f006-tenant-a/w/core-a/p/backend-api/issues?environment=staging&release=v1.1.0-rc1",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_filt, StatusCode::OK);
    let filtered: PaginatedIssuesResponse = parse_json(&body_filt);
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.items[0].exception_type, "TimeoutError");
    assert_eq!(filtered.items[0].last_environment, "staging");
    assert_eq!(
        filtered.items[0].last_release.as_deref(),
        Some("v1.1.0-rc1")
    );
    let timeout_issue_id = filtered.items[0].id;

    // 2. Issue detail & hourly/daily stats:
    let (st_detail, _, body_detail) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/t/f006-tenant-a/w/core-a/p/backend-api/issues/{key_error_issue_id}"),
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_detail, StatusCode::OK);
    let detail: IssueDetailView = parse_json(&body_detail);
    assert_eq!(detail.issue.id, key_error_issue_id);
    assert_eq!(detail.issue.title, "KeyError: 'customer_tier'");
    assert_eq!(
        detail.issue.culprit,
        "app.orders.pricing in apply_tier_discount"
    );
    assert_eq!(detail.issue.last_environment, "production");
    assert_eq!(detail.issue.last_release.as_deref(), Some("v1.0.0"));
    assert_eq!(detail.tags["region"], "ap-southeast-1");
    assert_eq!(detail.tags["endpoint"], "/v1/orders");
    assert_eq!(detail.stack_frames[0]["function"], "apply_tier_discount");
    assert_eq!(
        detail.stack_frames[0]["context_line"],
        "    tier = order['customer_tier']"
    );
    let sample_ev = detail.sample_event.expect("sample event must be attached");
    assert_eq!(sample_ev.event_id, "f006a111111111111111111111111111");
    assert_eq!(detail.hourly_series.len(), 2);
    let hourly_sum: i64 = detail.hourly_series.iter().map(|p| p.event_count).sum();
    assert_eq!(hourly_sum, 2);
    let daily_sum: i64 = detail.daily_series.iter().map(|p| p.event_count).sum();
    assert_eq!(daily_sum, 2);

    // Verify dedicated `/stats?interval=1h` and `/stats?interval=1d` and `/events` endpoints:
    let (st_stats_1h, _, body_stats_1h) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{key_error_issue_id}/stats?interval=1h"),
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_stats_1h, StatusCode::OK);
    let stats_1h: IssueStatsResponse = parse_json(&body_stats_1h);
    assert_eq!(stats_1h.interval, "1h");
    assert_eq!(stats_1h.series.len(), 2);

    let (st_stats_1d, _, body_stats_1d) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{key_error_issue_id}/stats?interval=1d"),
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_stats_1d, StatusCode::OK);
    let stats_1d: IssueStatsResponse = parse_json(&body_stats_1d);
    assert_eq!(stats_1d.interval, "1d");
    assert_eq!(
        stats_1d.series.iter().map(|b| b.event_count).sum::<i64>(),
        2
    );

    let (st_events, _, body_events) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{key_error_issue_id}/events"),
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_events, StatusCode::OK);
    let events_list: Vec<IssueEventView> = parse_json(&body_events);
    assert_eq!(events_list.len(), 2);

    // 3. Issue status triage (`unresolved` / `resolved` / `ignored`) & server-side authorization:
    // 3a. Viewer in Tenant A can read issues, but cannot update issue status (403 Forbidden).
    let (st_viewer_read, _, _) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{key_error_issue_id}"),
        Some(&cookie_viewer_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_viewer_read, StatusCode::OK);

    let (st_viewer_triage, _, _) = send_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{key_error_issue_id}/status"),
        Some(&cookie_viewer_a),
        Some(&sess_viewer_a.csrf_token),
        Some(json!({ "status": "resolved" })),
    )
    .await;
    assert_eq!(
        st_viewer_triage,
        StatusCode::FORBIDDEN,
        "viewer role must be forbidden from mutating issue status"
    );

    // 3b. Missing CSRF token on PATCH is rejected with 403 Forbidden.
    let (st_no_csrf, _, _) = send_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{key_error_issue_id}/status"),
        Some(&cookie_a),
        None,
        Some(json!({ "status": "resolved" })),
    )
    .await;
    assert_eq!(st_no_csrf, StatusCode::FORBIDDEN);

    // 3c. Invalid status value is rejected with 400 Bad Request.
    let (st_bad_status, _, _) = send_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{key_error_issue_id}/status"),
        Some(&cookie_a),
        Some(&sess_a.csrf_token),
        Some(json!({ "status": "deleted" })),
    )
    .await;
    assert_eq!(st_bad_status, StatusCode::BAD_REQUEST);

    // 3d. Authorized Owner A resolves `key_error_issue_id` and ignores `timeout_issue_id`.
    let (st_resolve, _, body_resolve) = send_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{key_error_issue_id}/status"),
        Some(&cookie_a),
        Some(&sess_a.csrf_token),
        Some(json!({ "status": "resolved" })),
    )
    .await;
    assert_eq!(st_resolve, StatusCode::OK);
    let resolved_view: IssueSummaryView = parse_json(&body_resolve);
    assert_eq!(resolved_view.status, "resolved");

    let (st_ignore, _, body_ignore) = send_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/t/f006-tenant-a/w/core-a/p/backend-api/issues/{timeout_issue_id}"),
        Some(&cookie_a),
        Some(&sess_a.csrf_token),
        Some(json!({ "status": "ignored" })),
    )
    .await;
    assert_eq!(st_ignore, StatusCode::OK);
    let ignored_view: IssueSummaryView = parse_json(&body_ignore);
    assert_eq!(ignored_view.status, "ignored");

    // Verify status filter (`?status=resolved`, `?status=ignored`, `?status=unresolved`):
    let (st_list_resolved, _, body_list_resolved) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues?status=resolved",
        Some(&cookie_a),
        None,
        None,
    )
    .await;
    assert_eq!(st_list_resolved, StatusCode::OK);
    let resolved_list: PaginatedIssuesResponse = parse_json(&body_list_resolved);
    assert_eq!(resolved_list.total, 1);
    assert_eq!(resolved_list.items[0].id, key_error_issue_id);

    // 4. Cross-tenant isolation verification:
    // Fetch Tenant B's issue ID using Owner B's session.
    let (st_b_list, _, body_b_list) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues",
        Some(&cookie_b),
        None,
        None,
    )
    .await;
    assert_eq!(st_b_list, StatusCode::OK);
    let list_b: PaginatedIssuesResponse = parse_json(&body_b_list);
    assert_eq!(list_b.total, 1);
    let tenant_b_issue_id = list_b.items[0].id;

    // Tenant A cannot access Tenant B's issue list via slug route, issue detail, events, stats, or status triage!
    for uri in [
        "/api/v1/t/f006-tenant-b/issues".to_owned(),
        "/api/v1/t/f006-tenant-b/w/core-b/p/secret-b/issues".to_owned(),
        format!("/api/v1/issues/{tenant_b_issue_id}"),
        format!("/api/v1/t/f006-tenant-b/w/core-b/p/secret-b/issues/{tenant_b_issue_id}"),
        format!("/api/v1/issues/{tenant_b_issue_id}/stats"),
        format!("/api/v1/issues/{tenant_b_issue_id}/events"),
        format!("/api/v1/issues/{tenant_b_issue_id}/events/f006b999999999999999999999999999"),
    ] {
        let (st_cross_read, _, body_cross_read) =
            send_json(&app, Method::GET, &uri, Some(&cookie_a), None, None).await;
        assert_eq!(
            st_cross_read,
            StatusCode::NOT_FOUND,
            "Tenant A must get 404 Not Found on Tenant B resource {uri}"
        );
        let body_text = String::from_utf8_lossy(&body_cross_read);
        assert!(!body_text.contains("TENANT_B_CONFIDENTIAL_PANIC_DO_NOT_LEAK"));
    }

    let (st_cross_patch, _, _) = send_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{tenant_b_issue_id}/status"),
        Some(&cookie_a),
        Some(&sess_a.csrf_token),
        Some(json!({ "status": "resolved" })),
    )
    .await;
    assert_eq!(
        st_cross_patch,
        StatusCode::NOT_FOUND,
        "Tenant A must not be able to mutate Tenant B issue status"
    );
}
