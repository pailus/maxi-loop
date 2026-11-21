use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::dsn_notify::{ProjectDsnInfo, ProjectDsnObserver};
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::scrub_notify::{PiiScrubRuleEntry, PiiScrubRulesObserver};
use serde::Serialize;
use sqlx::PgPool;

use crate::model::{AuthSession, AuthSessionView, LoginRequest};
use crate::rate_limit::LoginRateLimiter;
use crate::service::{authenticate_cookie, login, logout};
use crate::token::{build_clear_session_cookie, build_session_set_cookie};

#[derive(Clone)]
pub struct AuthState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub login_limiter: LoginRateLimiter,
    pub dsn_observer: Option<Arc<dyn ProjectDsnObserver>>,
    pub pii_observer: Option<Arc<dyn PiiScrubRulesObserver>>,
}

impl AuthState {
    pub fn new(config: Arc<Config>, db: PgPool, login_limiter: LoginRateLimiter) -> Self {
        Self {
            config,
            db,
            login_limiter,
            dsn_observer: None,
            pii_observer: None,
        }
    }

    pub fn with_dsn_observer(
        config: Arc<Config>,
        db: PgPool,
        login_limiter: LoginRateLimiter,
        dsn_observer: Arc<dyn ProjectDsnObserver>,
    ) -> Self {
        Self {
            config,
            db,
            login_limiter,
            dsn_observer: Some(dsn_observer),
            pii_observer: None,
        }
    }

    pub fn with_observers(
        config: Arc<Config>,
        db: PgPool,
        login_limiter: LoginRateLimiter,
        dsn_observer: Arc<dyn ProjectDsnObserver>,
        pii_observer: Arc<dyn PiiScrubRulesObserver>,
    ) -> Self {
        Self {
            config,
            db,
            login_limiter,
            dsn_observer: Some(dsn_observer),
            pii_observer: Some(pii_observer),
        }
    }

    pub fn notify_project_dsn(&self, info: ProjectDsnInfo) {
        if let Some(observer) = &self.dsn_observer {
            observer.on_project_dsn_upserted(info);
        }
    }

    pub fn notify_pii_rule_upserted(&self, rule: PiiScrubRuleEntry) {
        if let Some(observer) = &self.pii_observer {
            observer.on_pii_rule_upserted(rule);
        }
    }

    pub fn notify_pii_rule_deleted(&self, tenant_id: i64, rule_id: i64) {
        if let Some(observer) = &self.pii_observer {
            observer.on_pii_rule_deleted(tenant_id, rule_id);
        }
    }
}

impl<S> FromRequestParts<S> for AuthSession
where
    AuthState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth_state = AuthState::from_ref(state);
        let session = authenticate_cookie(&auth_state.db, &parts.headers).await?;

        // State-changing requests authenticated via session cookie MUST pass CSRF validation.
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
            session.verify_csrf_header(&parts.headers)?;
        }

        Ok(session)
    }
}

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/auth/login", post(login_handler))
        .route("/auth/logout", post(logout_handler))
        .route("/auth/me", get(me_handler))
}

async fn login_handler(
    State(state): State<AuthState>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let (session, raw_token) = login(&state.db, &state.config, &state.login_limiter, req).await?;

    let max_age_secs = u64::from(state.config.session_ttl_hours) * 3600;
    let set_cookie = build_session_set_cookie(&raw_token, max_age_secs, state.config.cookie_secure);
    let view = session.to_view(state.config.single_tenant_mode);

    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, set_cookie)],
        Json(view),
    )
        .into_response())
}

#[derive(Serialize)]
struct LogoutResponse {
    status: &'static str,
}

async fn logout_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Response, AppError> {
    logout(&state.db, &session).await?;
    let clear_cookie = build_clear_session_cookie(state.config.cookie_secure);

    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, clear_cookie)],
        Json(LogoutResponse {
            status: "logged_out",
        }),
    )
        .into_response())
}

async fn me_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<AuthSessionView>, AppError> {
    Ok(Json(session.to_view(state.config.single_tenant_mode)))
}
