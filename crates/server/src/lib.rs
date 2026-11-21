pub mod cli;
pub mod http;

use std::sync::Arc;

use maxi_loop_platform::config::Config;
use sqlx::PgPool;

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
