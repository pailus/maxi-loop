//! F022 invite-by-link onboarding and F023 self-service password change.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_platform::bootstrap::{AdminUserSpec, BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{connect_db, run_migrations};
use maxi_loop_platform::tenant::begin_worker_tx;
use serde_json::{Value, json};
use tower::ServiceExt;

fn load_test_config() -> Config {
    let _ = dotenvy::dotenv();
    let mut config = Config::from_env().expect("DATABASE_URL must be configured");
    config.cookie_secure = false;
    config.public_base_url = "http://127.0.0.1:8080".to_owned();
    config
}

struct Resp {
    status: StatusCode,
    body: Value,
    /// `name=value` of a `Set-Cookie` header, when present.
    cookie: Option<String>,
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    auth: Option<(&str, &str)>,
    body: Option<Value>,
) -> Resp {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    if let Some((cookie, csrf)) = auth {
        builder = builder.header(header::COOKIE, cookie);
        if method != Method::GET {
            builder = builder.header("x-csrf-token", csrf);
        }
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
    let cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(ToOwned::to_owned);
    let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .expect("bytes");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Resp {
        status,
        body,
        cookie,
    }
}

/// Returns `(cookie, csrf)` of a fresh session, panicking on failure.
fn session_of(resp: &Resp) -> (String, String) {
    assert_eq!(
        resp.status,
        StatusCode::OK,
        "expected session: {}",
        resp.body
    );
    (
        resp.cookie.clone().expect("set-cookie"),
        resp.body["csrf_token"].as_str().expect("csrf").to_owned(),
    )
}

async fn login(app: &Router, email: &str, password: &str, tenant: &str) -> Resp {
    send(
        app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({ "email": email, "password": password, "tenant_slug": tenant })),
    )
    .await
}

fn token_from(created: &Value) -> String {
    let url = created["invite_url"].as_str().expect("invite_url");
    let token = url.split("#invite=").nth(1).expect("token in fragment");
    assert_eq!(token.len(), 64);
    token.to_owned()
}

async fn bootstrap_tenant(pool: &sqlx::PgPool, slug: &str, email: &str, pw: &str) {
    bootstrap_defaults(
        pool,
        &BootstrapSpec {
            tenant_slug: slug.to_owned(),
            tenant_name: format!("Tenant {slug}"),
            retention_days: 30,
            workspace_slug: "default".to_owned(),
            workspace_name: "Default Workspace".to_owned(),
            admin_user: Some(AdminUserSpec {
                email: email.to_owned(),
                password: pw.to_owned(),
                name: "Owner".to_owned(),
            }),
        },
    )
    .await
    .expect("bootstrap tenant");
}

