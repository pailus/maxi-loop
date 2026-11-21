use std::time::Duration as StdDuration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::{Days, Duration, Utc};
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::model::AuthSessionView;
use maxi_loop_auth::token::{CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use maxi_loop_ingest::model::QueuedEvent;
use maxi_loop_ingest::worker::flush_queued_events;
use maxi_loop_org::model::{ProjectView, WorkspaceView};
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{connect_db, ensure_events_partition, run_migrations};
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx, begin_worker_tx};
use maxi_loop_retention::model::RetentionConfig;
use maxi_loop_retention::service::run_retention_cycle;
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

async fn partition_exists(pool: &PgPool, part_name: &str) -> bool {
    let regclass = format!("public.{part_name}");
    let mut tx = begin_worker_tx(pool).await.expect("worker tx");
    let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(&regclass)
        .fetch_one(&mut *tx)
        .await
        .expect("query to_regclass");
    tx.commit().await.expect("commit to_regclass");
    exists
}

#[tokio::test]
async fn retention_scheduler_partition_maintenance_drop_partition_and_override_batch_delete() {
    let (config, pool) = setup_test_env().await;

    // Clean up prior test fixtures.
    let mut cleanup_tx = begin_worker_tx(&pool).await.expect("cleanup tx");
    sqlx::query("DELETE FROM tenants WHERE slug = 'f007-retention-tenant'")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f007 tenant");
    sqlx::query("DELETE FROM users WHERE email = 'owner@f007.local'")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f007 user");
    cleanup_tx.commit().await.expect("commit cleanup");

    let spec = BootstrapSpec {
        tenant_slug: "f007-retention-tenant".to_owned(),
        tenant_name: "F007 Retention Tenant".to_owned(),
        retention_days: 30,
        workspace_slug: "core".to_owned(),
        workspace_name: "Core".to_owned(),
        admin_user: Some(AdminUserSpec {
            email: "owner@f007.local".to_owned(),
            password: "f007_owner_password_12345".to_owned(),
            name: "F007 Owner".to_owned(),
        }),
    };
    bootstrap_defaults(&pool, &spec)
        .await
        .expect("bootstrap f007 tenant");

    let app = build_router(AppState::new(config, pool.clone()));

    let (st_login, h_login, b_login) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner@f007.local",
            "password": "f007_owner_password_12345",
            "tenant_slug": "f007-retention-tenant"
        })),
    )
    .await;
    assert_eq!(st_login, StatusCode::OK);
    let cookie = extract_cookie(&h_login);
    let session: AuthSessionView = parse_json(&b_login);

    // 1. Validate workspace & project retention overrides `<= global` (30 days):
    // 1a. Workspace override > 30 days is rejected with 400 Bad Request.
    let (st_ws_invalid, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f007-retention-tenant/workspaces",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "too-long-ws",
            "name": "Too Long WS",
            "retention_days_override": 45
        })),
    )
    .await;
    assert_eq!(st_ws_invalid, StatusCode::BAD_REQUEST);

    // 1b. Workspace override = 14 days (`<= 30`) succeeds.
    let (st_ws_short, _, b_ws_short) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f007-retention-tenant/workspaces",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "short-ws",
            "name": "Short 14d Workspace",
            "retention_days_override": 14
        })),
    )
    .await;
    assert_eq!(st_ws_short, StatusCode::CREATED);
    let ws_short: WorkspaceView = parse_json(&b_ws_short);
    assert_eq!(ws_short.effective_retention_days, 14);

    // 1c. Project inside `short-ws` (14d) attempting override = 21d (`> 14d`) is rejected with 400.
    let (st_proj_invalid, _, _) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f007-retention-tenant/w/short-ws/projects",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "invalid-override-proj",
            "name": "Invalid Override Project",
            "retention_days_override": 21
        })),
    )
    .await;
    assert_eq!(st_proj_invalid, StatusCode::BAD_REQUEST);

    // 1d. Project inside `short-ws` (14d) with project override = 7d (`<= 14d`) succeeds (`proj_7d`).
    let (st_proj_7d, _, b_proj_7d) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f007-retention-tenant/w/short-ws/projects",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "seven-day-app",
            "name": "Seven Day App",
            "retention_days_override": 7
        })),
    )
    .await;
    assert_eq!(st_proj_7d, StatusCode::CREATED);
    let proj_7d: ProjectView = parse_json(&b_proj_7d);
    assert_eq!(proj_7d.effective_retention_days, 7);

    // 1e. Project inside `short-ws` (14d) inheriting workspace 14d override (`proj_14d`).
    let (st_proj_14d, _, b_proj_14d) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f007-retention-tenant/w/short-ws/projects",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "fourteen-day-app",
            "name": "Fourteen Day App"
        })),
    )
    .await;
    assert_eq!(st_proj_14d, StatusCode::CREATED);
    let proj_14d: ProjectView = parse_json(&b_proj_14d);
    assert_eq!(proj_14d.effective_retention_days, 14);

    // 1f. Project inside default `core` workspace using full 30-day global retention (`proj_30d`).
    let (st_proj_30d, _, b_proj_30d) = send_json(
        &app,
        Method::POST,
        "/api/v1/t/f007-retention-tenant/w/core/projects",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "thirty-day-app",
            "name": "Thirty Day App"
        })),
    )
    .await;
    assert_eq!(st_proj_30d, StatusCode::CREATED);
    let proj_30d: ProjectView = parse_json(&b_proj_30d);
    assert_eq!(proj_30d.effective_retention_days, 30);

    // 2. Pre-seed historical daily partitions:
    // - Expired global partitions: `today - 35 days` and `today - 31 days` (must be DROPPED!)
    // - Active partitions within 30-day global retention: `today - 20 days`, `today - 10 days`, `today - 2 days`
    let now = Utc::now();
    let today = now.date_naive();
    let expired_date_35 = today.checked_sub_days(Days::new(35)).unwrap();
    let expired_date_31 = today.checked_sub_days(Days::new(31)).unwrap();
    let active_date_20 = today.checked_sub_days(Days::new(20)).unwrap();
    let active_date_10 = today.checked_sub_days(Days::new(10)).unwrap();

    let expired_part_35 = ensure_events_partition(&pool, expired_date_35)
        .await
        .expect("create -35d partition");
    let expired_part_31 = ensure_events_partition(&pool, expired_date_31)
        .await
        .expect("create -31d partition");
    let active_part_20 = ensure_events_partition(&pool, active_date_20)
        .await
        .expect("create -20d partition");
    let active_part_10 = ensure_events_partition(&pool, active_date_10)
        .await
        .expect("create -10d partition");

    assert!(partition_exists(&pool, &expired_part_35).await);
    assert!(partition_exists(&pool, &expired_part_31).await);
    assert!(partition_exists(&pool, &active_part_20).await);
    assert!(partition_exists(&pool, &active_part_10).await);

    // 3. Seed events into the active partitions to verify shorter override batch deletes:
    // - In `proj_7d` (7-day retention override):
    //   * 3 events at `now - 10 days` (older than 7d override -> MUST BE DELETED across batches!)
    //   * 1 event at `now - 2 days` (newer than 7d override -> MUST BE KEPT!)
    // - In `proj_14d` (14-day workspace retention override):
    //   * 1 event at `now - 20 days` (older than 14d override -> MUST BE DELETED!)
    //   * 1 event at `now - 10 days` (newer than 14d override -> MUST BE KEPT!)
    // - In `proj_30d` (30-day global retention):
    //   * 1 event at `now - 20 days` (newer than 30d global retention -> MUST BE KEPT!)
    //   * 1 event at `now - 10 days` (newer than 30d global retention -> MUST BE KEPT!)
    let ts_10d_ago = now - Duration::days(10);
    let ts_20d_ago = now - Duration::days(20);
    let ts_2d_ago = now - Duration::days(2);

    let mut seed_events = Vec::new();
    for idx in 1..=3 {
        seed_events.push(QueuedEvent {
            tenant_id: proj_7d.tenant_id,
            workspace_id: proj_7d.workspace_id,
            project_id: proj_7d.id,
            event_id: format!("f007_7d_old_{idx}"),
            received_at: ts_10d_ago + Duration::seconds(idx),
            level: "error".to_owned(),
            environment: "production".to_owned(),
            release: None,
            platform: "python".to_owned(),
            raw_payload: serde_json::to_vec(&json!({
                "message": "stale 7d issue that is 10 days old"
            }))
            .unwrap(),
        });
    }
    seed_events.push(QueuedEvent {
        tenant_id: proj_7d.tenant_id,
        workspace_id: proj_7d.workspace_id,
        project_id: proj_7d.id,
        event_id: "f007_7d_fresh_1".to_owned(),
        received_at: ts_2d_ago,
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: None,
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "message": "fresh 7d issue seen 2 days ago"
        }))
        .unwrap(),
    });
    seed_events.push(QueuedEvent {
        tenant_id: proj_14d.tenant_id,
        workspace_id: proj_14d.workspace_id,
        project_id: proj_14d.id,
        event_id: "f007_14d_old_20d".to_owned(),
        received_at: ts_20d_ago,
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: None,
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "message": "stale 14d issue seen 20 days ago"
        }))
        .unwrap(),
    });
    seed_events.push(QueuedEvent {
        tenant_id: proj_14d.tenant_id,
        workspace_id: proj_14d.workspace_id,
        project_id: proj_14d.id,
        event_id: "f007_14d_fresh_10d".to_owned(),
        received_at: ts_10d_ago,
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: None,
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "message": "fresh 14d issue seen 10 days ago"
        }))
        .unwrap(),
    });
    seed_events.push(QueuedEvent {
        tenant_id: proj_30d.tenant_id,
        workspace_id: proj_30d.workspace_id,
        project_id: proj_30d.id,
        event_id: "f007_30d_fresh_20d".to_owned(),
        received_at: ts_20d_ago,
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: None,
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "message": "30d issue seen 20 days ago"
        }))
        .unwrap(),
    });
    seed_events.push(QueuedEvent {
        tenant_id: proj_30d.tenant_id,
        workspace_id: proj_30d.workspace_id,
        project_id: proj_30d.id,
        event_id: "f007_30d_fresh_10d".to_owned(),
        received_at: ts_10d_ago,
        level: "error".to_owned(),
        environment: "production".to_owned(),
        release: None,
        platform: "python".to_owned(),
        raw_payload: serde_json::to_vec(&json!({
            "message": "30d issue seen 10 days ago"
        }))
        .unwrap(),
    });

    flush_queued_events(&pool, seed_events, 50)
        .await
        .expect("flush retention seed events");

    // 4. Execute `run_retention_cycle` with small `batch_delete_limit = 2` to verify multi-batch loop:
    let retention_cfg = RetentionConfig {
        global_retention_days: 30,
        days_ahead: 7,
        batch_delete_limit: 2,
        max_batches_per_run: 5,
        schedule_interval: StdDuration::from_secs(3600),
    };
    let report = run_retention_cycle(&pool, &retention_cfg, now)
        .await
        .expect("run retention cycle");

    // Verify upcoming partitions (`today` .. `today + 7`) were ensured:
    assert_eq!(report.ensured_partitions.len(), 8);
    let future_date_7 = today.checked_add_days(Days::new(7)).unwrap();
    let future_part_7 = format!("events_{}", future_date_7.format("%Y_%m_%d"));
    assert!(partition_exists(&pool, &future_part_7).await);

    // Verify expired partitions (`-35d` and `-31d`) were dropped via DROP TABLE, while `-20d` and `-10d` remain:
    assert!(report.dropped_partitions.contains(&expired_part_35));
    assert!(report.dropped_partitions.contains(&expired_part_31));
    assert!(!partition_exists(&pool, &expired_part_35).await);
    assert!(!partition_exists(&pool, &expired_part_31).await);
    assert!(partition_exists(&pool, &active_part_20).await);
    assert!(partition_exists(&pool, &active_part_10).await);

    // Verify override batch deletes cleaned up the 3 old events in `proj_7d` and 1 old event in `proj_14d`:
    assert_eq!(report.override_deleted_events, 4);
    assert_eq!(report.override_deleted_hourly_counters, 2);
    assert_eq!(report.override_deleted_issues, 2);

    let tenant_ctx = TenantContext::new(proj_7d.tenant_id).expect("tenant ctx");
    let mut tx = begin_tenant_tx(&pool, tenant_ctx).await.expect("tenant tx");

    let proj_7d_events: Vec<String> = sqlx::query_scalar(
        "SELECT event_id FROM events WHERE tenant_id = $1 AND project_id = $2 ORDER BY event_id",
    )
    .bind(proj_7d.tenant_id)
    .bind(proj_7d.id)
    .fetch_all(&mut *tx)
    .await
    .expect("query proj_7d events");
    assert_eq!(proj_7d_events, vec!["f007_7d_fresh_1".to_owned()]);

    let proj_14d_events: Vec<String> = sqlx::query_scalar(
        "SELECT event_id FROM events WHERE tenant_id = $1 AND project_id = $2 ORDER BY event_id",
    )
    .bind(proj_14d.tenant_id)
    .bind(proj_14d.id)
    .fetch_all(&mut *tx)
    .await
    .expect("query proj_14d events");
    assert_eq!(proj_14d_events, vec!["f007_14d_fresh_10d".to_owned()]);

    let proj_30d_events: Vec<String> = sqlx::query_scalar(
        "SELECT event_id FROM events WHERE tenant_id = $1 AND project_id = $2 ORDER BY event_id",
    )
    .bind(proj_30d.tenant_id)
    .bind(proj_30d.id)
    .fetch_all(&mut *tx)
    .await
    .expect("query proj_30d events");
    assert_eq!(
        proj_30d_events,
        vec![
            "f007_30d_fresh_10d".to_owned(),
            "f007_30d_fresh_20d".to_owned()
        ],
        "events in sibling project with 30d retention in the same daily partitions must be preserved"
    );

    tx.commit().await.expect("commit final check");
}
