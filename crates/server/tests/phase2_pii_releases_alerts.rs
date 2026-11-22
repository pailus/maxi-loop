use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::routing::post;
use hmac::{Hmac, Mac};
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_alerts::delivery::DeliveryPolicy;
use maxi_loop_alerts::engine::evaluate_spike_rules;
use maxi_loop_alerts::model::CreateAlertChannelInput;
use maxi_loop_alerts::service::create_alert_channel;
use maxi_loop_auth::model::Role;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::{Config, SmtpConfig};
use maxi_loop_platform::db::{connect_db, ensure_upcoming_events_partitions, run_migrations};
use maxi_loop_platform::tenant::TenantContext;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tower::ServiceExt;

type HmacSha256 = Hmac<Sha256>;

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
) -> (String, String) {
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
        .expect("login request");

    let resp = app.clone().oneshot(req).await.expect("login response");
    assert_eq!(resp.status(), StatusCode::OK);

    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie header")
        .to_str()
        .expect("cookie utf8")
        .split(';')
        .next()
        .expect("cookie kv")
        .to_owned();

    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .expect("login body");
    let parsed: Value = serde_json::from_slice(&body).expect("login json");
    let csrf = parsed["csrf_token"]
        .as_str()
        .expect("csrf_token")
        .to_owned();
    (set_cookie, csrf)
}

async fn send_authed_json(
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

    let resp = app.clone().oneshot(req).await.expect("api response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .expect("resp bytes");
    let json_val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json_val)
}

async fn send_envelope(app: &Router, project_id: i64, dsn_key: &str, event: Value) {
    let event_id = event["event_id"]
        .as_str()
        .unwrap_or("11111111111111111111111111111111");
    let header_line = json!({"event_id": event_id}).to_string();
    let event_str = event.to_string();
    let item_header = json!({"type": "event", "length": event_str.len()}).to_string();
    let envelope = format!("{header_line}\n{item_header}\n{event_str}\n");

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/{project_id}/envelope/"))
        .header(
            "x-sentry-auth",
            format!("Sentry sentry_version=7, sentry_key={dsn_key}"),
        )
        .header(header::CONTENT_TYPE, "application/x-sentry-envelope")
        .body(Body::from(envelope))
        .expect("envelope req");

    let resp = app.clone().oneshot(req).await.expect("envelope resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[derive(Clone, Debug)]
struct CapturedHttp {
    path: String,
    signature: Option<String>,
    event_header: Option<String>,
    body: String,
}

async fn spawn_mock_http_receiver() -> (u16, Arc<Mutex<Vec<CapturedHttp>>>) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&captured);

    let mock_app = Router::new().fallback(post(
        move |uri: axum::http::Uri, headers: HeaderMap, body: String| {
            let store = Arc::clone(&store);
            async move {
                store.lock().await.push(CapturedHttp {
                    path: uri.path().to_owned(),
                    signature: headers
                        .get("x-maxiloop-signature")
                        .and_then(|v| v.to_str().ok())
                        .map(ToOwned::to_owned),
                    event_header: headers
                        .get("x-maxiloop-event")
                        .and_then(|v| v.to_str().ok())
                        .map(ToOwned::to_owned),
                    body,
                });
                StatusCode::OK
            }
        },
    ));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock http");
    let port = listener.local_addr().expect("mock addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, mock_app).await;
    });
    (port, captured)
}

