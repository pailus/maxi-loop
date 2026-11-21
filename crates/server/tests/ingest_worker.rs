use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::model::AuthSessionView;
use maxi_loop_auth::token::{CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use maxi_loop_ingest::model::QueuedEvent;
use maxi_loop_ingest::scrub::FILTERED_VALUE;
use maxi_loop_ingest::worker::flush_queued_events;
use maxi_loop_org::model::ProjectView;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{connect_db, run_migrations};
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx, begin_worker_tx};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn setup_test_env(max_full_events_per_issue: u32) -> (Config, PgPool) {
    let _ = dotenvy::dotenv();
    let mut config = Config::from_env().expect("DATABASE_URL must be set for integration tests");
    config.public_base_url = "http://127.0.0.1:8080".to_owned();
    config.single_tenant_mode = true;
    config.cookie_secure = false;
    config.retention_default_days = 30;
    config.max_full_events_per_issue = max_full_events_per_issue;
    config.ingest_channel_capacity = 128;

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

async fn send_envelope(
    app: &Router,
    uri: &str,
    sentry_auth: &str,
    raw_body: Vec<u8>,
) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-sentry-envelope")
        .header("x-sentry-auth", sentry_auth)
        .body(Body::from(raw_body))
        .expect("build envelope req");

    let response = app.clone().oneshot(req).await.expect("router response");
    response.status()
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

#[derive(Debug, sqlx::FromRow)]
struct IssueRecord {
    id: i64,
    fingerprint: String,
    title: String,
    exception_type: String,
    culprit: String,
    status: String,
    is_regressed: bool,
    event_count: i64,
    stored_full_events: i32,
    last_environment: String,
    last_release: Option<String>,
    sample_event_id: Option<String>,
}

#[tokio::test]
async fn worker_pii_scrubbing_grouping_regression_reopen_event_cap_and_hourly_counters() {
    let max_full_cap = 3u32;
    let (config, pool) = setup_test_env(max_full_cap).await;

    // Clean up prior test fixtures.
    let mut cleanup_tx = begin_worker_tx(&pool).await.expect("cleanup tx");
    sqlx::query("DELETE FROM tenants WHERE slug = 'f005-worker-tenant'")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f005 tenant");
    sqlx::query("DELETE FROM users WHERE email = 'owner@f005.local'")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f005 user");
    cleanup_tx.commit().await.expect("commit cleanup");

    let spec = BootstrapSpec {
        tenant_slug: "f005-worker-tenant".to_owned(),
        tenant_name: "F005 Worker Tenant".to_owned(),
        retention_days: 30,
        workspace_slug: "core".to_owned(),
        workspace_name: "Core".to_owned(),
        admin_user: Some(AdminUserSpec {
            email: "owner@f005.local".to_owned(),
            password: "f005_owner_password_12345".to_owned(),
            name: "F005 Owner".to_owned(),
        }),
    };
    bootstrap_defaults(&pool, &spec)
        .await
        .expect("bootstrap f005 tenant");

    let state = AppState::new(config, pool.clone());
    let app = build_router(state);

    let (st_login, headers_login, body_login) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner@f005.local",
            "password": "f005_owner_password_12345",
            "tenant_slug": "f005-worker-tenant"
        })),
    )
    .await;
    assert_eq!(st_login, StatusCode::OK);
    let cookie = extract_cookie(&headers_login);
    let session: AuthSessionView = parse_json(&body_login);

    let (st_proj, _, body_proj) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "worker-api",
            "name": "Worker API",
            "platform": "python-fastapi",
            "rate_limit_per_minute": 5000
        })),
    )
    .await;
    assert_eq!(st_proj, StatusCode::CREATED);
    let proj: ProjectView = parse_json(&body_proj);
    let tenant_ctx = TenantContext::new(proj.tenant_id).expect("valid tenant ctx");

    // 1. Live end-to-end envelope HTTP ingest -> bounded channel -> background worker -> Postgres:
    // Include sensitive headers (`Authorization`, `Cookie`) and fields (`password`, `token`, `secret`, `api_key`)
    // to verify default PII scrubbing before persistence.
    let auth_header = format!(
        "Sentry sentry_version=7, sentry_client=sentry.python/2.0.0, sentry_key={}",
        proj.dsn_public_key
    );
    let pii_event_payload = json!({
        "event_id": "aaaa1111aaaa1111aaaa1111aaaa1111",
        "level": "error",
        "environment": "production",
        "release": "sha-1000001",
        "platform": "python",
        "exception": {
            "values": [{
                "type": "DatabaseOperationalError",
                "value": "connection failed for user 101",
                "stacktrace": {
                    "frames": [
                        {
                            "filename": "site-packages/sqlalchemy/engine.py",
                            "module": "sqlalchemy.engine",
                            "function": "connect",
                            "lineno": 50,
                            "in_app": false
                        },
                        {
                            "filename": "app/db/session.py",
                            "module": "app.db.session",
                            "function": "acquire_connection",
                            "lineno": 42,
                            "in_app": true,
                            "context_line": "return engine.connect()"
                        }
                    ]
                }
            }]
        },
        "request": {
            "url": "https://api.example.com/checkout?api_key=raw_url_api_key_999&item=book",
            "headers": {
                "Authorization": "Bearer super_secret_jwt_token_value",
                "Cookie": "session_token=raw_cookie_secret_value",
                "User-Agent": "FastAPIClient/1.0"
            },
            "data": {
                "password": "plain_text_user_password",
                "token": "plain_text_reset_token",
                "secret": "plain_text_webhook_secret",
                "api_key": "plain_text_body_api_key",
                "customer_name": "Budi"
            }
        },
        "tags": [
            ["service", "checkout"],
            ["access_token", "tag_secret_access_token"]
        ]
    });

    let envelope_1 = format!(
        "{{\"event_id\":\"aaaa1111aaaa1111aaaa1111aaaa1111\"}}\n{{\"type\":\"event\"}}\n{}\n",
        serde_json::to_string(&pii_event_payload).unwrap()
    );
    let st_env1 = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj.id),
        &auth_header,
        envelope_1.into_bytes(),
    )
    .await;
    assert_eq!(st_env1, StatusCode::OK);

    // Wait briefly for the 100ms background batch worker to flush to PostgreSQL.
    let mut stored_payload: Option<Value> = None;
    let mut stored_tags: Option<Value> = None;
    for _ in 0..30 {
        let mut tx = begin_tenant_tx(&pool, tenant_ctx).await.expect("tenant tx");
        let row: Option<(Value, Value)> = sqlx::query_as(
            "SELECT payload, tags FROM events WHERE tenant_id = $1 AND project_id = $2 AND event_id = 'aaaa1111aaaa1111aaaa1111aaaa1111'",
        )
        .bind(proj.tenant_id)
        .bind(proj.id)
        .fetch_optional(&mut *tx)
        .await
        .expect("query stored event");
        tx.commit().await.expect("commit read tx");

        if let Some((p, t)) = row {
            stored_payload = Some(p);
            stored_tags = Some(t);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let persisted_payload =
        stored_payload.expect("background worker must persist event within 750ms");
    let persisted_tags = stored_tags.expect("persisted tags");
    let raw_persisted_str = serde_json::to_string(&persisted_payload).unwrap();

    // Verify NO raw secrets ever reached PostgreSQL!
    assert!(!raw_persisted_str.contains("super_secret_jwt_token_value"));
    assert!(!raw_persisted_str.contains("raw_cookie_secret_value"));
    assert!(!raw_persisted_str.contains("plain_text_user_password"));
    assert!(!raw_persisted_str.contains("plain_text_reset_token"));
    assert!(!raw_persisted_str.contains("plain_text_webhook_secret"));
    assert!(!raw_persisted_str.contains("plain_text_body_api_key"));
    assert!(!raw_persisted_str.contains("raw_url_api_key_999"));
    assert!(!raw_persisted_str.contains("tag_secret_access_token"));

    assert_eq!(
        persisted_payload["request"]["headers"]["Authorization"],
        FILTERED_VALUE
    );
    assert_eq!(
        persisted_payload["request"]["headers"]["Cookie"],
        FILTERED_VALUE
    );
    assert_eq!(
        persisted_payload["request"]["headers"]["User-Agent"],
        "FastAPIClient/1.0"
    );
    assert_eq!(
        persisted_payload["request"]["data"]["password"],
        FILTERED_VALUE
    );
    assert_eq!(
        persisted_payload["request"]["data"]["token"],
        FILTERED_VALUE
    );
    assert_eq!(
        persisted_payload["request"]["data"]["secret"],
        FILTERED_VALUE
    );
    assert_eq!(
        persisted_payload["request"]["data"]["api_key"],
        FILTERED_VALUE
    );
    assert_eq!(
        persisted_payload["request"]["data"]["customer_name"],
        "Budi"
    );
    assert_eq!(persisted_tags["service"], "checkout");
    assert_eq!(persisted_tags["access_token"], FILTERED_VALUE);

    // 2. Grouping consistency & Event Cap (`max_full_events_per_issue = 3`):
    // Send 9 more events with the same exception type (`DatabaseOperationalError`) and same in-app frame
    // (`app.db.session` / `acquire_connection`), but different user IDs and different line numbers.
    // Total occurrences for this issue = 1 (already persisted) + 9 = 10.
    let now = Utc::now();
    let mut batch_events = Vec::new();
    for i in 2..=10 {
        let ev_payload = json!({
            "event_id": format!("aaaa{i:04}aaaa{i:04}aaaa{i:04}aaaa{i:04}"),
            "exception": {
                "values": [{
                    "type": "DatabaseOperationalError",
                    "value": format!("connection failed for user {}", 100 + i),
                    "stacktrace": {
                        "frames": [
                            {
                                "filename": "site-packages/sqlalchemy/pool.py",
                                "module": "sqlalchemy.pool",
                                "function": "checkout",
                                "lineno": 100 + i,
                                "in_app": false
                            },
                            {
                                "filename": "app/db/session.py",
                                "module": "app.db.session",
                                "function": "acquire_connection",
                                "lineno": 40 + i,
                                "in_app": true
                            }
                        ]
                    }
                }]
            }
        });
        batch_events.push(QueuedEvent {
            tenant_id: proj.tenant_id,
            workspace_id: proj.workspace_id,
            project_id: proj.id,
            event_id: format!("aaaa{i:04}aaaa{i:04}aaaa{i:04}aaaa{i:04}"),
            received_at: now + chrono::Duration::seconds(i),
            level: "error".to_owned(),
            environment: "production".to_owned(),
            release: Some("sha-1000001".to_owned()),
            platform: "python".to_owned(),
            raw_payload: serde_json::to_vec(&ev_payload).unwrap(),
        });
    }

    let stats = flush_queued_events(&pool, batch_events, max_full_cap)
        .await
        .expect("flush 9 more events");
    assert_eq!(stats.processed_events, 9);
    assert_eq!(stats.upserted_issues, 1);
    assert_eq!(
        stats.inserted_full_events, 2,
        "1 event was already stored and cap is 3, so only 2 more full events should be stored"
    );

    let mut tx = begin_tenant_tx(&pool, tenant_ctx).await.expect("tenant tx");
    let issue: IssueRecord = sqlx::query_as(
        "SELECT id, fingerprint, title, exception_type, culprit, status, is_regressed, \
                event_count, stored_full_events, last_environment, last_release, sample_event_id \
         FROM issues WHERE tenant_id = $1 AND project_id = $2",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .fetch_one(&mut *tx)
    .await
    .expect("fetch grouped issue");

    assert_eq!(issue.exception_type, "DatabaseOperationalError");
    assert_eq!(issue.culprit, "app.db.session in acquire_connection");
    assert_eq!(issue.status, "unresolved");
    assert!(!issue.is_regressed);
    assert_eq!(issue.event_count, 10);
    assert_eq!(issue.stored_full_events, 3);
    assert_eq!(
        issue.sample_event_id.as_deref(),
        Some("aaaa1111aaaa1111aaaa1111aaaa1111")
    );

    let stored_event_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .bind(issue.id)
    .fetch_one(&mut *tx)
    .await
    .expect("count stored events");
    assert_eq!(
        stored_event_rows, 3,
        "full event payloads must be capped at max_full_events_per_issue (3)"
    );

    let hourly_total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(event_count)::BIGINT, 0) FROM issue_hourly_counters \
         WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .bind(issue.id)
    .fetch_one(&mut *tx)
    .await
    .expect("sum hourly counters");
    assert_eq!(
        hourly_total, 10,
        "hourly counters must count all 10 occurrences even after full-event cap is reached"
    );

    // 3. Regression auto-reopen + extra sample capture after full-event cap:
    // Mark the issue as `resolved`, then ingest an 11th occurrence in a new release (`sha-2000002`).
    sqlx::query(
        "UPDATE issues SET status = 'resolved' WHERE tenant_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .bind(issue.id)
    .execute(&mut *tx)
    .await
    .expect("mark issue resolved");
    tx.commit().await.expect("commit resolve");

    let regression_payload = json!({
        "event_id": "bbbb9999bbbb9999bbbb9999bbbb9999",
        "exception": {
            "values": [{
                "type": "DatabaseOperationalError",
                "value": "connection failed again after deploy",
                "stacktrace": {
                    "frames": [{
                        "filename": "app/db/session.py",
                        "module": "app.db.session",
                        "function": "acquire_connection",
                        "lineno": 55,
                        "in_app": true
                    }]
                }
            }]
        }
    });
    let reg_stats = flush_queued_events(
        &pool,
        vec![QueuedEvent {
            tenant_id: proj.tenant_id,
            workspace_id: proj.workspace_id,
            project_id: proj.id,
            event_id: "bbbb9999bbbb9999bbbb9999bbbb9999".to_owned(),
            received_at: now + chrono::Duration::seconds(60),
            level: "fatal".to_owned(),
            environment: "production".to_owned(),
            release: Some("sha-2000002".to_owned()),
            platform: "python".to_owned(),
            raw_payload: serde_json::to_vec(&regression_payload).unwrap(),
        }],
        max_full_cap,
    )
    .await
    .expect("flush regression event");

    assert_eq!(reg_stats.regressed_issues, 1);
    assert_eq!(
        reg_stats.inserted_full_events, 1,
        "regression after reaching cap stores 1 extra sample event (N + sample)"
    );

    let mut tx = begin_tenant_tx(&pool, tenant_ctx).await.expect("tenant tx");
    let regressed_issue: IssueRecord = sqlx::query_as(
        "SELECT id, fingerprint, title, exception_type, culprit, status, is_regressed, \
                event_count, stored_full_events, last_environment, last_release, sample_event_id \
         FROM issues WHERE tenant_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .bind(issue.id)
    .fetch_one(&mut *tx)
    .await
    .expect("fetch regressed issue");

    assert_eq!(
        regressed_issue.status, "unresolved",
        "resolved issue must auto-reopen to unresolved on new occurrence"
    );
    assert!(
        regressed_issue.is_regressed,
        "reopened issue must be flagged is_regressed = true"
    );
    assert_eq!(regressed_issue.event_count, 11);
    assert_eq!(regressed_issue.stored_full_events, 3);
    assert_eq!(regressed_issue.last_release.as_deref(), Some("sha-2000002"));
    assert_eq!(
        regressed_issue.sample_event_id.as_deref(),
        Some("bbbb9999bbbb9999bbbb9999bbbb9999")
    );
    assert_eq!(
        regressed_issue.title,
        "DatabaseOperationalError: connection failed again after deploy"
    );

    let total_events_plus_sample: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE tenant_id = $1 AND project_id = $2 AND issue_id = $3",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .bind(issue.id)
    .fetch_one(&mut *tx)
    .await
    .expect("count events after regression sample");
    assert_eq!(
        total_events_plus_sample, 4,
        "3 capped full events + 1 regression sample event = 4 rows"
    );
    tx.commit().await.expect("commit regression check");

    // 4. Verify `ignored` issues stay `ignored` (do not reopen) and message fallback grouping works:
    let msg_ev_1 = QueuedEvent {
        tenant_id: proj.tenant_id,
        workspace_id: proj.workspace_id,
        project_id: proj.id,
        event_id: "cccc1111cccc1111cccc1111cccc1111".to_owned(),
        received_at: now,
        level: "warning".to_owned(),
        environment: "staging".to_owned(),
        release: None,
        platform: "go".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "message": "Upstream payment gateway timeout after 1500 ms for order 9001"
        }))
        .unwrap(),
    };
    flush_queued_events(&pool, vec![msg_ev_1], max_full_cap)
        .await
        .expect("flush msg_ev_1");

    let mut tx = begin_tenant_tx(&pool, tenant_ctx).await.expect("tenant tx");
    sqlx::query(
        "UPDATE issues SET status = 'ignored' \
         WHERE tenant_id = $1 AND project_id = $2 AND id <> $3",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .bind(issue.id)
    .execute(&mut *tx)
    .await
    .expect("mark second issue ignored");
    tx.commit().await.expect("commit ignore");

    let msg_ev_2 = QueuedEvent {
        tenant_id: proj.tenant_id,
        workspace_id: proj.workspace_id,
        project_id: proj.id,
        event_id: "cccc2222cccc2222cccc2222cccc2222".to_owned(),
        received_at: now + chrono::Duration::seconds(10),
        level: "warning".to_owned(),
        environment: "staging".to_owned(),
        release: None,
        platform: "go".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "message": "Upstream payment gateway timeout after 2200 ms for order 9002"
        }))
        .unwrap(),
    };
    flush_queued_events(&pool, vec![msg_ev_2], max_full_cap)
        .await
        .expect("flush msg_ev_2");

    let mut tx = begin_tenant_tx(&pool, tenant_ctx).await.expect("tenant tx");
    let ignored_issue: IssueRecord = sqlx::query_as(
        "SELECT id, fingerprint, title, exception_type, culprit, status, is_regressed, \
                event_count, stored_full_events, last_environment, last_release, sample_event_id \
         FROM issues WHERE tenant_id = $1 AND project_id = $2 AND id <> $3",
    )
    .bind(proj.tenant_id)
    .bind(proj.id)
    .bind(issue.id)
    .fetch_one(&mut *tx)
    .await
    .expect("fetch ignored message-grouped issue");
    tx.commit().await.expect("commit final check");

    assert_ne!(ignored_issue.fingerprint, issue.fingerprint);
    assert_eq!(ignored_issue.status, "ignored");
    assert!(!ignored_issue.is_regressed);
    assert_eq!(ignored_issue.event_count, 2);
    assert_eq!(ignored_issue.last_environment, "staging");
}
