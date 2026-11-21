use std::io::Write;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::model::AuthSessionView;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_auth::token::{CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use maxi_loop_ingest::http::X_SENTRY_RATE_LIMITS;
use maxi_loop_ingest::model::EnvelopeIngestResponse;
use maxi_loop_org::model::ProjectView;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
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
    config.max_envelope_compressed_bytes = 4096;
    config.max_envelope_decompressed_bytes = 8192;
    config.max_event_payload_bytes = 2048;
    config.ingest_channel_capacity = 2;

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
    sentry_auth: Option<&str>,
    content_encoding: Option<&str>,
    raw_body: Vec<u8>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-sentry-envelope");

    if let Some(auth) = sentry_auth {
        builder = builder.header("x-sentry-auth", auth);
    }
    if let Some(enc) = content_encoding {
        builder = builder.header(header::CONTENT_ENCODING, enc);
    }

    let response = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(raw_body))
                .expect("build envelope req"),
        )
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
async fn envelope_ingest_dsn_cache_decompression_early_drop_rate_limit_and_backpressure() {
    let (config, pool) = setup_test_env().await;

    // Clean up prior test fixtures.
    let mut cleanup_tx = begin_worker_tx(&pool).await.expect("cleanup tx");
    sqlx::query("DELETE FROM tenants WHERE slug = 'f004-ingest-tenant'")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f004 tenant");
    sqlx::query("DELETE FROM users WHERE email = 'owner@f004.local'")
        .execute(&mut *cleanup_tx)
        .await
        .expect("delete old f004 user");
    cleanup_tx.commit().await.expect("commit cleanup");

    let spec = BootstrapSpec {
        tenant_slug: "f004-ingest-tenant".to_owned(),
        tenant_name: "F004 Ingest Tenant".to_owned(),
        retention_days: 30,
        workspace_slug: "core".to_owned(),
        workspace_name: "Core".to_owned(),
        admin_user: Some(AdminUserSpec {
            email: "owner@f004.local".to_owned(),
            password: "f004_owner_password_12345".to_owned(),
            name: "F004 Owner".to_owned(),
        }),
    };
    bootstrap_defaults(&pool, &spec)
        .await
        .expect("bootstrap f004 tenant");

    let (state, mut ingest_rx) =
        AppState::with_channel(config, pool.clone(), LoginRateLimiter::default());
    let dsn_cache = state.dsn_cache.clone();
    let app = build_router(state);

    // Login as owner@f004.local to create projects via the org API.
    let (st_login, headers_login, body_login) = send_json(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({
            "email": "owner@f004.local",
            "password": "f004_owner_password_12345",
            "tenant_slug": "f004-ingest-tenant"
        })),
    )
    .await;
    assert_eq!(st_login, StatusCode::OK);
    let cookie = extract_cookie(&headers_login);
    let session: AuthSessionView = parse_json(&body_login);

    // Create Project 1 (generous rate limit: 1000/min) and Project 2 (strict rate limit: 2/min).
    let (st_p1, _, body_p1) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "ingest-app",
            "name": "Ingest App",
            "platform": "python-fastapi",
            "rate_limit_per_minute": 1000
        })),
    )
    .await;
    assert_eq!(st_p1, StatusCode::CREATED);
    let proj1: ProjectView = parse_json(&body_p1);

    let (st_p2, _, body_p2) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        Some(&cookie),
        Some(&session.csrf_token),
        Some(json!({
            "slug": "ratelimited-app",
            "name": "Rate Limited App",
            "platform": "go",
            "rate_limit_per_minute": 2
        })),
    )
    .await;
    assert_eq!(st_p2, StatusCode::CREATED);
    let proj2: ProjectView = parse_json(&body_p2);

    // Verify 0 DB reloads were needed because `ProjectDsnObserver` populated `DsnCache` in memory on creation!
    assert_eq!(dsn_cache.db_reload_count(), 0);

    // 1. Unauthorized DSN attempts (unknown project_id, missing key, wrong key) return 401 with 0 DB queries.
    let sample_envelope = concat!(
        "{\"event_id\":\"11111111111111111111111111111111\"}\n",
        "{\"type\":\"event\"}\n",
        "{\"event_id\":\"11111111111111111111111111111111\",\"level\":\"error\",\"environment\":\"production\",\"release\":\"v1.0.0\",\"platform\":\"python\"}\n"
    );

    let (st_bad_key, _, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some("Sentry sentry_version=7, sentry_key=wrong_dsn_key"),
        None,
        sample_envelope.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(st_bad_key, StatusCode::UNAUTHORIZED);

    let (st_unknown_proj, _, _) = send_envelope(
        &app,
        "/api/999999/envelope/",
        Some(&format!(
            "Sentry sentry_version=7, sentry_key={}",
            proj1.dsn_public_key
        )),
        None,
        sample_envelope.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(st_unknown_proj, StatusCode::UNAUTHORIZED);
    assert_eq!(dsn_cache.db_reload_count(), 0);

    // 2. Authenticate via X-Sentry-Auth, ?sentry_key=, and envelope header `dsn`, and test gzip, deflate, and brotli decompression!
    let auth_header_1 = format!(
        "Sentry sentry_version=7, sentry_client=sentry.python/2.0.0, sentry_key={}",
        proj1.dsn_public_key
    );

    // 2a. Gzip-compressed envelope with X-Sentry-Auth header
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(sample_envelope.as_bytes()).unwrap();
    let gz_body = gz.finish().unwrap();

    let (st_gz, _, body_gz) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&auth_header_1),
        Some("gzip"),
        gz_body,
    )
    .await;
    assert_eq!(st_gz, StatusCode::OK);
    let resp_gz: EnvelopeIngestResponse = parse_json(&body_gz);
    assert_eq!(resp_gz.id, "11111111111111111111111111111111");
    assert_eq!(resp_gz.accepted_items, 1);
    assert_eq!(resp_gz.dropped_items, 0);

    let queued_1 = ingest_rx.try_recv().expect("queued gzip envelope");
    assert_eq!(queued_1.tenant_id, proj1.tenant_id);
    assert_eq!(queued_1.project_id, proj1.id);
    assert_eq!(queued_1.events.len(), 1);
    assert_eq!(queued_1.events[0].release.as_deref(), Some("v1.0.0"));

    // 2b. Deflate-compressed envelope authenticated via `?sentry_key=` query parameter
    let mut def = ZlibEncoder::new(Vec::new(), Compression::default());
    def.write_all(sample_envelope.as_bytes()).unwrap();
    let def_body = def.finish().unwrap();

    let (st_def, _, _) = send_envelope(
        &app,
        &format!(
            "/api/{}/envelope/?sentry_key={}",
            proj1.id, proj1.dsn_public_key
        ),
        None,
        Some("deflate"),
        def_body,
    )
    .await;
    assert_eq!(st_def, StatusCode::OK);
    let _ = ingest_rx.try_recv().expect("queued deflate envelope");

    // 2c. Brotli-compressed envelope authenticated via envelope header `"dsn"` field
    let dsn_header_envelope = format!(
        "{{\"event_id\":\"22222222222222222222222222222222\",\"dsn\":\"{}\"}}\n{{\"type\":\"event\"}}\n{{\"message\":\"from brotli\"}}\n",
        proj1.dsn
    );
    let mut br_body = Vec::new();
    {
        let mut br = brotli::CompressorWriter::new(&mut br_body, 4096, 5, 22);
        br.write_all(dsn_header_envelope.as_bytes()).unwrap();
    }
    let (st_br, _, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope", proj1.id),
        None,
        Some("br"),
        br_body,
    )
    .await;
    assert_eq!(st_br, StatusCode::OK);
    let _ = ingest_rx.try_recv().expect("queued brotli envelope");
    assert_eq!(
        dsn_cache.db_reload_count(),
        0,
        "all envelope authentications must come strictly from in-memory DSN cache"
    );

    // 3. Body and Event size limits (413 Payload Too Large):
    // 3a. Decompressed zip-bomb exceeding max_envelope_decompressed_bytes (8192 bytes)
    let large_uncompressed = format!(
        "{{\"event_id\":\"33333333333333333333333333333333\"}}\n{{\"type\":\"session\"}}\n{{\"pad\":\"{}\"}}\n",
        "A".repeat(10_000)
    );
    let mut bomb_gz = GzEncoder::new(Vec::new(), Compression::best());
    bomb_gz.write_all(large_uncompressed.as_bytes()).unwrap();
    let bomb_bytes = bomb_gz.finish().unwrap();
    assert!(
        bomb_bytes.len() < 4096,
        "compressed size is well below 4KB limit, but decompressed exceeds 8KB limit"
    );

    let (st_bomb, _, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&auth_header_1),
        Some("gzip"),
        bomb_bytes,
    )
    .await;
    assert_eq!(st_bomb, StatusCode::PAYLOAD_TOO_LARGE);

    // 3b. Single event item exceeding max_event_payload_bytes (2048 bytes)
    let oversized_event_envelope = format!(
        "{{\"event_id\":\"44444444444444444444444444444444\"}}\n{{\"type\":\"event\"}}\n{{\"message\":\"{}\"}}\n",
        "B".repeat(2500)
    );
    let (st_oversized_ev, _, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&auth_header_1),
        None,
        oversized_event_envelope.into_bytes(),
    )
    .await;
    assert_eq!(st_oversized_ev, StatusCode::PAYLOAD_TOO_LARGE);

    // 4. Early drop of unsupported item types (session, replay_event, profile, attachment):
    let mixed_envelope = concat!(
        "{\"event_id\":\"55555555555555555555555555555555\"}\n",
        "{\"type\":\"session\"}\n",
        "{\"sid\":\"s1\"}\n",
        "{\"type\":\"replay_event\"}\n",
        "{\"replay_id\":\"r1\"}\n",
        "{\"type\":\"profile\"}\n",
        "{\"profile_id\":\"p1\"}\n",
        "{\"type\":\"attachment\",\"length\":8}\n",
        "rawbytes\n",
        "{\"type\":\"event\"}\n",
        "{\"event_id\":\"55555555555555555555555555555555\",\"level\":\"warning\"}\n"
    );
    let (st_mixed, _, body_mixed) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&auth_header_1),
        None,
        mixed_envelope.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(st_mixed, StatusCode::OK);
    let resp_mixed: EnvelopeIngestResponse = parse_json(&body_mixed);
    assert_eq!(resp_mixed.accepted_items, 1);
    assert_eq!(resp_mixed.dropped_items, 4);
    let queued_mixed = ingest_rx.try_recv().expect("queued mixed envelope");
    assert_eq!(queued_mixed.events.len(), 1);
    assert_eq!(queued_mixed.events[0].level, "warning");

    // Envelope with ONLY unsupported items returns 200 OK without enqueuing to worker channel.
    let only_unsupported = concat!(
        "{\"event_id\":\"66666666666666666666666666666666\"}\n",
        "{\"type\":\"session\"}\n",
        "{\"sid\":\"s2\"}\n"
    );
    let (st_only_dropped, _, body_only_dropped) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&auth_header_1),
        None,
        only_unsupported.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(st_only_dropped, StatusCode::OK);
    let resp_only_dropped: EnvelopeIngestResponse = parse_json(&body_only_dropped);
    assert_eq!(resp_only_dropped.accepted_items, 0);
    assert_eq!(resp_only_dropped.dropped_items, 1);
    assert!(ingest_rx.try_recv().is_err());

    // 5. Bounded channel backpressure: channel capacity is 2. Fill 2 slots without draining, then verify 3rd returns 429!
    for _ in 0..2 {
        let (st, _, _) = send_envelope(
            &app,
            &format!("/api/{}/envelope/", proj1.id),
            Some(&auth_header_1),
            None,
            sample_envelope.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }

    let (st_full, headers_full, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&auth_header_1),
        None,
        sample_envelope.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(
        st_full,
        StatusCode::TOO_MANY_REQUESTS,
        "full bounded channel must return 429 backpressure"
    );
    assert_eq!(
        headers_full
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
        Some("5")
    );
    assert_eq!(
        headers_full
            .get(X_SENTRY_RATE_LIMITS)
            .and_then(|v| v.to_str().ok()),
        Some("5:error:organization")
    );

    // Drain the 2 queued envelopes so the channel has space again.
    let _ = ingest_rx.try_recv().expect("drain slot 1");
    let _ = ingest_rx.try_recv().expect("drain slot 2");

    // 6. Per-project `governor` rate limit on Project 2 (rate_limit_per_minute = 2):
    let auth_header_2 = format!(
        "Sentry sentry_version=7, sentry_key={}",
        proj2.dsn_public_key
    );
    for _ in 0..2 {
        let (st, _, _) = send_envelope(
            &app,
            &format!("/api/{}/envelope/", proj2.id),
            Some(&auth_header_2),
            None,
            sample_envelope.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let _ = ingest_rx.try_recv().expect("drain proj2 event");
    }

    let (st_proj_rl, headers_proj_rl, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj2.id),
        Some(&auth_header_2),
        None,
        sample_envelope.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(st_proj_rl, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers_proj_rl.contains_key(header::RETRY_AFTER));
    let sentry_rl_val = headers_proj_rl
        .get(X_SENTRY_RATE_LIMITS)
        .and_then(|v| v.to_str().ok())
        .expect("X-Sentry-Rate-Limits header");
    assert!(sentry_rl_val.ends_with(":error:project"));

    // 7. Live DSN rotation updates the in-memory DsnCache immediately (0 DB queries on ingest):
    let (st_rot, _, body_rot) = send_json(
        &app,
        Method::POST,
        "/api/v1/projects/ingest-app/dsn/rotate",
        Some(&cookie),
        Some(&session.csrf_token),
        None,
    )
    .await;
    assert_eq!(st_rot, StatusCode::OK);
    let rotated_proj1: ProjectView = parse_json(&body_rot);

    // Old DSN key is immediately rejected from memory:
    let (st_old_key, _, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&auth_header_1),
        None,
        sample_envelope.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(st_old_key, StatusCode::UNAUTHORIZED);

    // New DSN key is immediately accepted from memory:
    let new_auth_header_1 = format!(
        "Sentry sentry_version=7, sentry_key={}",
        rotated_proj1.dsn_public_key
    );
    let (st_new_key, headers_new_key, _) = send_envelope(
        &app,
        &format!("/api/{}/envelope/", proj1.id),
        Some(&new_auth_header_1),
        None,
        sample_envelope.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(st_new_key, StatusCode::OK);
    assert_eq!(
        headers_new_key
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    assert!(
        headers_new_key
            .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("X-Sentry-Rate-Limits") && v.contains("Retry-After"))
    );
    assert!(
        !headers_new_key.contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
        "wildcard CORS on ingest must never allow credentials"
    );
    assert_eq!(dsn_cache.db_reload_count(), 0);

    // 8. Browser SDK CORS preflight on `/api/{id}/envelope/` succeeds without touching `/api/v1/*`:
    let preflight_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri(format!("/api/{}/envelope/", proj1.id))
                .header(header::ORIGIN, "https://app.example.com")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "content-type, x-sentry-auth",
                )
                .body(Body::empty())
                .expect("build preflight request"),
        )
        .await
        .expect("preflight response");
    assert_eq!(preflight_resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        preflight_resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    assert!(
        preflight_resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("X-Sentry-Auth"))
    );
    assert!(
        !preflight_resp
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
    );

    // Cookie-authenticated `/api/v1/*` routes must NEVER emit wildcard CORS:
    let (st_health, headers_health, _) =
        send_json(&app, Method::GET, "/api/v1/health", None, None, None).await;
    assert_eq!(st_health, StatusCode::OK);
    assert!(!headers_health.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
}
