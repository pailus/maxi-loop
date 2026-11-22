//! F019 — performance alerts: p95 latency and error-rate thresholds per transaction / project,
//! evaluated from `transaction_minute_metrics`, delivered with cooldown + audit log.

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
use maxi_loop_alerts::engine::evaluate_performance_rules;
use maxi_loop_alerts::model::CreateAlertChannelInput;
use maxi_loop_alerts::service::create_alert_channel;
use maxi_loop_auth::model::Role;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_performance::histogram::{NUM_HISTOGRAM_BUCKETS, observe_duration};
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{connect_db, run_migrations};
use maxi_loop_platform::tenant::{TenantContext, begin_worker_tx};
use serde_json::{Value, json};
use sqlx::PgPool;
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
    (
        cookie,
        parsed["csrf_token"].as_str().expect("csrf").to_owned(),
    )
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
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

/// Insert one minute of aggregated transaction metrics (what the ingest worker writes).
async fn insert_minute(
    pool: &PgPool,
    tenant_id: i64,
    project_id: i64,
    transaction: &str,
    durations: &[f64],
    errors: i64,
) {
    let mut buckets = [0i64; NUM_HISTOGRAM_BUCKETS];
    for &d in durations {
        observe_duration(&mut buckets, d);
    }
    let minute = {
        let secs = Utc::now().timestamp();
        chrono::DateTime::<Utc>::from_timestamp(secs - secs.rem_euclid(60), 0).expect("minute")
    };
    let mut tx = begin_worker_tx(pool).await.expect("worker tx");
    sqlx::query(
        "INSERT INTO transaction_minute_metrics (
            tenant_id, project_id, transaction_name, op, environment, release, minute_bucket,
            total_count, error_count, duration_sum_ms, duration_min_ms, duration_max_ms,
            histogram_buckets
         ) VALUES ($1, $2, $3, 'http.server', 'production', '', $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(transaction)
    .bind(minute)
    .bind(i64::try_from(durations.len()).expect("len"))
    .bind(errors)
    .bind(durations.iter().sum::<f64>())
    .bind(durations.iter().copied().fold(f64::MAX, f64::min))
    .bind(durations.iter().copied().fold(0.0, f64::max))
    .bind(buckets.to_vec())
    .execute(&mut *tx)
    .await
    .expect("insert minute metrics");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn f019_p95_and_error_rate_alerts_with_cooldown_audit_and_validation() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect postgres");
    run_migrations(&pool).await.expect("run migrations");

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
    let tenant_slug = format!("f019-{suffix}");
    let email = format!("owner-f019-{suffix}@example.com");
    let pw = "F019Test_StrongPass_1234";
    let outcome = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_slug.clone(),
            tenant_name: "F019".to_owned(),
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
    let t = outcome.tenant_id;

    let (st, project) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &session,
        Some(json!({"slug": "api", "name": "API", "platform": "python", "workspace_slug": "core"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let project_id = project["id"].as_i64().expect("project id");

    // 20 slow checkout calls (5 errors) + 50 fast health calls in the current minute.
    insert_minute(&pool, t, project_id, "POST /checkout", &[800.0; 20], 5).await;
    insert_minute(&pool, t, project_id, "GET /health", &[20.0; 50], 0).await;

    let channel = create_alert_channel(
        &pool,
        TenantContext::new(t).expect("tenant ctx"),
        &Role::TenantOwner,
        CreateAlertChannelInput {
            name: "F019 hook".to_owned(),
            channel_type: "webhook".to_owned(),
            config: json!({"url": format!("http://127.0.0.1:{http_port}/hook")}),
            workspace_slug: None,
            project_slug: Some("api".to_owned()),
            is_enabled: Some(true),
        },
        true,
    )
    .await
    .expect("webhook channel");

    // --- Validation (negative cases) ---
    for bad in [
        json!({"trigger_type": "perf_p95"}),
        json!({"trigger_type": "perf_p95", "perf_threshold": 0.5}),
        json!({"trigger_type": "perf_error_rate", "perf_threshold": 150}),
        json!({"trigger_type": "perf_p95", "perf_threshold": 300, "perf_window_minutes": 0}),
        json!({"trigger_type": "perf_p95", "perf_threshold": 300, "perf_min_count": 0}),
        json!({"trigger_type": "perf_p95", "perf_threshold": 300, "transaction_name": "x".repeat(600)}),
        json!({"trigger_type": "perf_latency", "perf_threshold": 300}),
    ] {
        let mut body = bad.clone();
        body["name"] = json!("bad");
        body["channel_id"] = json!(channel.id);
        let (st, resp) = send_authed_json(
            &app,
            Method::POST,
            "/api/v1/alerts/rules",
            &session,
            Some(body),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad} -> {resp}");
    }

    let mut rule_ids = Vec::new();
    for (name, body) in [
        // fires: project-wide p95 (20/70 at 800ms) > 500ms
        (
            "p95 project",
            json!({"trigger_type": "perf_p95", "perf_threshold": 500, "perf_window_minutes": 5}),
        ),
        // silent: fast transaction only
        (
            "p95 health",
            json!({"trigger_type": "perf_p95", "perf_threshold": 500, "transaction_name": "GET /health"}),
        ),
        // fires: checkout error rate 25% > 5%
        (
            "errors checkout",
            json!({"trigger_type": "perf_error_rate", "perf_threshold": 5, "transaction_name": "POST /checkout"}),
        ),
        // silent: project error rate 7.14% <= 50%
        (
            "errors project",
            json!({"trigger_type": "perf_error_rate", "perf_threshold": 50}),
        ),
        // silent: not enough traffic
        (
            "p95 low traffic",
            json!({"trigger_type": "perf_p95", "perf_threshold": 500, "perf_min_count": 1000}),
        ),
    ] {
        let mut body = body;
        body["name"] = json!(name);
        body["channel_id"] = json!(channel.id);
        body["project_slug"] = json!("api");
        let (st, rule) = send_authed_json(
            &app,
            Method::POST,
            "/api/v1/alerts/rules",
            &session,
            Some(body),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{name}: {rule}");
        rule_ids.push(rule["id"].as_i64().expect("rule id"));
        if name == "p95 health" {
            assert_eq!(rule["transaction_name"], "GET /health");
            assert_eq!(rule["perf_window_minutes"], 5, "default window");
            assert_eq!(rule["perf_min_count"], 10, "default min count");
        }
    }

    // Update validation: perf fields on a non-perf rule, out-of-range threshold.
    let (st, spike) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        &session,
        Some(json!({"name": "spike", "channel_id": channel.id, "trigger_type": "spike", "spike_threshold": 5})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = send_authed_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/alerts/rules/{}", spike["id"]),
        &session,
        Some(json!({"perf_threshold": 100})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send_authed_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/alerts/rules/{}", rule_ids[3]),
        &session,
        Some(json!({"perf_threshold": 100})),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "error rate threshold must be < 100"
    );
    let (st, updated) = send_authed_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/alerts/rules/{}", rule_ids[3]),
        &session,
        Some(json!({"perf_threshold": 60, "perf_window_minutes": 10})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{updated}");
    assert_eq!(updated["perf_threshold"], 60.0);
    assert_eq!(updated["perf_window_minutes"], 10);

    // --- Evaluation ---
    evaluate_performance_rules(&pool, &config.public_base_url, &policy)
        .await
        .expect("perf eval 1");
    let ours = |bodies: &[String]| -> Vec<Value> {
        bodies
            .iter()
            .filter_map(|b| serde_json::from_str::<Value>(b).ok())
            .filter(|v| v["project_id"] == project_id)
            .collect()
    };
    let first = ours(&captured.lock().await);
    assert_eq!(
        first.len(),
        2,
        "p95 project + checkout error rate: {first:?}"
    );

    let p95 = first
        .iter()
        .find(|v| v["event"] == "perf_p95")
        .expect("p95 alert");
    assert!(
        p95.get("issue_id").is_none(),
        "perf alerts carry no issue_id"
    );
    assert_eq!(p95["performance"]["metric"], "p95_ms");
    assert!(p95["performance"]["value"].as_f64().expect("value") > 500.0);
    assert_eq!(p95["performance"]["transaction"], Value::Null);
    assert_eq!(p95["performance"]["transaction_count"], 70);
    assert!(
        p95["issue_url"]
            .as_str()
            .is_some_and(|u| u.ends_with("/?view=performance&project=api"))
    );

    let err = first
        .iter()
        .find(|v| v["event"] == "perf_error_rate")
        .expect("error-rate alert");
    assert_eq!(err["performance"]["value"], 25.0);
    assert_eq!(err["performance"]["transaction"], "POST /checkout");
    assert!(
        err["title"]
            .as_str()
            .is_some_and(|t| t.contains("25.00% > 5%"))
    );

    // Cooldown suppresses repeats.
    evaluate_performance_rules(&pool, &config.public_base_url, &policy)
        .await
        .expect("perf eval 2");
    assert_eq!(
        ours(&captured.lock().await).len(),
        2,
        "cooldown must dedupe"
    );

    // Audit log: deliveries listed without an issue.
    let (st, deliveries) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/alerts/deliveries",
        &session,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{deliveries}");
    let items = deliveries
        .as_array()
        .cloned()
        .unwrap_or_else(|| deliveries["items"].as_array().cloned().unwrap_or_default());
    let perf: Vec<&Value> = items
        .iter()
        .filter(|d| {
            d["trigger_type"]
                .as_str()
                .is_some_and(|t| t.starts_with("perf_"))
        })
        .collect();
    assert_eq!(perf.len(), 2, "{items:?}");
    for d in &perf {
        assert_eq!(d["issue_id"], Value::Null);
        assert_eq!(d["status"], "sent");
        assert!(d["issue_title"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(d["metric_value"].as_f64().is_some());
    }

    // Disabling works; then remove the rules so the shared dev DB's alert engine does not keep
    // evaluating this test tenant.
    let (st, _) = send_authed_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/alerts/rules/{}", rule_ids[0]),
        &session,
        Some(json!({"is_enabled": false})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    for id in rule_ids {
        let (st, _) = send_authed_json(
            &app,
            Method::DELETE,
            &format!("/api/v1/alerts/rules/{id}"),
            &session,
            None,
        )
        .await;
        assert!(st.is_success(), "delete rule {id}: {st}");
    }
}