async fn spawn_mock_smtp_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&messages);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock smtp");
    let port = listener.local_addr().expect("smtp addr").port();

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let _ = reader.get_mut().write_all(b"220 mock-smtp ready\r\n").await;
                let mut line = String::new();
                let mut in_data = false;
                let mut data_buf = String::new();
                loop {
                    line.clear();
                    let Ok(n) = reader.read_line(&mut line).await else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    if in_data {
                        if line == ".\r\n" || line == ".\n" {
                            in_data = false;
                            store.lock().await.push(data_buf.clone());
                            data_buf.clear();
                            let _ = reader.get_mut().write_all(b"250 OK queued\r\n").await;
                        } else {
                            data_buf.push_str(&line);
                        }
                        continue;
                    }
                    let upper = line.trim().to_ascii_uppercase();
                    if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                        let _ = reader.get_mut().write_all(b"250 mock-smtp\r\n").await;
                    } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
                        let _ = reader.get_mut().write_all(b"250 OK\r\n").await;
                    } else if upper == "DATA" {
                        in_data = true;
                        let _ = reader
                            .get_mut()
                            .write_all(b"354 Start mail input\r\n")
                            .await;
                    } else if upper == "QUIT" {
                        let _ = reader.get_mut().write_all(b"221 Bye\r\n").await;
                        break;
                    } else {
                        let _ = reader.get_mut().write_all(b"250 OK\r\n").await;
                    }
                }
            });
        }
    });

    (port, messages)
}

