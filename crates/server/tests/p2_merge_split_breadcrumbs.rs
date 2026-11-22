use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::api_tokens::CreatedApiTokenResponse;
use maxi_loop_auth::model::AuthSessionView;
use maxi_loop_auth::token::{CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use maxi_loop_ingest::model::QueuedEvent;
use maxi_loop_ingest::worker::flush_queued_events;
use maxi_loop_issues::merge_split::{MergeIssuesResponse, SplitIssueResponse};
use maxi_loop_issues::model::{IssueDetailView, IssueEventView, PaginatedIssuesResponse};
use maxi_loop_issues::notes::IssueNoteView;
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
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE_NAME}={c}"));
    }
    if let Some(token) = csrf {
        builder = builder.header(CSRF_HEADER_NAME, token);
    }
    if let Some(b) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {b}"));
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
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
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

fn make_event(
    ids: (i64, i64, i64),
    event_id: &str,
    release: &str,
    exc: (&str, &str, &str),
    extra_payload: Value,
) -> QueuedEvent {
    let (tenant_id, workspace_id, project_id) = ids;
    let (exc_type, exc_val, fn_name) = exc;
    let mut payload = json!({
        "event_id": event_id,
        "platform": "python",
        "level": "error",
        "environment": "production",
        "release": release,
        "exception": {
            "values": [{
                "type": exc_type,
                "value": exc_val,
                "stacktrace": {
                    "frames": [{
                        "filename": "app/checkout.py",
                        "module": "app.checkout",
                        "function": fn_name,
                        "lineno": 42,
                        "in_app": true,
                        "context_line": format!("    raise {exc_type}('{exc_val}')")
                    }]
                }
            }]
        }
    });
    if let (Some(base), Some(extra)) = (payload.as_object_mut(), extra_payload.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    QueuedEvent {
        tenant_id,
        workspace_id,
        project_id,
        event_id: event_id.to_owned(),
        received_at: Utc::now(),
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: Some(release.to_owned()),
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&payload).expect("serialize event"),
    }
}

