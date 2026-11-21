use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_platform::bootstrap::{
    AdminUserSpec, BootstrapSpec, bootstrap_defaults, hash_password_argon2id,
};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{
    connect_db, ensure_upcoming_events_partitions, ensure_upcoming_traces_partitions,
    run_migrations,
};
use maxi_loop_platform::tenant::begin_worker_tx;
use serde_json::{Value, json};
use tower::ServiceExt;

fn load_test_config() -> Config {
    let _ = dotenvy::dotenv();
    let mut config = Config::from_env().expect("DATABASE_URL must be configured");
    config.cookie_secure = false;
    config.public_base_url = "http://127.0.0.1:8080".to_owned();
    config.sourcemap_max_upload_bytes = 2 * 1024 * 1024;
    config.sourcemap_cache_max_bytes = 16 * 1024 * 1024;
    config.sourcemap_cache_max_entries = 16;
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
        .expect("login req");

    let resp = app.clone().oneshot(req).await.expect("login resp");
    assert_eq!(resp.status(), StatusCode::OK);

    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("utf8")
        .split(';')
        .next()
        .expect("cookie kv")
        .to_owned();

    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .expect("body");
    let parsed: Value = serde_json::from_slice(&body).expect("json");
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

    let resp = app.clone().oneshot(req).await.expect("resp");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 512 * 1024)
        .await
        .expect("bytes");
    let json_val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json_val)
}

async fn send_error_envelope(
    app: &Router,
    project_id: i64,
    dsn_key: &str,
    event_payload: Value,
) -> StatusCode {
    let ev_id = event_payload["event_id"]
        .as_str()
        .unwrap_or("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let raw = format!(
        "{{\"event_id\":\"{ev_id}\"}}\n{{\"type\":\"event\"}}\n{}\n",
        event_payload
    );

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/{project_id}/envelope/"))
        .header(
            "x-sentry-auth",
            format!("Sentry sentry_version=7, sentry_key={dsn_key}"),
        )
        .body(Body::from(raw))
        .expect("env req");
    app.clone().oneshot(req).await.expect("env resp").status()
}

