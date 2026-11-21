use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}: {1}")]
    InvalidValue(&'static str, &'static str),
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database connection failed: {0}")]
    DatabaseConnect(String),
    #[error("not found")]
    NotFound,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found"),
            AppError::DatabaseConnect(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "service unavailable")
            }
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}
