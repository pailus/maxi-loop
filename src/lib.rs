pub mod config;
pub mod error;
pub mod http;

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::Config;
use crate::error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> Self {
        Self {
            config: Arc::new(config),
            db,
        }
    }
}

/// Build a PostgreSQL connection pool with bounded acquire timeout.
pub async fn connect_db(config: &Config) -> Result<PgPool, AppError> {
    PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .map_err(|err| AppError::DatabaseConnect(err.to_string()))
}