#[tokio::test]
async fn phase4_sourcemaps_upload_bounded_lru_resolution_before_fingerprinting_and_isolation() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect db");
    run_migrations(&pool).await.expect("migrations");
    let today = Utc::now().date_naive();
    ensure_upcoming_events_partitions(&pool, today, 3)
        .await
        .expect("events parts");
    ensure_upcoming_traces_partitions(&pool, today, 3)
        .await
        .expect("traces parts");

    let run_id = Utc::now().timestamp_nanos_opt().unwrap_or(77) % 100_000_000;
    let tenant_a_slug = format!("sm-a-{run_id}");
    let tenant_b_slug = format!("sm-b-{run_id}");
    let email_a = format!("sm-a-{run_id}@example.com");
    let viewer_email = format!("sm-viewer-{run_id}@example.com");
    let email_b = format!("sm-b-{run_id}@example.com");
    let password = "sourcemap_test_password_123";

    let boot_a = bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_a_slug.clone(),
            tenant_name: "SourceMap Tenant A".to_owned(),
            workspace_slug: "web".to_owned(),
            workspace_name: "Web".to_owned(),
            retention_days: 30,
            admin_user: Some(AdminUserSpec {
                email: email_a.clone(),
                name: "Alice SM".to_owned(),
                password: password.to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap A");

    bootstrap_defaults(
        &pool,
        &BootstrapSpec {
            tenant_slug: tenant_b_slug.clone(),
            tenant_name: "SourceMap Tenant B".to_owned(),
            workspace_slug: "web".to_owned(),
            workspace_name: "Web".to_owned(),
            retention_days: 30,
            admin_user: Some(AdminUserSpec {
                email: email_b.clone(),
                name: "Bob SM".to_owned(),
                password: password.to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap B");

    // Create a read-only `viewer` user in Tenant A for negative RBAC verification.
    {
        let pw_hash = hash_password_argon2id(password).expect("hash pw");
        let mut tx = begin_worker_tx(&pool).await.expect("worker tx");
        let viewer_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email, name, password_hash) VALUES ($1, 'Viewer', $2) RETURNING id",
        )
        .bind(&viewer_email)
        .bind(&pw_hash)
        .fetch_one(&mut *tx)
        .await
        .expect("insert viewer");
        sqlx::query("INSERT INTO memberships (tenant_id, user_id, role) VALUES ($1, $2, 'viewer')")
            .bind(boot_a.tenant_id)
            .bind(viewer_id)
            .execute(&mut *tx)
            .await
            .expect("insert viewer membership");
        tx.commit().await.expect("commit");
    }

    let state =
        AppState::with_login_limiter(config.clone(), pool.clone(), LoginRateLimiter::default());
    state.dsn_cache.reload_from_db(&pool).await.expect("dsn");
    state
        .scrub_cache
        .reload_from_db(&pool)
        .await
        .expect("scrub");
    let app = build_router(state.clone());

    let (cookie_a, csrf_a) = login_session(&app, &email_a, password, &tenant_a_slug).await;
    let (cookie_viewer, csrf_viewer) =
        login_session(&app, &viewer_email, password, &tenant_a_slug).await;
    let (cookie_b, csrf_b) = login_session(&app, &email_b, password, &tenant_b_slug).await;

    let (st, proj_a) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_a,
        &csrf_a,
        Some(json!({ "slug": "react-store", "name": "React Store", "platform": "javascript" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_a_id = proj_a["id"].as_i64().unwrap();
    let proj_a_key = proj_a["dsn_public_key"].as_str().unwrap().to_owned();

    let (st, proj_b) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/projects",
        &cookie_b,
        &csrf_b,
        Some(json!({ "slug": "react-store", "name": "Tenant B React", "platform": "javascript" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let proj_b_id = proj_b["id"].as_i64().unwrap();
    let proj_b_key = proj_b["dsn_public_key"].as_str().unwrap().to_owned();

    // Two distinct minified builds (different bundle filenames & different minified col positions)
    // that BOTH map to the EXACT same original React source `src/Checkout.tsx:submitOrder` (line 3)!
    let source_code = "import React from 'react';\nexport function submitOrder(cart: any) {\n  if (!cart) throw new TypeError('empty cart');\n  return cart.total;\n}\n";

    // Build 1 (`assets/bundle-v1.js`): minified (line 1, col 1 -> 0-based 0,0) maps to src line 2 (1-based 3), name `submitOrder`
    let sm_v1 = json!({
        "version": 3,
        "file": "bundle-v1.js",
        "sources": ["webpack:///./src/Checkout.tsx"],
        "sourcesContent": [source_code],
        "names": ["submitOrder"],
        "mappings": "AAECA"
    })
    .to_string();

    // Build 2 (`assets/bundle-v2.js`): minified (line 1, col 11 -> 0-based 0,10) ALSO maps to src line 2 (1-based 3), name `submitOrder`
    // VLQ: col 10 = 'U', src 0 = 'A', src_line 2 = 'E', src_col 2 = 'E', name 0 = 'A' => "UAEEA"
    let sm_v2 = json!({
        "version": 3,
        "file": "bundle-v2.js",
        "sources": ["webpack:///./src/Checkout.tsx"],
        "sourcesContent": [source_code],
        "names": ["submitOrder"],
        "mappings": "UAEEA"
    })
    .to_string();

    // Debug-ID-matched source map (release-independent resolution via Sentry `debug_meta.images`)
    let debug_id = format!("8f2b0d2c-1a2b-4c3d-9e8f-{run_id:012x}");
    let sm_debug = json!({
        "version": 3,
        "file": "chunk-debug.js",
        "debugId": debug_id,
        "sources": ["src/ProfileSettings.tsx"],
        "sourcesContent": [
            "export function saveProfile(user: any) {\n  return user.email.toLowerCase();\n}\n"
        ],
        "names": ["saveProfile"],
        "mappings": "AACAA"
    })
    .to_string();

    // 1. Negative upload checks: viewer -> 403, invalid JSON -> 400, path traversal -> 400.
    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/sourcemaps",
        &cookie_viewer,
        &csrf_viewer,
        Some(json!({
            "project_slug": "react-store",
            "release_version": "web@1.0.0",
            "filename": "~/assets/bundle-v1.js",
            "content": sm_v1
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/sourcemaps",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "project_slug": "react-store",
            "release_version": "web@1.0.0",
            "filename": "~/assets/bad.js",
            "content": "not valid sourcemap"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/sourcemaps",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "project_slug": "react-store",
            "release_version": "web@1.0.0",
            "filename": "../../etc/passwd",
            "content": sm_v1
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 2. Upload sm_v1 (JSON endpoint) and sm_v2 (raw body CI endpoint) and sm_debug (embedded debugId).
    let (st, uploaded_1) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/sourcemaps",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "project_slug": "react-store",
            "release_version": "web@1.0.0",
            "filename": "~/assets/bundle-v1.js.map",
            "content": sm_v1
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(uploaded_1["filename"], "assets/bundle-v1.js");
    let sm1_id = uploaded_1["id"].as_i64().unwrap();

    // Raw binary/text upload route for CI pipelines (`POST /api/v1/projects/{slug}/releases/{ver}/sourcemaps`)
    let raw_req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/projects/react-store/releases/web@2.0.0/sourcemaps?filename=~/assets/bundle-v2.js")
        .header(header::COOKIE, &cookie_a)
        .header("x-csrf-token", &csrf_a)
        .body(Body::from(sm_v2.clone()))
        .expect("raw upload req");
    let raw_resp = app.clone().oneshot(raw_req).await.expect("raw resp");
    assert_eq!(raw_resp.status(), StatusCode::CREATED);

    let (st, uploaded_dbg) = send_authed_json(
        &app,
        Method::POST,
        "/api/v1/sourcemaps",
        &cookie_a,
        &csrf_a,
        Some(json!({
            "project_slug": "react-store",
            "release_version": "web@2.0.0",
            "filename": "~/assets/chunk-debug.js",
            "content": sm_debug
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(uploaded_dbg["debug_id"], debug_id);

    // 3. Ingest minified React event #1 (release web@1.0.0, `bundle-v1.js`, line 1 col 1)
    //    AND minified React event #2 (release web@2.0.0, `bundle-v2.js`, line 1 col 11)
    //    Because resolution runs BEFORE fingerprinting, BOTH must resolve to `src/Checkout.tsx:submitOrder`
    //    and group into ONE SINGLE issue (`event_count == 2`)!
    let st = send_error_envelope(
        &app,
        proj_a_id,
        &proj_a_key,
        json!({
            "event_id": format!("sm1{run_id:08}000000000000000000001"),
            "level": "error",
            "environment": "production",
            "release": "web@1.0.0",
            "platform": "javascript",
            "exception": {
                "values": [{
                    "type": "TypeError",
                    "value": "empty cart for session 101",
                    "stacktrace": {
                        "frames": [{
                            "filename": "https://shop.example.com/assets/bundle-v1.js?v=1",
                            "abs_path": "https://shop.example.com/assets/bundle-v1.js?v=1",
                            "function": "a",
                            "lineno": 1,
                            "colno": 1,
                            "in_app": true
                        }]
                    }
                }]
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let st = send_error_envelope(
        &app,
        proj_a_id,
        &proj_a_key,
        json!({
            "event_id": format!("sm2{run_id:08}000000000000000000002"),
            "level": "error",
            "environment": "production",
            "release": "web@2.0.0",
            "platform": "javascript",
            "exception": {
                "values": [{
                    "type": "TypeError",
                    "value": "empty cart for session 202",
                    "stacktrace": {
                        "frames": [{
                            "filename": "https://shop.example.com/assets/bundle-v2.js",
                            "abs_path": "https://shop.example.com/assets/bundle-v2.js",
                            "function": "z",
                            "lineno": 1,
                            "colno": 11,
                            "in_app": true
                        }]
                    }
                }]
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 4. Ingest event #3 matched strictly by `debug_id` (NO `release` field sent!).
    let st = send_error_envelope(
        &app,
        proj_a_id,
        &proj_a_key,
        json!({
            "event_id": format!("smd{run_id:08}000000000000000000003"),
            "level": "error",
            "environment": "production",
            "platform": "javascript",
            "debug_meta": {
                "images": [{
                    "type": "sourcemap",
                    "code_file": "https://shop.example.com/assets/renamed-chunk.js",
                    "debug_id": debug_id
                }]
            },
            "exception": {
                "values": [{
                    "type": "TypeError",
                    "value": "Cannot read properties of undefined (reading 'toLowerCase')",
                    "stacktrace": {
                        "frames": [{
                            "filename": "https://shop.example.com/assets/renamed-chunk.js",
                            "abs_path": "https://shop.example.com/assets/renamed-chunk.js",
                            "function": "q",
                            "lineno": 1,
                            "colno": 1,
                            "in_app": true
                        }]
                    }
                }]
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 5. Ingest an event on Tenant B referencing Tenant A's `debug_id` and `release + filename`:
    //    Tenant B MUST NOT resolve using Tenant A's source maps!
    let st = send_error_envelope(
        &app,
        proj_b_id,
        &proj_b_key,
        json!({
            "event_id": format!("smb{run_id:08}000000000000000000004"),
            "level": "error",
            "environment": "production",
            "release": "web@1.0.0",
            "platform": "javascript",
            "debug_meta": {
                "images": [{
                    "type": "sourcemap",
                    "code_file": "https://evil.example.com/assets/bundle-v1.js",
                    "debug_id": debug_id
                }]
            },
            "exception": {
                "values": [{
                    "type": "TypeError",
                    "value": "tenant b minified crash",
                    "stacktrace": {
                        "frames": [{
                            "filename": "https://evil.example.com/assets/bundle-v1.js",
                            "abs_path": "https://evil.example.com/assets/bundle-v1.js",
                            "function": "minifiedFnB",
                            "lineno": 1,
                            "colno": 1,
                            "in_app": true
                        }]
                    }
                }]
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Wait for async worker flush (poll until Tenant A has 2 issues: Checkout with count=2, Profile with count=1)
    let mut issues_a = Vec::new();
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (st, resp) = send_authed_json(
            &app,
            Method::GET,
            "/api/v1/issues?project_slug=react-store",
            &cookie_a,
            "",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let items = resp["items"].as_array().cloned().unwrap_or_default();
        if items.len() == 2 && items.iter().any(|i| i["event_count"] == 2) {
            issues_a = items;
            break;
        }
    }
    assert_eq!(
        issues_a.len(),
        2,
        "expected exactly 2 issues (Checkout grouped across v1+v2 builds, and Profile via debug_id), got: {issues_a:?}"
    );

    let checkout_issue = issues_a
        .iter()
        .find(|i| i["culprit"].as_str().unwrap_or("").contains("Checkout.tsx"))
        .expect("resolved Checkout.tsx culprit");
    assert_eq!(
        checkout_issue["event_count"], 2,
        "v1 and v2 minified builds must group into ONE issue after source map resolution"
    );
    let checkout_id = checkout_issue["id"].as_i64().unwrap();

    let (st, detail) = send_authed_json(
        &app,
        Method::GET,
        &format!("/api/v1/issues/{checkout_id}"),
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let frame0 = &detail["stack_frames"][0];
    assert_eq!(frame0["filename"], "src/Checkout.tsx");
    assert_eq!(frame0["function"], "submitOrder");
    assert_eq!(frame0["lineno"], 3);
    assert_eq!(
        frame0["context_line"],
        "  if (!cart) throw new TypeError('empty cart');"
    );
    assert_eq!(frame0["pre_context"].as_array().unwrap().len(), 2);

    // Verify debug_id resolved issue (`src/ProfileSettings.tsx:saveProfile`)
    let profile_issue = issues_a
        .iter()
        .find(|i| {
            i["culprit"]
                .as_str()
                .unwrap_or("")
                .contains("ProfileSettings.tsx")
        })
        .expect("resolved ProfileSettings.tsx culprit via debug_id");
    assert_eq!(profile_issue["event_count"], 1);

    // 6. Verify LRU cache stats recorded entries and hits on repeat event.
    let st = send_error_envelope(
        &app,
        proj_a_id,
        &proj_a_key,
        json!({
            "event_id": format!("smh{run_id:08}000000000000000000005"),
            "level": "error",
            "environment": "production",
            "release": "web@1.0.0",
            "platform": "javascript",
            "exception": {
                "values": [{
                    "type": "TypeError",
                    "value": "empty cart for session 303",
                    "stacktrace": {
                        "frames": [{
                            "filename": "https://shop.example.com/assets/bundle-v1.js",
                            "function": "a",
                            "lineno": 1,
                            "colno": 1,
                            "in_app": true
                        }]
                    }
                }]
            }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let cache_stats = state.sourcemap_resolver.cache_stats();
    assert!(
        cache_stats.hits >= 1,
        "expected LRU cache hit on repeated bundle-v1.js crash, got {cache_stats:?}"
    );
    assert!(cache_stats.used_bytes <= cache_stats.max_bytes);

    // 7. Verify cross-tenant isolation: Tenant B's event stayed unresolved (`minifiedFnB`),
    //    Tenant B cannot list or delete Tenant A's source maps!
    let (st, issues_b) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/issues?project_slug=react-store",
        &cookie_b,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let b_item = &issues_b["items"][0];
    assert!(
        b_item["culprit"]
            .as_str()
            .unwrap_or("")
            .contains("minifiedFnB"),
        "Tenant B must NEVER resolve using Tenant A's source maps: {b_item:?}"
    );

    let (st, list_b) =
        send_authed_json(&app, Method::GET, "/api/v1/sourcemaps", &cookie_b, "", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list_b.as_array().unwrap().len(), 0);

    let (st, _) = send_authed_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/sourcemaps/{sm1_id}"),
        &cookie_b,
        &csrf_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Tenant A can list and delete its own source map cleanly.
    let (st, list_a) = send_authed_json(
        &app,
        Method::GET,
        "/api/v1/sourcemaps?project_slug=react-store",
        &cookie_a,
        "",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list_a.as_array().unwrap().len(), 3);

    let (st, _) = send_authed_json(
        &app,
        Method::DELETE,
        &format!("/api/v1/sourcemaps/{sm1_id}"),
        &cookie_a,
        &csrf_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}
