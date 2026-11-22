//! F018 — users affected per issue, impact sorting, merge/split carry-over and user-count spikes.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::routing::post;
use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_alerts::delivery::DeliveryPolicy;
use maxi_loop_alerts::engine::evaluate_spike_rules;
use maxi_loop_alerts::model::CreateAlertChannelInput;
use maxi_loop_alerts::service::create_alert_channel;
use maxi_loop_auth::model::Role;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_ingest::model::QueuedEvent;
use maxi_loop_ingest::worker::flush_queued_events;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{connect_db, ensure_upcoming_events_partitions, run_migrations};
use maxi_loop_platform::tenant::{TenantContext, begin_worker_tx};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
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
    tenant: &str,
) -> (String, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"email": email, "password": password, "tenant_slug": tenant}).to_string(),
        ))
        .expect("login request");
    let resp = app.clone().oneshot(req).await.expect("login response");
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .expect("session cookie")
        .to_owned();
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .expect("login body");
    let parsed: Value = serde_json::from_slice(&body).expect("login json");
    let csrf = parsed["csrf_token"].as_str().expect("csrf").to_owned();
    (cookie, csrf)
}

async fn send_authed_json(
    app: &Router,
    method: Method,
    uri: &str,
    session: &(String, String),
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method.clone())
        .uri(uri)
        .header(header::COOKIE, &session.0);
    if method != Method::GET {
        builder = builder.header("x-csrf-token", &session.1);
    }
    let req = match body {
        Some(v) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .expect("json req"),
        None => builder.body(Body::empty()).expect("empty req"),
    };
    let resp = app.clone().oneshot(req).await.expect("api response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("resp bytes");
    let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
}

fn queued(
    tenant_id: i64,
    workspace_id: i64,
    project_id: i64,
    exc_type: &str,
    user: Option<Value>,
) -> QueuedEvent {
    let event_id = uuid_like();
    let mut payload = json!({
        "event_id": event_id,
        "level": "error",
        "platform": "python",
        "exception": {"values": [{
            "type": exc_type,
            "value": "impact test",
            "stacktrace": {"frames": [{"filename": "app/users.py", "function": "handler", "lineno": 7, "in_app": true}]}
        }]}
    });
    if let Some(u) = user {
        payload["user"] = u;
    }
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
        raw_payload: serde_json::to_vec(&payload).expect("payload bytes"),
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let seed = format!(
        "{}-{n}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    hex::encode(&Sha256::digest(seed.as_bytes())[..16])
}

async fn spawn_mock_webhook() -> (u16, Arc<Mutex<Vec<String>>>) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&captured);
    let mock = Router::new().fallback(post(move |body: String| {
        let store = Arc::clone(&store);
        async move {
            store.lock().await.push(body);
            StatusCode::OK
        }
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, mock).await;
    });
    (port, captured)
}

fn issue_by_type<'a>(items: &'a [Value], exc_type: &str) -> &'a Value {
    items
        .iter()
        .find(|i| i["exception_type"] == exc_type)
        .unwrap_or_else(|| panic!("issue {exc_type} listed"))
}