#[tokio::test]
async fn phase2_configurable_pii_releases_alerts_ssrf_and_tenant_isolation() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect postgres");
    run_migrations(&pool).await.expect("run migrations");
    ensure_upcoming_events_partitions(&pool, chrono::Utc::now().date_naive(), 2)
        .await
        .expect("ensure partitions");

    let (http_port, captured_http) = spawn_mock_http_receiver().await;
    let (smtp_port, captured_emails) = spawn_mock_smtp_server().await;

    let test_policy = DeliveryPolicy {
        allow_private_ips: true,
        telegram_api_base: format!("http://127.0.0.1:{http_port}"),
        timeout: Duration::from_secs(5),
        smtp: Some(SmtpConfig {
            host: "127.0.0.1".to_owned(),
            port: smtp_port,
            from_address: "alerts@maxi-loop.test".to_owned(),
            username: None,
            password: None,
            use_tls: false,
        }),
    };

    let state = AppState::with_alert_policy(
        config.clone(),
        pool.clone(),
        LoginRateLimiter::new(100, Duration::from_secs(60), 1024),
        test_policy.clone(),
        Duration::from_secs(3600),
    );
    state
        .dsn_cache
        .reload_from_db(&pool)
        .await
        .expect("dsn cache");
    state
        .scrub_cache
        .reload_from_db(&pool)
        .await
        .expect("scrub cache");
    let app = build_router(state.clone());

    let suffix = chrono::Utc::now().timestamp_millis();
    let tenant_a_slug = format!("p2-alpha-{suffix}");
    let tenant_b_slug = format!("p2-beta-{suffix}");
    let email_a = format!("owner-a-{suffix}@example.com");
    let email_b = format!("owner-b-{suffix}@example.com");
    let pw = "Phase2Test_StrongPass_1234";

    let outcome_a = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_a_slug.clone(),
            tenant_name: "Phase2 Alpha".to_owned(),
            retention_days: 30,
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: email_a.clone(),
                password: pw.to_owned(),
                name: "Owner A".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap A");

    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_b_slug.clone(),
            tenant_name: "Phase2 Beta".to_owned(),
            retention_days: 30,
            workspace_slug: "core".to_owned(),
            workspace_name: "Core".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: email_b.clone(),
                password: pw.to_owned(),
                name: "Owner B".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap B");

    let (cookie_a, csrf_a) = login_session(&app, &email_a, pw, &tenant_a_slug).await;
    let (cookie_b, csrf_b) = login_session(&app, &email_b, pw, &tenant_b_slug).await;

    // 1. Create a project in Tenant A
    let (st, proj_a) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "slug": "payments-api",
            "name": "Payments API",
            "platform": "python",
            "workspace_slug": "core"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let project_id = proj_a["id"].as_i64().expect("project id");
    let dsn_key = proj_a["dsn_public_key"]
        .as_str()
        .expect("dsn key")
        .to_owned();

    // 2. Configure inherited PII scrub rules (tenant + workspace + project)
    let (st, rule_tenant) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Scrub Tax ID",
            "rule_type": "field_name",
            "pattern": "tax_id",
            "replacement": "[TaxRedacted]"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let rule_tenant_id = rule_tenant["id"].as_i64().expect("rule id");
    assert_eq!(rule_tenant["scope"], "tenant");

    let (st, rule_ws) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Scrub Internal Header",
            "rule_type": "header_name",
            "pattern": "X-Employee-Secret",
            "replacement": "[EmpRedacted]",
            "workspace_slug": "core"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(rule_ws["scope"], "workspace");

    let (st, rule_proj) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/pii-rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Scrub Corp Domain",
            "rule_type": "value_substring",
            "pattern": "secret.internal.corp",
            "replacement": "[CorpHost]",
            "project_slug": "payments-api"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(rule_proj["scope"], "project");

    // 3. Negative SSRF checks on public HTTP API for alert channels
    for blocked_url in [
        "http://127.0.0.1:9999/hook",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/internal",
        "http://localhost:8080/hook",
    ] {
        let (st, _) = send_authed_json(
            &app,
            Method::POST,
            "/api/v1/alerts/channels",
            &cookie_a,
            &csrf_a,
            Some(json!({
                "name": "Blocked SSRF",
                "channel_type": "webhook",
                "config": { "url": blocked_url }
            })),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "SSRF url must be blocked: {blocked_url}"
        );
    }

    // 4. Create alert channels:
    //    - Webhook (via service with allow_private_ips=true for local mock receiver) + secret
    //    - Telegram (via public HTTP API — proves Telegram validates token/chat_id & masks secret)
    //    - Email (via public HTTP API)
    let tenant_ctx_a = TenantContext::new(outcome_a.tenant_id).expect("tenant ctx a");
    let webhook_secret = "whsec_test_hmac_signing_key_99";
    let webhook_chan = create_alert_channel(
        &pool,
        tenant_ctx_a,
        &Role::TenantOwner,
        CreateAlertChannelInput {
            name: "Ops Webhook".to_owned(),
            channel_type: "webhook".to_owned(),
            config: json!({
                "url": format!("http://127.0.0.1:{http_port}/webhook"),
                "secret": webhook_secret,
            }),
            workspace_slug: None,
            project_slug: Some("payments-api".to_owned()),
            is_enabled: Some(true),
        },
        true,
    )
    .await
    .expect("create mock webhook channel");
    assert_eq!(webhook_chan.config_summary["has_secret"], true);
    assert!(webhook_chan.config_summary.get("secret").is_none());

    let telegram_bot_token = "123456789:ABCDEF_super_secret_telegram_token";
    let (st, tg_chan) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/channels",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Oncall Telegram",
            "channel_type": "telegram",
            "config": {
                "bot_token": telegram_bot_token,
                "chat_id": "-10099887766"
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let tg_chan_id = tg_chan["id"].as_i64().expect("tg id");
    // Secret must NEVER be returned in API response
    assert!(!tg_chan.to_string().contains(telegram_bot_token));

    let (st, email_chan) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/channels",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Oncall Email",
            "channel_type": "email",
            "config": {
                "to": ["oncall@example.com"],
                "subject_prefix": "[ProdAlert]"
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let email_chan_id = email_chan["id"].as_i64().expect("email id");

    // 5. Create alert rules: new_issue -> Webhook, regression -> Telegram, spike (>=3) -> Email
    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "New issue webhook",
            "channel_id": webhook_chan.id,
            "trigger_type": "new_issue",
            "project_slug": "payments-api",
            "cooldown_minutes": 15
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Regression telegram",
            "channel_id": tg_chan_id,
            "trigger_type": "regression",
            "cooldown_minutes": 15
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/alerts/rules",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "name": "Spike email",
            "channel_id": email_chan_id,
            "trigger_type": "spike",
            "spike_threshold": 3,
            "spike_window_minutes": 60,
            "cooldown_minutes": 15
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 6. Send first event with release `2026.11.21@a1b2c3d4` + baseline & custom PII
    send_envelope(
        &app,
        project_id,
        &dsn_key,
        json!({
            "event_id": "a0000000000000000000000000000001",
            "level": "error",
            "environment": "production",
            "release": "2026.11.21@a1b2c3d4",
            "platform": "python",
            "exception": {
                "values": [{
                    "type": "PaymentGatewayError",
                    "value": "failed contacting secret.internal.corp",
                    "stacktrace": {
                        "frames": [{
                            "filename": "app/payments.py",
                            "module": "app.payments",
                            "function": "charge_card",
                            "lineno": 88,
                            "in_app": true
                        }]
                    }
                }]
            },
            "request": {
                "headers": {
                    "Authorization": "Bearer raw_top_secret_bearer",
                    "X-Employee-Secret": "EMP-99999"
                },
                "data": {
                    "customer_tax_id": "TAX-887766",
                    "note": "upstream secret.internal.corp timed out"
                }
            }
        }),
    )
    .await;

    // Wait for batch worker + real-time webhook alert delivery
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify issue persisted with scrubbed baseline + custom PII and first_release/last_release
    let (st, issues_list) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/projects/payments-api/issues",
        &cookie_a,
        &csrf_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(issues_list["items"].as_array().expect("items").len(), 1);
    let issue_id = issues_list["items"][0]["id"].as_i64().expect("issue id");
    assert_eq!(
        issues_list["items"][0]["first_release"],
        "2026.11.21@a1b2c3d4"
    );
    assert_eq!(
        issues_list["items"][0]["last_release"],
        "2026.11.21@a1b2c3d4"
    );

    let (st, detail) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{issue_id}"),
        &cookie_a,
        &csrf_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let payload_str = detail["sample_event"]["payload"].to_string();
    assert!(!payload_str.contains("raw_top_secret_bearer"));
    assert!(!payload_str.contains("EMP-99999"));
    assert!(!payload_str.contains("TAX-887766"));
    assert!(!payload_str.contains("secret.internal.corp"));
    assert!(payload_str.contains("[Filtered]"));
    assert!(payload_str.contains("[EmpRedacted]"));
    assert!(payload_str.contains("[TaxRedacted]"));
    assert!(payload_str.contains("[CorpHost]"));

    // Verify Webhook received `new_issue` with valid HMAC-SHA256 signature
    {
        let calls = captured_http.lock().await;
        let wh = calls
            .iter()
            .find(|c| c.path == "/webhook")
            .expect("webhook new_issue call");
        assert_eq!(wh.event_header.as_deref(), Some("new_issue"));
        let sig = wh.signature.as_deref().expect("hmac signature header");
        let mut mac = HmacSha256::new_from_slice(webhook_secret.as_bytes()).expect("hmac");
        mac.update(wh.body.as_bytes());
        let expected_sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert_eq!(sig, expected_sig);
    }

    // 7. Resolve issue, then send 2 more events on release `v2.0.0` (triggers regression + spike>=3)
    let (st, _) = send_authed_json(
        &app,
        Method::PATCH,
        &format!("/api/v1/issues/{issue_id}/status"),
        &cookie_a,
        &csrf_a,
        Some(json!({"status": "resolved"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    for idx in 2..=3 {
        send_envelope(
            &app,
            project_id,
            &dsn_key,
            json!({
                "event_id": format!("a000000000000000000000000000000{idx}"),
                "level": "error",
                "environment": "production",
                "release": "v2.0.0",
                "platform": "python",
                "exception": {
                    "values": [{
                        "type": "PaymentGatewayError",
                        "value": "reappeared on v2.0.0",
                        "stacktrace": {
                            "frames": [{
                                "filename": "app/payments.py",
                                "module": "app.payments",
                                "function": "charge_card",
                                "lineno": 88,
                                "in_app": true
                            }]
                        }
                    }]
                }
            }),
        )
        .await;
    }

    tokio::time::sleep(Duration::from_millis(600)).await;

    // Verify Telegram received `regression` alert
    {
        let calls = captured_http.lock().await;
        let tg = calls
            .iter()
            .find(|c| c.path.ends_with("/sendMessage"))
            .expect("telegram regression call");
        assert!(tg.body.contains("REGRESSION"));
        assert!(tg.body.contains("-10099887766"));
    }

    // Verify Release tracking links both releases (`2026.11.21@a1b2c3d4` and `v2.0.0`) to this issue
    let (st, releases) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/projects/payments-api/releases",
        &cookie_a,
        &csrf_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let rel_arr = releases.as_array().expect("releases array");
    assert_eq!(rel_arr.len(), 2);
    let rel_v1 = rel_arr
        .iter()
        .find(|r| r["version"] == "2026.11.21@a1b2c3d4")
        .expect("v1 release");
    assert_eq!(rel_v1["commit_sha"], "a1b2c3d4");
    assert_eq!(rel_v1["new_issues_count"], 1);
    assert_eq!(rel_v1["linked_issues_count"], 1);

    let rel_v1_id = rel_v1["id"].as_i64().expect("rel_v1 id");
    let (st, rel_issues) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/releases/{rel_v1_id}/issues"),
        &cookie_a,
        &csrf_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rel_issues.as_array().expect("rel issues").len(), 1);
    assert_eq!(rel_issues[0]["id"], issue_id);

    // Annotate v2.0.0 with explicit commit SHA via POST /api/v1/releases
    let (st, annotated) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/releases",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "project_slug": "payments-api",
            "version": "v2.0.0",
            "commit_sha": "deadbeef1234",
            "notes": "Hotfix checkout rollout"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(annotated["commit_sha"], "deadbeef1234");
    assert_eq!(annotated["notes"], "Hotfix checkout rollout");

    // 8. Run spike evaluator -> dispatches Email alert over SMTP, then cooldown suppresses repeat
    let sent_spikes = evaluate_spike_rules(&pool, &config.public_base_url, &test_policy)
        .await
        .expect("spike eval 1");
    assert_eq!(sent_spikes, 1);
    {
        let emails = captured_emails.lock().await;
        assert_eq!(emails.len(), 1);
        assert!(emails[0].contains("[ProdAlert] [SPIKE] payments-api"));
        assert!(emails[0].contains("oncall@example.com"));
    }

    let sent_spikes_repeat = evaluate_spike_rules(&pool, &config.public_base_url, &test_policy)
        .await
        .expect("spike eval 2");
    assert_eq!(
        sent_spikes_repeat, 0,
        "cooldown must suppress duplicate spike alert"
    );

    // Verify deliveries log shows all 3 sent deliveries (webhook, telegram, email)
    let (st, deliveries) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/alerts/deliveries",
        &cookie_a,
        &csrf_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let del_arr = deliveries.as_array().expect("deliveries");
    assert_eq!(del_arr.len(), 3);
    assert!(del_arr.iter().all(|d| d["status"] == "sent"));

    // 9. Cross-tenant isolation: Tenant B cannot see or mutate Tenant A's PII rules, releases, or alerts
    let (st, b_rules) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/pii-rules",
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_rules.as_array().expect("b rules").len(), 0);

    let (st, _) = send_authed_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/pii-rules/{rule_tenant_id}"),
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, b_releases) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/releases",
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_releases.as_array().expect("b releases").len(), 0);

    let (st, _) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/releases/{rel_v1_id}/issues"),
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, b_channels) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/alerts/channels",
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_channels.as_array().expect("b channels").len(), 0);

    let (st, _) = send_authed_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/alerts/channels/{tg_chan_id}"),
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, b_deliveries) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/alerts/deliveries",
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_deliveries.as_array().expect("b deliveries").len(), 0);
}
