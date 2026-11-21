use axum::Json;
use axum::http::{StatusCode, header};
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
    #[error("unauthorized: {0}")]
    Unauthorized(&'static str),
    #[error("forbidden: {0}")]
    Forbidden(&'static str),
    #[error("conflict: {0}")]
    Conflict(&'static str),
    #[error("rate limited (retry after {retry_after_secs}s)")]
    RateLimited { retry_after_secs: u64 },
    #[error("not found")]
    NotFound,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::RateLimited { retry_after_secs } => {
                let retry_val = retry_after_secs.max(1).to_string();
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(header::RETRY_AFTER, retry_val)],
                    Json(ErrorBody {
                        error: "too many requests",
                    }),
                )
                    .into_response()
            }
            other => {
                let (status, message) = match other {
                    AppError::NotFound => (StatusCode::NOT_FOUND, "not found"),
                    AppError::Validation(_) => (StatusCode::BAD_REQUEST, "invalid request"),
                    AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
                    AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
                    AppError::Conflict(msg) => (StatusCode::CONFLICT, msg),
                    AppError::DatabaseConnect(_)
                    | AppError::DatabaseMigration(_)
                    | AppError::DatabaseQuery(_) => {
                        (StatusCode::SERVICE_UNAVAILABLE, "service unavailable")
                    }
                    AppError::Crypto(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
                    }
                    AppError::RateLimited { .. } => unreachable!(),
                };
                (status, Json(ErrorBody { error: message })).into_response()
            }
        }
    }
}
