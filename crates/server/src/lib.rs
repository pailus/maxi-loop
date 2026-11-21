pub mod cli;
pub mod http;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::FromRef;
use maxi_loop_alerts::delivery::DeliveryPolicy;
use maxi_loop_alerts::engine::{AlertSignalSender, spawn_alert_engine};
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::rate_limit::LoginRateLimiter;
use maxi_loop_ingest::cache::DsnCache;
use maxi_loop_ingest::http::IngestState;
use maxi_loop_ingest::model::QueuedEnvelope;
use maxi_loop_ingest::scrub_cache::ScrubRulesCache;
use maxi_loop_ingest::worker::{IngestWorkerConfig, spawn_ingest_worker_with_hooks};
use maxi_loop_platform::config::Config;
use sqlx::PgPool;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub login_limiter: LoginRateLimiter,
    pub dsn_cache: DsnCache,
    pub scrub_cache: ScrubRulesCache,
    pub ingest_tx: mpsc::Sender<QueuedEnvelope>,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> Self {
        Self::with_options(
            config,
            db,
            LoginRateLimiter::default(),
            None,
            Duration::from_secs(60),
        )
    }

    pub fn with_login_limiter(config: Config, db: PgPool, login_limiter: LoginRateLimiter) -> Self {
        Self::with_options(config, db, login_limiter, None, Duration::from_secs(60))
    }

    pub fn with_alert_policy(
        config: Config,
        db: PgPool,
        login_limiter: LoginRateLimiter,
        delivery_policy: DeliveryPolicy,
        spike_interval: Duration,
    ) -> Self {
        Self::with_options(
            config,
            db,
            login_limiter,
            Some(delivery_policy),
            spike_interval,
        )
    }

    fn with_options(
        config: Config,
        db: PgPool,
        login_limiter: LoginRateLimiter,
        custom_policy: Option<DeliveryPolicy>,
        spike_interval: Duration,
    ) -> Self {
        let (state, rx) = Self::with_channel(config, db, login_limiter);
        let worker_cfg = IngestWorkerConfig::from_config(&state.config);
        let (alert_sender, alert_rx) = AlertSignalSender::new(1024);
        let policy =
            custom_policy.unwrap_or_else(|| DeliveryPolicy::strict(state.config.smtp.clone()));
        spawn_alert_engine(
            state.db.clone(),
            state.config.public_base_url.clone(),
            alert_rx,
            policy,
            spike_interval,
        );
        spawn_ingest_worker_with_hooks(
            state.db.clone(),
            rx,
            worker_cfg,
            Some(state.scrub_cache.clone()),
            Some(Arc::new(alert_sender)),
        );
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
            scrub_cache: ScrubRulesCache::new(),
            ingest_tx,
        };
        (state, ingest_rx)
    }
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        AuthState::with_observers(
            Arc::clone(&state.config),
            state.db.clone(),
            state.login_limiter.clone(),
            Arc::new(state.dsn_cache.clone()),
            Arc::new(state.scrub_cache.clone()),
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