#[tokio::test]
async fn p2_issue_merge_split_fingerprint_routing_breadcrumbs_and_isolation() {
    let (config, pool) = setup_test_env().await;

    // Clean up prior test fixtures.
    let mut cleanup_tx = begin_worker_tx(&pool).await.expect("cleanup tx");
    sqlx::query("DELETE FROM tenants WHERE slug IN ('f015-tenant-a', 'f015-tenant-b')")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f015 tenants");
    sqlx::query(
        "DELETE FROM users WHERE email IN ('owner-a@f015.local', 'viewer-a@f015.local', 'owner-b@f015.local')",
    )
    .execute(&mut *cleanup_tx)
    .await
    .expect("delete old f015 users");
    cleanup_tx.commit().await.expect("commit cleanup");

    let boot_a = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: "f015-tenant-a".to_owned(),
            tenant_name: "F015 Tenant A".to_owned(),
            retention_days: 30,
            workspace_slug: "ws-a".to_owned(),
            workspace_name: "WS A".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: "owner-a@f015.local".to_owned(),
                name: "Owner A".to_owned(),
                password: "OwnerAPassword123!".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant A");

    let boot_b = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: "f015-tenant-b".to_owned(),
            tenant_name: "F015 Tenant B".to_owned(),
            retention_days: 30,
            workspace_slug: "ws-b".to_owned(),
            workspace_name: "WS B".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: "owner-b@f015.local".to_owned(),
                name: "Owner B".to_owned(),
                password: "OwnerBPassword123!".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant B");

    // Create a viewer user in Tenant A for RBAC negative tests.
    {
        let viewer_hash = hash_password_argon2id("ViewerAPassword123!").expect("hash viewer");
        let mut tx = begin_worker_tx(&pool).await.expect("worker tx");
        let viewer_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email, name, password_hash) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind("viewer-a@f015.local")
        .bind("Viewer A")
        .bind(&viewer_hash)
        .fetch_one(&mut *tx)
        .await
        .expect("insert viewer");
        sqlx::query(
            "INSERT INTO memberships (tenant_id, workspace_id, user_id, role) \
             VALUES ($1, NULL, $2, 'viewer')",
        )
        .bind(boot_a.tenant_id)
        .bind(viewer_id)
        .execute(&mut *tx)
        .await
        .expect("insert viewer membership");
        tx.commit().await.expect("commit viewer");
    }

    let state = AppState::new(config, pool.clone());
    let app = build_router(state);

    // Login Owner A, Viewer A, Owner B
    let (st, hdrs, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        None,
        Some(json!({
            "email": "owner-a@f015.local",
            "password": "OwnerAPassword123!",
            "tenant_slug": "f015-tenant-a"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let owner_a_cookie = extract_cookie(&hdrs);
    let owner_a_csrf = parse_json::<AuthSessionView>(&body).csrf_token;

    let (st, hdrs, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        None,
        Some(json!({
            "email": "viewer-a@f015.local",
            "password": "ViewerAPassword123!",
            "tenant_slug": "f015-tenant-a"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let viewer_a_cookie = extract_cookie(&hdrs);
    let viewer_a_csrf = parse_json::<AuthSessionView>(&body).csrf_token;

    let (st, hdrs, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        None,
        Some(json!({
            "email": "owner-b@f015.local",
            "password": "OwnerBPassword123!",
            "tenant_slug": "f015-tenant-b"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let owner_b_cookie = extract_cookie(&hdrs);
    let owner_b_csrf = parse_json::<AuthSessionView>(&body).csrf_token;

    // Create two projects in Tenant A and one in Tenant B.
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "slug": "checkout-api", "name": "Checkout API", "platform": "python" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_a1: ProjectView = parse_json(&body);

    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "slug": "billing-api", "name": "Billing API", "platform": "python" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_a2: ProjectView = parse_json(&body);

    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&owner_b_cookie),
        Some(&owner_b_csrf),
        None,
        Some(json!({ "slug": "tenant-b-api", "name": "Tenant B API", "platform": "python" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_b1: ProjectView = parse_json(&body);

    // Mint a read-only API token and a write-enabled API token scoped to `proj_a1`.
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/api-tokens",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "name": "ro-ci", "project_slug": "checkout-api", "can_write": false })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let ro_api_token = parse_json::<CreatedApiTokenResponse>(&body).raw_token;

    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/api-tokens",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "name": "rw-ci", "project_slug": "checkout-api", "can_write": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let rw_api_token = parse_json::<CreatedApiTokenResponse>(&body).raw_token;

    // 1. Ingest events:
    //    - Issue A (2 events) with rich Sentry breadcrumbs containing sensitive PII fields
    //    - Issue B (2 events) in release `v1.1.0`
    //    - Issue C (1 event) in release `v1.2.0`
    //    - Issue OtherProj (1 event) in proj_a2
    //    - Issue TenantB (1 event) in proj_b1
    let breadcrumbs_payload = json!({
        "breadcrumbs": {
            "values": [
                {
                    "timestamp": 1763790000.25,
                    "type": "navigation",
                    "category": "navigation",
                    "level": "info",
                    "data": { "from": "/cart", "to": "/checkout" }
                },
                {
                    "timestamp": "2026-11-22T05:00:01Z",
                    "type": "http",
                    "category": "fetch",
                    "level": "warning",
                    "message": "POST /api/charge?api_key=super_secret_key returned 502",
                    "data": {
                        "url": "https://pay.internal/charge?token=secret_qs_token",
                        "Authorization": "Bearer super_secret_bearer",
                        "password": "my_plaintext_password",
                        "status_code": 502
                    }
                },
                {
                    "category": "console",
                    "level": "error",
                    "message": "Unhandled gateway error"
                }
            ]
        }
    });

    let a1_ids = (boot_a.tenant_id, boot_a.workspace_id, proj_a1.id);
    let a2_ids = (boot_a.tenant_id, boot_a.workspace_id, proj_a2.id);
    let b1_ids = (boot_b.tenant_id, boot_b.workspace_id, proj_b1.id);

    flush_queued_events(
        &pool,
        vec![
            make_event(
                a1_ids,
                "ev_a_1",
                "v1.0.0",
                (
                    "TypeError",
                    "cannot read property total of null",
                    "compute_cart",
                ),
                breadcrumbs_payload.clone(),
            ),
            make_event(
                a1_ids,
                "ev_a_2",
                "v1.0.0",
                (
                    "TypeError",
                    "cannot read property total of null",
                    "compute_cart",
                ),
                json!({}),
            ),
            make_event(
                a1_ids,
                "ev_b_1",
                "v1.1.0",
                ("ValueError", "invalid promo code ABC", "apply_coupon"),
                json!({}),
            ),
            make_event(
                a1_ids,
                "ev_b_2",
                "v1.1.0",
                ("ValueError", "invalid promo code XYZ", "apply_coupon"),
                json!({}),
            ),
            make_event(
                a1_ids,
                "ev_c_1",
                "v1.2.0",
                ("TimeoutError", "upstream gateway timeout", "call_gateway"),
                json!({}),
            ),
            make_event(
                a2_ids,
                "ev_other_1",
                "v1.0.0",
                ("RuntimeError", "billing worker crashed", "run_billing"),
                json!({}),
            ),
            make_event(
                b1_ids,
                "ev_tb_1",
                "v1.0.0",
                ("KeyError", "missing tenant b key", "load_b"),
                json!({}),
            ),
        ],
        50,
    )
    .await
    .expect("flush initial events");

    // Fetch issues for proj_a1
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=checkout-api",
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let list_a1: PaginatedIssuesResponse = parse_json(&body);
    assert_eq!(list_a1.total, 3);

    let issue_a = list_a1
        .items
        .iter()
        .find(|i| i.exception_type == "TypeError")
        .expect("issue_a")
        .clone();
    let issue_b = list_a1
        .items
        .iter()
        .find(|i| i.exception_type == "ValueError")
        .expect("issue_b")
        .clone();
    let issue_c = list_a1
        .items
        .iter()
        .find(|i| i.exception_type == "TimeoutError")
        .expect("issue_c")
        .clone();

    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=billing-api",
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let issue_other_proj = parse_json::<PaginatedIssuesResponse>(&body).items[0].clone();

    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=tenant-b-api",
        Some(&owner_b_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let issue_tenant_b = parse_json::<PaginatedIssuesResponse>(&body).items[0].clone();

    // 2. Verify F-ISS-10 Breadcrumbs in Issue Detail & Event Detail (including PII scrubbing!)
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{}", issue_a.id),
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let detail_a: IssueDetailView = parse_json(&body);
    assert_eq!(detail_a.breadcrumbs.len(), 3);
    assert_eq!(detail_a.breadcrumbs[0].crumb_type, "navigation");
    assert_eq!(detail_a.breadcrumbs[0].category, "navigation");
    assert!(detail_a.breadcrumbs[0].timestamp.is_some());
    assert_eq!(detail_a.breadcrumbs[1].crumb_type, "http");
    assert_eq!(detail_a.breadcrumbs[1].category, "fetch");
    assert_eq!(detail_a.breadcrumbs[1].level, "warning");
    let http_data = detail_a.breadcrumbs[1].data.as_ref().expect("http data");
    assert_eq!(http_data["Authorization"], "[Filtered]");
    assert_eq!(http_data["password"], "[Filtered]");
    assert!(
        http_data["url"]
            .as_str()
            .unwrap()
            .contains("token=[Filtered]"),
        "expected scrubbed query param in breadcrumb URL, got: {http_data}"
    );
    assert_eq!(detail_a.breadcrumbs[2].category, "console");
    assert_eq!(detail_a.breadcrumbs[2].level, "error");

    let (st, _, body) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{}/events/ev_a_1", issue_a.id),
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ev_a1: IssueEventView = parse_json(&body);
    assert_eq!(ev_a1.breadcrumbs.len(), 3);
    assert_eq!(
        ev_a1.fingerprint.as_deref(),
        Some(issue_a.fingerprint.as_str())
    );

    // Add a triage note on `issue_b` to ensure notes are preserved during merge.
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/notes", issue_b.id),
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "body": "Coupon validation fails on expired promo codes" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 3. Negative merge checks (RBAC, read-only token, cross-project, cross-tenant, self-merge)
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/issues/merge",
        Some(&viewer_a_cookie),
        Some(&viewer_a_csrf),
        None,
        Some(json!({ "target_issue_id": issue_a.id, "source_issue_ids": [issue_b.id] })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/issues/merge",
        None,
        None,
        Some(&ro_api_token),
        Some(json!({ "target_issue_id": issue_a.id, "source_issue_ids": [issue_b.id] })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/issues/merge",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "target_issue_id": issue_a.id, "source_issue_ids": [issue_other_proj.id] })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/issues/merge",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "target_issue_id": issue_a.id, "source_issue_ids": [issue_tenant_b.id] })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/issues/merge",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "target_issue_id": issue_a.id, "source_issue_ids": [issue_a.id] })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 4. Happy-path manual merge: merge [issue_b, issue_c] -> issue_a
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        "/api/v1/issues/merge",
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({
            "target_issue_id": issue_a.id,
            "source_issue_ids": [issue_b.id, issue_c.id]
        })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "merge failed: {}",
        String::from_utf8_lossy(&body)
    );
    let merged: MergeIssuesResponse = parse_json(&body);
    assert_eq!(merged.target_issue.id, issue_a.id);
    assert_eq!(merged.target_issue.event_count, 5);
    assert_eq!(merged.target_issue.stored_full_events, 5);
    assert_eq!(merged.merged_fingerprints.len(), 2);

    // Source issues B and C no longer exist independently.
    let (st, _, _) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{}", issue_b.id),
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Notes from issue_b + merge audit note are both present on issue_a.
    let (st, _, body) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{}/notes", issue_a.id),
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let notes: Vec<IssueNoteView> = parse_json(&body);
    assert!(notes.iter().any(|n| n.body.contains("Coupon validation")));
    assert!(notes.iter().any(|n| n.body.contains("Merged 2 issue(s)")));

    // 5. Future event with issue_b's fingerprint routes automatically into issue_a (no duplicate created)!
    flush_queued_events(
        &pool,
        vec![make_event(
            a1_ids,
            "ev_b_3_after_merge",
            "v1.3.0",
            ("ValueError", "invalid promo code NEW", "apply_coupon"),
            json!({}),
        )],
        50,
    )
    .await
    .expect("flush post-merge event");

    let (st, _, body) = send_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=checkout-api",
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let list_after_merge: PaginatedIssuesResponse = parse_json(&body);
    assert_eq!(
        list_after_merge.total, 1,
        "merged alias must not resurrect duplicate issue"
    );
    assert_eq!(list_after_merge.items[0].id, issue_a.id);
    assert_eq!(list_after_merge.items[0].event_count, 6);
    assert_eq!(
        list_after_merge.items[0].title, issue_a.title,
        "alias routing must preserve target title"
    );

    // 6. Negative split checks (viewer 403, cross-tenant 404, both modes 400, cross-project API token denied)
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/split", issue_a.id),
        Some(&viewer_a_cookie),
        Some(&viewer_a_csrf),
        None,
        Some(json!({ "fingerprints": [issue_b.fingerprint] })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/split", issue_a.id),
        Some(&owner_b_cookie),
        Some(&owner_b_csrf),
        None,
        Some(json!({ "fingerprints": [issue_b.fingerprint] })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/split", issue_a.id),
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "fingerprints": [issue_b.fingerprint], "event_ids": ["ev_b_1"] })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // API token scoped to proj_a1 cannot split proj_a2's issue.
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/split", issue_other_proj.id),
        None,
        None,
        Some(&rw_api_token),
        Some(json!({ "event_ids": ["ev_other_1"] })),
    )
    .await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN,
        "expected cross-project API token split denial, got {st}"
    );

    // Cannot split ALL events out of a single-event issue (`issue_other_proj` has 1 event).
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/split", issue_other_proj.id),
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "event_ids": ["ev_other_1"] })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 7. Split merged fingerprint `issue_b.fingerprint` back out into a standalone issue (using write API token!)
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/split", issue_a.id),
        None,
        None,
        Some(&rw_api_token),
        Some(json!({ "fingerprints": [issue_b.fingerprint] })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "split by fingerprint failed: {}",
        String::from_utf8_lossy(&body)
    );
    let split_fp_res: SplitIssueResponse = parse_json(&body);
    assert_eq!(split_fp_res.created_issues.len(), 1);
    let split_b = &split_fp_res.created_issues[0];
    assert_eq!(split_b.fingerprint, issue_b.fingerprint);
    assert_eq!(
        split_b.event_count, 3,
        "all 3 issue_b events (incl. post-merge) should move to split_b"
    );
    assert_eq!(split_b.stored_full_events, 3);
    assert_eq!(
        split_fp_res.source_issue.event_count, 3,
        "issue_a should retain 2 A + 1 C events"
    );
    assert_eq!(split_fp_res.source_issue.stored_full_events, 3);

    // Future event with `issue_b.fingerprint` now routes to `split_b`, NOT `issue_a`!
    flush_queued_events(
        &pool,
        vec![make_event(
            a1_ids,
            "ev_b_4_after_split",
            "v1.4.0",
            (
                "ValueError",
                "invalid promo code POST_SPLIT",
                "apply_coupon",
            ),
            json!({}),
        )],
        50,
    )
    .await
    .expect("flush post-split event");

    let (st, _, body) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{}", split_b.id),
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let split_b_after: IssueDetailView = parse_json(&body);
    assert_eq!(split_b_after.issue.event_count, 4);

    // 8. Split by specific `event_ids` (`ev_a_2` out of `issue_a`)
    let (st, _, body) = send_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{}/split", issue_a.id),
        Some(&owner_a_cookie),
        Some(&owner_a_csrf),
        None,
        Some(json!({ "event_ids": ["ev_a_2"] })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "split by event_ids failed: {}",
        String::from_utf8_lossy(&body)
    );
    let split_ev_res: SplitIssueResponse = parse_json(&body);
    assert_eq!(split_ev_res.created_issues.len(), 1);
    let split_ev_issue = &split_ev_res.created_issues[0];
    assert!(split_ev_issue.fingerprint.starts_with("split:"));
    assert_eq!(split_ev_issue.event_count, 1);
    assert_eq!(split_ev_res.source_issue.event_count, 2);

    // Confirm `ev_a_2` now lives under `split_ev_issue` and no longer under `issue_a`.
    let (st, _, _) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{}/events/ev_a_2", split_ev_issue.id),
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, _) = send_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{}/events/ev_a_2", issue_a.id),
        Some(&owner_a_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
