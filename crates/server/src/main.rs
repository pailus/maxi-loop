use std::net::SocketAddr;

use maxi_loop::AppState;
use maxi_loop::http::build_router;
use maxi_loop_platform::config::{Config, LogFormat};
use maxi_loop_platform::db::connect_db;
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    let config = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("configuration error: {err}");
            std::process::exit(1);
        }
    };

    init_tracing(config.log_format);
    info!(?config, "starting maxi-loop");

    let db = match connect_db(&config).await {
        Ok(pool) => pool,
        Err(err) => {
            error!(error = %err, "failed to connect to PostgreSQL");
            std::process::exit(1);
        }
    };

    let addr: SocketAddr = match config.socket_addr() {
        Ok(addr) => addr,
        Err(err) => {
            error!(error = %err, "invalid bind address");
            std::process::exit(1);
        }
    };

    let state = AppState::new(config, db);
    let app = build_router(state);

    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            error!(%addr, error = %err, "failed to bind TCP listener");
            std::process::exit(1);
        }
    };

    info!(%addr, "listening");

    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        error!(error = %err, "server exited with error");
        std::process::exit(1);
    }

    info!("server shut down cleanly");
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("maxi_loop=info,tower_http=info"));

    match format {
        LogFormat::Pretty => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!(error = %err, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                error!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    info!("shutdown signal received");
}