#[tokio::test]
async fn invitations_and_password_change() {
    let config = load_test_config();
    let pool = connect_db(&config).await.expect("connect db");
    run_migrations(&pool).await.expect("run migrations");

    let suffix = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tenant_a = format!("f022-a-{suffix}");
    let tenant_b = format!("f022-b-{suffix}");
    let owner_a = format!("owner-a-{suffix}@example.com");
    let owner_b = format!("owner-b-{suffix}@example.com");
    let invitee = format!("new-{suffix}@example.com");
    let pw = "F022-Strong-Password-2026!";
    let invitee_pw = "Invitee-Chosen-Password-1";

    bootstrap_tenant(&pool, &tenant_a, &owner_a, pw).await;
    bootstrap_tenant(&pool, &tenant_b, &owner_b, pw).await;

    let state = AppState::with_login_limiter(
        config.clone(),
        pool.clone(),
        LoginRateLimiter::new(100, Duration::from_secs(60), 1000),
    );
    let app = build_router(state);

    let (owner_cookie, owner_csrf) = session_of(&login(&app, &owner_a, pw, &tenant_a).await);
    let owner = Some((owner_cookie.as_str(), owner_csrf.as_str()));
    let (owner_b_cookie, owner_b_csrf) = session_of(&login(&app, &owner_b, pw, &tenant_b).await);
    let owner_b_auth = Some((owner_b_cookie.as_str(), owner_b_csrf.as_str()));

    // --- Create + list ---
    let created = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        owner,
        Some(json!({ "email": format!("  NEW-{suffix}@Example.com "), "role": "member" })),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["invitation"]["email"], invitee.as_str());
    assert!(
        created.body["invite_url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:8080/#invite=")
    );
    let token = token_from(&created.body);
    let invite_id = created.body["invitation"]["id"].as_i64().unwrap();

    let listed = send(&app, Method::GET, "/api/v1/invitations", owner, None).await;
    assert_eq!(listed.status, StatusCode::OK);
    let items = listed.body.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].get("token_hash").is_none(), "hash must not leak");
    assert_eq!(items[0]["invited_by_email"], owner_a.as_str());

    // Tenant B cannot see or revoke tenant A's invite.
    let listed_b = send(&app, Method::GET, "/api/v1/invitations", owner_b_auth, None).await;
    assert_eq!(listed_b.body.as_array().unwrap().len(), 0);
    let revoke_b = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/invitations/{invite_id}"),
        owner_b_auth,
        None,
    )
    .await;
    assert_eq!(revoke_b.status, StatusCode::NOT_FOUND);

    // Unauthenticated callers cannot manage invites.
    let anon = send(&app, Method::GET, "/api/v1/invitations", None, None).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);

    // --- Preview ---
    let preview = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/preview",
        None,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(preview.status, StatusCode::OK);
    assert_eq!(preview.body["email"], invitee.as_str());
    assert_eq!(preview.body["account_exists"], false);
    assert_eq!(preview.body["tenant_name"], format!("Tenant {tenant_a}"));

    for bad in ["nope".to_owned(), "a".repeat(64)] {
        let r = send(
            &app,
            Method::POST,
            "/api/v1/auth/invitations/preview",
            None,
            Some(json!({ "token": bad })),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }

    // --- Accept ---
    let weak = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({ "token": token, "name": "New Person", "password": "short" })),
    )
    .await;
    assert_eq!(weak.status, StatusCode::BAD_REQUEST);
    let no_name = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({ "token": token, "password": invitee_pw })),
    )
    .await;
    assert_eq!(no_name.status, StatusCode::BAD_REQUEST);

    let accepted = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({ "token": token, "name": "New Person", "password": invitee_pw })),
    )
    .await;
    let (inv_cookie, inv_csrf) = session_of(&accepted);
    assert_eq!(accepted.body["user"]["email"], invitee.as_str());
    assert_eq!(accepted.body["tenant"]["slug"], tenant_a.as_str());
    assert_eq!(accepted.body["tenant_role"], "member");

    let me = send(
        &app,
        Method::GET,
        "/api/v1/auth/me",
        Some((&inv_cookie, &inv_csrf)),
        None,
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);

    // Single use.
    let replay = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({ "token": token, "name": "Again", "password": invitee_pw })),
    )
    .await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    let listed = send(&app, Method::GET, "/api/v1/invitations", owner, None).await;
    assert_eq!(listed.body.as_array().unwrap().len(), 0);

    // A plain member cannot invite.
    let member_invite = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        Some((&inv_cookie, &inv_csrf)),
        Some(json!({ "email": format!("x-{suffix}@example.com"), "role": "viewer" })),
    )
    .await;
    assert_eq!(member_invite.status, StatusCode::FORBIDDEN);

    // Inviting someone who already has this scope is a conflict.
    let dup = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        owner,
        Some(json!({ "email": invitee, "role": "viewer" })),
    )
    .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);

    // --- Re-invite replaces the old token; revoke kills a token ---
    let other = format!("other-{suffix}@example.com");
    let first = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        owner,
        Some(json!({ "email": other, "role": "viewer" })),
    )
    .await;
    let first_token = token_from(&first.body);
    let second = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        owner,
        Some(json!({ "email": other, "role": "member" })),
    )
    .await;
    assert_eq!(second.status, StatusCode::CREATED);
    let second_token = token_from(&second.body);
    let old = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/preview",
        None,
        Some(json!({ "token": first_token })),
    )
    .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);

    let second_id = second.body["invitation"]["id"].as_i64().unwrap();
    let revoked = send(
        &app,
        Method::DELETE,
        &format!("/api/v1/invitations/{second_id}"),
        owner,
        None,
    )
    .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    let after_revoke = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({ "token": second_token, "name": "Other", "password": invitee_pw })),
    )
    .await;
    assert_eq!(after_revoke.status, StatusCode::UNAUTHORIZED);

    // --- Expired invites cannot be used ---
    let expiring = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        owner,
        Some(json!({ "email": format!("late-{suffix}@example.com"), "role": "viewer" })),
    )
    .await;
    let expiring_token = token_from(&expiring.body);
    let expiring_id = expiring.body["invitation"]["id"].as_i64().unwrap();
    {
        let mut tx = begin_worker_tx(&pool).await.expect("worker tx");
        sqlx::query(
            "UPDATE invitations SET expires_at = NOW() - INTERVAL '1 minute' WHERE id = $1",
        )
        .bind(expiring_id)
        .execute(&mut *tx)
        .await
        .expect("expire invite");
        tx.commit().await.expect("commit");
    }
    let expired = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/preview",
        None,
        Some(json!({ "token": expiring_token })),
    )
    .await;
    assert_eq!(expired.status, StatusCode::UNAUTHORIZED);

    // --- Existing account (owner of tenant B) must prove its password ---
    let cross = send(
        &app,
        Method::POST,
        "/api/v1/invitations",
        owner,
        Some(json!({ "email": owner_b, "role": "viewer", "workspace_slug": "default" })),
    )
    .await;
    assert_eq!(cross.status, StatusCode::CREATED, "{}", cross.body);
    let cross_token = token_from(&cross.body);
    let p = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/preview",
        None,
        Some(json!({ "token": cross_token })),
    )
    .await;
    assert_eq!(p.body["account_exists"], true);
    assert_eq!(p.body["scope"], "workspace");
    let wrong = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({ "token": cross_token, "password": "Definitely-Wrong-Pass" })),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    let ok = send(
        &app,
        Method::POST,
        "/api/v1/auth/invitations/accept",
        None,
        Some(json!({ "token": cross_token, "name": "Ignored", "password": pw })),
    )
    .await;
    session_of(&ok);
    assert_eq!(ok.body["tenant"]["slug"], tenant_a.as_str());
    assert_eq!(ok.body["user"]["name"], "Owner", "existing name kept");
    // Their password is unchanged and tenant B access is intact.
    session_of(&login(&app, &owner_b, pw, &tenant_b).await);

    // --- F023: password change ---
    let inv = Some((inv_cookie.as_str(), inv_csrf.as_str()));
    let (other_cookie, other_csrf) =
        session_of(&login(&app, &invitee, invitee_pw, &tenant_a).await);
    let new_pw = "Rotated-Password-2026-xyz";

    let wrong_current = send(
        &app,
        Method::POST,
        "/api/v1/auth/password",
        inv,
        Some(json!({ "current_password": "not-my-password!", "new_password": new_pw })),
    )
    .await;
    assert_eq!(wrong_current.status, StatusCode::FORBIDDEN);
    let too_short = send(
        &app,
        Method::POST,
        "/api/v1/auth/password",
        inv,
        Some(json!({ "current_password": invitee_pw, "new_password": "short" })),
    )
    .await;
    assert_eq!(too_short.status, StatusCode::BAD_REQUEST);
    let no_csrf = send(
        &app,
        Method::POST,
        "/api/v1/auth/password",
        Some((&inv_cookie, "wrong-csrf")),
        Some(json!({ "current_password": invitee_pw, "new_password": new_pw })),
    )
    .await;
    assert!(no_csrf.status.is_client_error());
    assert_ne!(no_csrf.status, StatusCode::OK);

    let changed = send(
        &app,
        Method::POST,
        "/api/v1/auth/password",
        inv,
        Some(json!({ "current_password": invitee_pw, "new_password": new_pw })),
    )
    .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);

    // Current session survives, the other one is revoked.
    let me = send(&app, Method::GET, "/api/v1/auth/me", inv, None).await;
    assert_eq!(me.status, StatusCode::OK);
    let me_other = send(
        &app,
        Method::GET,
        "/api/v1/auth/me",
        Some((&other_cookie, &other_csrf)),
        None,
    )
    .await;
    assert_eq!(me_other.status, StatusCode::UNAUTHORIZED);

    assert_eq!(
        login(&app, &invitee, invitee_pw, &tenant_a).await.status,
        StatusCode::UNAUTHORIZED
    );
    session_of(&login(&app, &invitee, new_pw, &tenant_a).await);
}
