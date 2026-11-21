use std::net::SocketAddr;

use chrono::Utc;
use maxi_loop::AppState;
use maxi_loop::cli::{Command, parse_args};
use maxi_loop::http::build_router;
use maxi_loop_platform::bootstrap::{BootstrapSpec, bootstrap_defaults};
use maxi_loop_platform::config::{Config, LogFormat};
use maxi_loop_platform::db::{connect_db, ensure_upcoming_events_partitions, run_migrations};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    let command = match parse_args(std::env::args()) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("argument error: {err}");
            std::process::exit(2);
        }
    };

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

    if let Err(err) = run_migrations(&db).await {
        error!(error = %err, "failed to run database migrations");
        std::process::exit(1);
    }

    let today_utc = Utc::now().date_naive();
    if let Err(err) = ensure_upcoming_events_partitions(&db, today_utc, 7).await {
        error!(error = %err, "failed to ensure upcoming events partitions");
        std::process::exit(1);
    }

    match command {
        Command::Bootstrap(overrides) => {
            let spec = match overrides.apply_to(&config) {
                Ok(spec) => spec,
                Err(err) => {
                    error!(error = %err, "invalid bootstrap parameters");
                    std::process::exit(2);
                }
            };
            match bootstrap_defaults(&db, &spec).await {
                Ok(outcome) => {
                    info!(
                        tenant_id = outcome.tenant_id,
                        tenant_slug = %outcome.tenant_slug,
                        workspace_id = outcome.workspace_id,
                        workspace_slug = %outcome.workspace_slug,
                        admin_user_id = ?outcome.admin_user_id,
                        admin_email = ?outcome.admin_email,
                        "bootstrap completed"
                    );
                    return;
                }
                Err(err) => {
                    error!(error = %err, "bootstrap failed");
                    std::process::exit(1);
                }
            }
        }
        Command::Serve => {
            let spec = BootstrapSpec::from_config(&config);
            match bootstrap_defaults(&db, &spec).await {
                Ok(outcome) => {
                    info!(
                        tenant_id = outcome.tenant_id,
                        tenant_slug = %outcome.tenant_slug,
                        workspace_id = outcome.workspace_id,
                        workspace_slug = %outcome.workspace_slug,
                        admin_user_id = ?outcome.admin_user_id,
                        "verified default tenant and workspace"
                    );
                }
                Err(err) => {
                    error!(error = %err, "first-boot bootstrap failed");
                    std::process::exit(1);
                }
            }
        }
    }

    let addr: SocketAddr = match config.socket_addr() {
        Ok(addr) => addr,
        Err(err) => {
            error!(error = %err, "invalid bind address");
            std::process::exit(1);
        }
    };

    let state = AppState::new(config, db);
    match state.dsn_cache.reload_from_db(&state.db).await {
        Ok(loaded_projects) => {
            info!(loaded_projects, "preloaded project DSN cache into memory");
        }
        Err(err) => {
            error!(error = %err, "failed to preload project DSN cache");
            std::process::exit(1);
        }
    }
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
