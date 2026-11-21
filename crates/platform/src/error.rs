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
    #[error("database migration failed: {0}")]
    DatabaseMigration(String),
    #[error("database query failed: {0}")]
    DatabaseQuery(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("cryptographic operation failed: {0}")]
    Crypto(String),
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
            AppError::Validation(_) => (StatusCode::BAD_REQUEST, "invalid request"),
            AppError::DatabaseConnect(_)
            | AppError::DatabaseMigration(_)
            | AppError::DatabaseQuery(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "service unavailable")
            }
            AppError::Crypto(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal server error"),
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}
