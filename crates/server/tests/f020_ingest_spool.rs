//! F020: envelopes accepted with `200` survive a crash before the worker flushes them,
//! are replayed exactly once on restart, and checkpointed segments are deleted.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::model::AuthSessionView;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_auth::token::{CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use maxi_loop_ingest::spool::Spool;
use maxi_loop_org::model::ProjectView;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{connect_db, run_migrations};
use maxi_loop_platform::tenant::begin_worker_tx;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tower::ServiceExt;

const TENANT_SLUG: &str = "f020-spool-tenant";
const OWNER_EMAIL: &str = "owner@f020.local";
const OWNER_PASSWORD: &str = "f020_owner_password_12345";

async fn setup_test_env() -> (Config, PgPool) {
    let _ = dotenvy::dotenv();
    let mut config = Config::from_env().expect("DATABASE_URL must be set for integration tests");
    config.public_base_url = "http://127.0.0.1:8080".to_owned();
    config.single_tenant_mode = true;
    config.cookie_secure = false;
    config.ingest_spool_dir = None;
    let pool = connect_db(&config)
        .await
        .expect("connect to test Postgres 16");
    run_migrations(&pool)
        .await
        .expect("apply migrations cleanly");
    (config, pool)
}

async fn request(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(req).await.expect("router response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body bytes")
        .to_vec();
    (status, headers, bytes)
}

async fn create_project(app: &Router) -> ProjectView {
    let login = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "email": OWNER_EMAIL, "password": OWNER_PASSWORD, "tenant_slug": TENANT_SLUG })
                .to_string(),
        ))
        .expect("login request");
    let (status, headers, body) = request(app, login).await;
    assert_eq!(status, StatusCode::OK);
    let set_cookie = headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("Set-Cookie header");
    let cookie = set_cookie
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned();
    let session: AuthSessionView = serde_json::from_slice(&body).expect("session json");
    assert!(cookie.starts_with(SESSION_COOKIE_NAME));

    let create = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/projects")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .header(CSRF_HEADER_NAME, session.csrf_token)
        .body(Body::from(
            json!({ "slug": "spool-app", "name": "Spool App", "platform": "python", "rate_limit_per_minute": 1000 })
                .to_string(),
        ))
        .expect("create project request");
    let (status, _, body) = request(app, create).await;
    assert_eq!(status, StatusCode::CREATED);
    serde_json::from_slice(&body).expect("project json")
}

fn envelope_request(project: &ProjectView, event_id: &str) -> Request<Body> {
    let body = format!(
        "{{\"event_id\":\"{event_id}\"}}\n{{\"type\":\"event\"}}\n{}\n",
        json!({
            "event_id": event_id,
            "level": "error",
            "platform": "python",
            "environment": "production",
            "exception": { "values": [{ "type": "SpoolCrashError", "value": "survives restart" }] }
        })
    );
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/{}/envelope/", project.id))
        .header(header::CONTENT_TYPE, "application/x-sentry-envelope")
        .header(
            "x-sentry-auth",
            format!(
                "Sentry sentry_version=7, sentry_key={}",
                project.dsn_public_key
            ),
        )
        .body(Body::from(body))
        .expect("envelope request")
}

async fn stored_event_count(pool: &PgPool, project_id: i64) -> i64 {
    let mut tx = begin_worker_tx(pool).await.expect("worker tx");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE project_id = $1")
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await
        .expect("count events");
    tx.commit().await.expect("commit");
    count
}

/// The worker task owns a clone of the spool until its channel closes, so wait for the
/// directory lock to be released instead of assuming it happens synchronously.
async fn reopen_when_unlocked(dir: &std::path::Path) -> Spool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match Spool::open(dir) {
            Ok(spool) => return spool,
            Err(err) if Instant::now() < deadline => {
                assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock, "{err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(err) => panic!("spool still locked: {err}"),
        }
    }
}

#[tokio::test]
async fn accepted_envelopes_survive_a_crash_and_are_replayed_exactly_once() {
    let (config, pool) = setup_test_env().await;

    let mut cleanup = begin_worker_tx(&pool).await.expect("cleanup tx");
    sqlx::query("DELETE FROM tenants WHERE slug = $1")
        .bind(TENANT_SLUG)
        .execute(&mut *cleanup)
        .await
        .expect("delete old tenant");
    sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(OWNER_EMAIL)
        .execute(&mut *cleanup)
        .await
        .expect("delete old user");
    cleanup.commit().await.expect("commit cleanup");
    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: TENANT_SLUG.to_owned(),
            tenant_name: "F020 Spool Tenant".to_owned(),
            retention_days: 30,
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: OWNER_EMAIL.to_owned(),
                password: OWNER_PASSWORD.to_owned(),
                name: "F020 Owner".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant");

    let spool_root = std::env::temp_dir().join(format!("maxi-loop-f020-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&spool_root);
    let spool_dir: PathBuf = spool_root.join("spool");

    // Phase 1: accept envelopes, but no worker ever drains the channel, then "crash".
    let project = {
        let (mut state, rx): (AppState, mpsc::Receiver<_>) =
            AppState::with_channel(config.clone(), pool.clone(), LoginRateLimiter::default());
        state.ingest_spool = Some(Spool::open(&spool_dir).expect("open spool"));
        let app = build_router(state);
        let project = create_project(&app).await;
        for event_id in [
            "f0200000000000000000000000000001",
            "f0200000000000000000000000000002",
            "f0200000000000000000000000000003",
        ] {
            let (status, _, body) = request(&app, envelope_request(&project, event_id)).await;
            assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
            let ack: Value = serde_json::from_slice(&body).expect("ingest json");
            assert_eq!(ack["accepted_items"], 1);
        }
        assert_eq!(rx.len(), 3, "accepted envelopes are only in memory");
        project
    };
    assert_eq!(stored_event_count(&pool, project.id).await, 0);

    // Phase 2: restart with a real worker; replay must store every accepted event.
    let spool = reopen_when_unlocked(&spool_dir).await;
    {
        let state = AppState::with_spool(config.clone(), pool.clone(), spool.clone());
        let stats = spool.replay(&state.ingest_tx).await.expect("replay");
        assert_eq!(stats.replayed, 3);
        assert_eq!(stats.undecodable, 0);

        let deadline = Instant::now() + Duration::from_secs(15);
        while stored_event_count(&pool, project.id).await < 3 {
            assert!(Instant::now() < deadline, "replayed events were not stored");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // The worker checkpoints after the flush; the pre-crash segment is then deletable.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            spool.run_maintenance();
            if spool.segment_count().expect("segments") == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "checkpointed segment was not deleted"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    drop(spool);

    // Phase 3: another restart must not replay anything again (no duplicates).
    let spool = reopen_when_unlocked(&spool_dir).await;
    let (tx, mut rx) = mpsc::channel(8);
    let stats = spool.replay(&tx).await.expect("replay");
    assert_eq!(stats.replayed, 0);
    assert!(rx.try_recv().is_err());
    assert_eq!(stored_event_count(&pool, project.id).await, 3);

    drop(spool);
    let _ = std::fs::remove_dir_all(&spool_root);
}
