pub mod cli;
pub mod http;

use std::sync::Arc;

use axum::extract::FromRef;
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_platform::config::Config;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub login_limiter: LoginRateLimiter,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> Self {
        Self {
            config: Arc::new(config),
            db,
            login_limiter: LoginRateLimiter::default(),
        }
    }

    pub fn with_login_limiter(config: Config, db: PgPool, login_limiter: LoginRateLimiter) -> Self {
        Self {
            config: Arc::new(config),
            db,
            login_limiter,
        }
    }
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        AuthState::new(
            Arc::clone(&state.config),
            state.db.clone(),
            state.login_limiter.clone(),
        )
    }
}