#[tokio::test]
async fn f018_users_affected_sorting_merge_split_and_user_spike_alerts() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect postgres");
    run_migrations(&pool).await.expect("run migrations");
    ensure_upcoming_events_partitions(&pool, Utc::now().date_naive(), 2)
        .await
        .expect("ensure partitions");

    let (http_port, captured) = spawn_mock_webhook().await;
    let policy = DeliveryPolicy {
        allow_private_ips: true,
        telegram_api_base: format!("http://127.0.0.1:{http_port}"),
        timeout: Duration::from_secs(5),
        smtp: None,
    };
    let state = AppState::with_alert_policy(
        config.clone(),
        pool.clone(),
        LoginRateLimiter::new(100, Duration::from_secs(60), 1024),
        policy.clone(),
        Duration::from_secs(3600),
    );
    let app = build_router(state);

    let suffix = Utc::now().timestamp_millis();
    let tenant_slug = format!("f018-{suffix}");
    let email = format!("owner-f018-{suffix}@example.com");
    let pw = "F018Test_StrongPass_1234";
    let outcome = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_slug.clone(),
            tenant_name: "F018".to_owned(),
            retention_days: 30,
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: email.clone(),
                password: pw.to_owned(),
                name: "Owner".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap");
    let session = login_session(&app, &email, pw, &tenant_slug).await;

    let (st, project) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &session,
        Some(
            json!({"slug": "shop", "name": "Shop", "platform": "python", "workspace_slug": "core"}),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let project_id = project["id"].as_i64().expect("project id");
    let workspace_id = project["workspace_id"].as_i64().expect("workspace id");
    let t = outcome.tenant_id;

    // Batch 1: `WideError` hits 3 distinct users (id, email, ip) + repeats + 1 anonymous event;
    // `NarrowError` hits 1 user but many more events.
    let mut batch = vec![
        queued(
            t,
            workspace_id,
            project_id,
            "WideError",
            Some(json!({"id": "u-1"})),
        ),
        queued(
            t,
            workspace_id,
            project_id,
            "WideError",
            Some(json!({"id": 1001})),
        ),
        queued(
            t,
            workspace_id,
            project_id,
            "WideError",
            Some(json!({"id": "u-1", "email": "x@y.z"})),
        ),
        queued(
            t,
            workspace_id,
            project_id,
            "WideError",
            Some(json!({"email": "Alice@Example.com"})),
        ),
        queued(
            t,
            workspace_id,
            project_id,
            "WideError",
            Some(json!({"email": "alice@example.com"})),
        ),
        queued(
            t,
            workspace_id,
            project_id,
            "WideError",
            Some(json!({"ip_address": "{{auto}}"})),
        ),
        queued(t, workspace_id, project_id, "WideError", None),
    ];
    for _ in 0..10 {
        batch.push(queued(
            t,
            workspace_id,
            project_id,
            "NarrowError",
            Some(json!({"id": "u-1"})),
        ));
    }
    flush_queued_events(&pool, batch, 50)
        .await
        .expect("flush 1");

    // Batch 2 (dedupe across batches): `u-1` again + one new user.
    flush_queued_events(
        &pool,
        vec![
            queued(
                t,
                workspace_id,
                project_id,
                "WideError",
                Some(json!({"id": "u-1"})),
            ),
            queued(
                t,
                workspace_id,
                project_id,
                "WideError",
                Some(json!({"username": "bob"})),
            ),
        ],
        50,
    )
    .await
    .expect("flush 2");

    let (st, by_users) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=shop&sort=users",
        &session,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{by_users}");
    let items = by_users["items"].as_array().expect("items").clone();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0]["exception_type"], "WideError",
        "users sort puts widest impact first"
    );
    let wide = issue_by_type(&items, "WideError");
    let narrow = issue_by_type(&items, "NarrowError");
    // u-1, 1001, alice@example.com, bob = 4 users ({{auto}} and anonymous ignored)
    assert_eq!(wide["user_count"], 4);
    assert_eq!(narrow["user_count"], 1);
    assert_eq!(narrow["event_count"], 10);
    let wide_id = wide["id"].as_i64().expect("wide id");
    let narrow_id = narrow["id"].as_i64().expect("narrow id");

    let (st, by_events) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=shop&sort=events",
        &session,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(by_events["items"][0]["exception_type"], "NarrowError");

    let (st, bad) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/issues?sort=users;DROP",
        &session,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");

    // Stored hashes are opaque, fixed-size and never the plain digest of the identity.
    let mut wtx = begin_worker_tx(&pool).await.expect("worker tx");
    let hashes: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT user_hash FROM issue_users WHERE tenant_id = $1 AND issue_id = $2",
    )
    .bind(t)
    .bind(wide_id)
    .fetch_all(&mut *wtx)
    .await
    .expect("issue_users");
    assert_eq!(hashes.len(), 4);
    let plain = Sha256::digest(b"email:alice@example.com");
    for h in &hashes {
        assert_eq!(h.len(), 16);
        assert_ne!(&h[..], &plain[..16]);
    }
    let stored_with_hash: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE tenant_id = $1 AND issue_id = $2 AND user_hash IS NOT NULL",
    )
    .bind(t)
    .bind(wide_id)
    .fetch_one(&mut *wtx)
    .await
    .expect("events user_hash");
    assert_eq!(
        stored_with_hash, 7,
        "5 identified events in batch 1 + 2 in batch 2"
    );
    wtx.commit().await.expect("commit");

    // The tenant API role cannot read the installation key.
    let mut app_tx = maxi_loop_platform::tenant::begin_tenant_tx(
        &pool,
        TenantContext::new(t).expect("tenant ctx"),
    )
    .await
    .expect("tenant tx");
    let denied = sqlx::query_scalar::<_, Vec<u8>>("SELECT secret FROM instance_secrets LIMIT 1")
        .fetch_optional(&mut *app_tx)
        .await;
    assert!(
        denied.is_err(),
        "maxi_loop_app must not read instance_secrets"
    );
    drop(app_tx);

    // Merge: union of users, `u-1` counted once.
    let (st, merged) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/issues/merge",
        &session,
        Some(json!({"target_issue_id": wide_id, "source_issue_ids": [narrow_id]})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{merged}");
    let (st, detail) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{wide_id}"),
        &session,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let detail_issue = if detail.get("issue").is_some() {
        &detail["issue"]
    } else {
        &detail
    };
    assert_eq!(detail_issue["user_count"], 4);
    assert_eq!(detail_issue["event_count"], 19);

    // Split the former NarrowError fingerprint back out: its users follow the moved events.
    let narrow_fp = narrow["fingerprint"].as_str().expect("fp").to_owned();
    let (st, split) = send_authed_json(
        &app,
        Method::POST,
        &format!("/api/v1/issues/{wide_id}/split"),
        &session,
        Some(json!({"fingerprints": [narrow_fp]})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{split}");
    let (_, after_split) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=shop&sort=users",
        &session,
        None,
    )
    .await;
    let after = after_split["items"].as_array().expect("items");
    assert_eq!(after.len(), 2);
    assert_eq!(issue_by_type(after, "NarrowError")["user_count"], 1);
    assert_eq!(issue_by_type(after, "WideError")["user_count"], 4);
    let wide_id = issue_by_type(after, "WideError")["id"]
        .as_i64()
        .expect("id");

    // Spike rule on users: threshold 4 fires for WideError only, and says "affected users".
    let channel = create_alert_channel(
        &pool,
        TenantContext::new(t).expect("tenant ctx"),
        Role::TenantOwner,
        CreateAlertChannelInput {
            name: "F018 hook".to_owned(),
            channel_type: "webhook".to_owned(),
            config: json!({"url": format!("http://127.0.0.1:{http_port}/hook")}),
            workspace_slug: None,
            project_slug: Some("shop".to_owned()),
            is_enabled: Some(true),
        },
        true,
    )
    .await
    .expect("webhook channel");

    let (st, bad_metric) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        &session,
        Some(json!({
            "name": "bad", "channel_id": channel.id, "trigger_type": "spike",
            "spike_threshold": 4, "spike_window_minutes": 60, "spike_metric": "sessions"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{bad_metric}");

    let (st, rule) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        &session,
        Some(json!({
            "name": "User spike", "channel_id": channel.id, "trigger_type": "spike",
            "project_slug": "shop", "spike_threshold": 4, "spike_window_minutes": 60,
            "spike_metric": "users"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{rule}");
    assert_eq!(rule["spike_metric"], "users");

    evaluate_spike_rules(&pool, &config.public_base_url, &policy)
        .await
        .expect("spike eval");
    let bodies = captured.lock().await.clone();
    let ours: Vec<Value> = bodies
        .iter()
        .filter_map(|b| serde_json::from_str::<Value>(b).ok())
        .filter(|v| v["project_id"] == project_id)
        .collect();
    assert_eq!(ours.len(), 1, "exactly one user spike alert: {ours:?}");
    assert_eq!(ours[0]["issue_id"], wide_id);
    assert!(
        ours[0]["details"]
            .as_str()
            .is_some_and(|d| d.starts_with("4 affected users")),
        "{ours:?}"
    );
    assert!(!bodies.iter().any(|b| b.contains("alice@example.com")));
}
