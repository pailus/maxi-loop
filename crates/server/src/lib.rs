pub mod cli;
pub mod http;

use std::sync::Arc;

use axum::extract::FromRef;
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_ingest::cache::DsnCache;
use maxi_loop_ingest::http::IngestState;
use maxi_loop_ingest::model::QueuedEnvelope;
use maxi_loop_ingest::worker::{IngestWorkerConfig, spawn_ingest_worker};
use maxi_loop_platform::config::Config;
use sqlx::PgPool;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub login_limiter: LoginRateLimiter,
    pub dsn_cache: DsnCache,
    pub ingest_tx: mpsc::Sender<QueuedEnvelope>,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> Self {
        let (state, rx) = Self::with_channel(config, db, LoginRateLimiter::default());
        let worker_cfg = IngestWorkerConfig::from_config(&state.config);
        spawn_ingest_worker(state.db.clone(), rx, worker_cfg);
        state
    }

    pub fn with_login_limiter(config: Config, db: PgPool, login_limiter: LoginRateLimiter) -> Self {
        let (state, rx) = Self::with_channel(config, db, login_limiter);
        let worker_cfg = IngestWorkerConfig::from_config(&state.config);
        spawn_ingest_worker(state.db.clone(), rx, worker_cfg);
        state
    }

    pub fn with_channel(
        config: Config,
        db: PgPool,
        login_limiter: LoginRateLimiter,
    ) -> (Self, mpsc::Receiver<QueuedEnvelope>) {
        let capacity = config.ingest_channel_capacity.max(1);
        let (ingest_tx, ingest_rx) = mpsc::channel(capacity);
        let state = Self {
            config: Arc::new(config),
            db,
            login_limiter,
            dsn_cache: DsnCache::new(),
            ingest_tx,
        };
        (state, ingest_rx)
    }
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        AuthState::with_dsn_observer(
            Arc::clone(&state.config),
            state.db.clone(),
            state.login_limiter.clone(),
            Arc::new(state.dsn_cache.clone()),
        )
    }
}

impl FromRef<AppState> for IngestState {
    fn from_ref(state: &AppState) -> Self {
        IngestState::new(
            Arc::clone(&state.config),
            state.dsn_cache.clone(),
            state.ingest_tx.clone(),
        )
    }
}
