use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use tokio::time::timeout;
use tracing::warn;

use crate::AppState;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
    database: &'static str,
}

pub async fn check(State(state): State<AppState>) -> impl IntoResponse {
    let db_check = timeout(
        Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&state.db),
    )
    .await;

    match db_check {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ok",
                service: env!("CARGO_PKG_NAME"),
                version: env!("CARGO_PKG_VERSION"),
                database: "ok",
            }),
        ),
        Ok(Err(err)) => {
            warn!(error = %err, "database health check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "degraded",
                    service: env!("CARGO_PKG_NAME"),
                    version: env!("CARGO_PKG_VERSION"),
                    database: "unavailable",
                }),
            )
        }
        Err(_) => {
            warn!("database health check timed out");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "degraded",
                    service: env!("CARGO_PKG_NAME"),
                    version: env!("CARGO_PKG_VERSION"),
                    database: "unavailable",
                }),
            )
        }
    }
}
