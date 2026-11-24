use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRef, Path, Query, State};
use axum::http::header::HeaderValue;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use maxi_loop_platform::config::Config;
use maxi_loop_platform::error::AppError;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{error, warn};

use crate::cache::DsnCache;
use crate::decompress::decompress_envelope_body;
use crate::envelope::{
    EnvelopeTarget, ItemSizeLimits, extract_dsn_public_key, parse_envelope_header_line,
    parse_envelope_items,
};
use crate::model::{EnvelopeIngestResponse, QueuedEnvelope};
use crate::spool::{EnqueueError, Spool};

pub const X_SENTRY_RATE_LIMITS: &str = "x-sentry-rate-limits";

#[derive(Clone)]
pub struct IngestState {
    pub config: Arc<Config>,
    pub dsn_cache: DsnCache,
    pub queue_tx: mpsc::Sender<QueuedEnvelope>,
    /// When set, envelopes are written ahead to disk before being queued (`F020`).
    pub spool: Option<Spool>,
}

impl IngestState {
    pub fn new(
        config: Arc<Config>,
        dsn_cache: DsnCache,
        queue_tx: mpsc::Sender<QueuedEnvelope>,
    ) -> Self {
        Self {
            config,
            dsn_cache,
            queue_tx,
            spool: None,
        }
    }

    pub fn with_spool(mut self, spool: Option<Spool>) -> Self {
        self.spool = spool;
        self
    }

    fn enqueue(&self, envelope: QueuedEnvelope) -> Result<(), EnqueueError> {
        match &self.spool {
            Some(spool) => spool.enqueue(envelope, &self.queue_tx),
            None => self.queue_tx.try_send(envelope).map_err(|err| match err {
                TrySendError::Full(_) => EnqueueError::Full,
                TrySendError::Closed(_) => EnqueueError::Closed,
            }),
        }
    }
}

pub fn router<S>() -> Router<S>
where
    IngestState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    // Raise Axum's default 2 MiB extractor limit to 64 MiB so our handler can return
    // structured 413 JSON responses based on `config.max_envelope_compressed_bytes`.
    // Attach non-credentialed CORS headers strictly to the public DSN ingest routes so
    // browser SDKs (`@sentry/react`) can POST cross-origin and read rate-limit headers.
    Router::new()
        .route(
            "/api/{project_id}/envelope/",
            post(ingest_envelope_handler).options(ingest_cors_preflight_handler),
        )
        .route(
            "/api/{project_id}/envelope",
            post(ingest_envelope_handler).options(ingest_cors_preflight_handler),
        )
        .layer(middleware::from_fn(apply_ingest_cors_headers))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}

async fn ingest_cors_preflight_handler() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn apply_ingest_cors_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, Content-Encoding, X-Sentry-Auth"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("X-Sentry-Rate-Limits, Retry-After"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("86400"),
    );
    resp
}

#[derive(Deserialize, Default)]
struct EnvelopeQuery {
    sentry_key: Option<String>,
}

#[derive(Serialize)]
struct RateLimitBody {
    error: &'static str,
    retry_after_secs: u64,
}

async fn ingest_envelope_handler(
    State(state): State<IngestState>,
    Path(project_id): Path<i64>,
    Query(query): Query<EnvelopeQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    if project_id <= 0 {
        return Err(AppError::Validation(
            "project_id must be a positive integer".to_owned(),
        ));
    }

    // Fast path: if `X-Sentry-Auth` or `?sentry_key=` is present, authenticate from the in-memory
    // DSN cache BEFORE decompressing the body so unauthorized requests cannot burn decompression CPU.
    let pre_body_key = extract_dsn_public_key(&headers, query.sentry_key.as_deref(), None);
    let mut cached_project = match &pre_body_key {
        Some(key) => Some(state.dsn_cache.authenticate(project_id, key)?),
        None => None,
    };

    let content_encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok());

    let decompressed = decompress_envelope_body(
        content_encoding,
        &body,
        state.config.max_envelope_compressed_bytes,
        state.config.max_envelope_decompressed_bytes,
    )
    .inspect_err(|err| {
        // Only log once the DSN is known to be valid, so anonymous clients cannot flood logs.
        if let Some(project) = &cached_project {
            log_oversized_rejection(err, project.tenant_id, project_id, body.len());
        }
    })?;

    let (envelope_header, cursor) = parse_envelope_header_line(&decompressed)?;

    // Fallback: if DSN key was only sent inside the envelope header `dsn` JSON field, authenticate now.
    let cached_project = match cached_project.take() {
        Some(proj) => proj,
        None => {
            let key = extract_dsn_public_key(
                &headers,
                query.sentry_key.as_deref(),
                envelope_header.dsn.as_deref(),
            )
            .ok_or(AppError::Unauthorized("missing dsn credentials"))?;
            state.dsn_cache.authenticate(project_id, &key)?
        }
    };

    let parsed = parse_envelope_items(
        envelope_header,
        &decompressed,
        cursor,
        ItemSizeLimits {
            event_bytes: state.config.max_event_payload_bytes,
            transaction_bytes: state.config.max_transaction_payload_bytes,
        },
        EnvelopeTarget {
            tenant_id: cached_project.tenant_id,
            workspace_id: cached_project.workspace_id,
            project_id: cached_project.project_id,
            received_at: Utc::now(),
        },
    )
    .inspect_err(|err| {
        log_oversized_rejection(
            err,
            cached_project.tenant_id,
            project_id,
            decompressed.len(),
        );
    })?;

    let accepted_events = parsed.envelope.events.len();
    let accepted_txs = parsed.envelope.transactions.len();
    let accepted_items = accepted_events + accepted_txs;
    let dropped_items = parsed.dropped_items;
    let envelope_id = parsed.envelope.envelope_id.clone();
    let limit_category = if accepted_events == 0 && accepted_txs > 0 {
        "transaction"
    } else {
        "error"
    };

    // If all items were unsupported (session, replay, profile, attachment) and dropped early,
    // acknowledge immediately without consuming project rate quota or worker queue capacity.
    if accepted_items == 0 {
        return Ok((
            StatusCode::OK,
            Json(EnvelopeIngestResponse {
                id: envelope_id,
                accepted_items: 0,
                dropped_items,
            }),
        )
            .into_response());
    }

    // Per-project and aggregate per-tenant `governor` rate limit check.
    let item_count_u32 = u32::try_from(accepted_items).unwrap_or(u32::MAX);
    if let Err(hit) = state
        .dsn_cache
        .check_rate_limit(&cached_project, item_count_u32)
    {
        return Ok(rate_limit_response(
            hit.retry_after_secs,
            limit_category,
            hit.scope.sentry_scope(),
        ));
    }

    // Push into bounded tokio::mpsc worker channel with non-blocking backpressure
    // (written ahead to the optional spool first).
    match state.enqueue(parsed.envelope) {
        Ok(()) => Ok((
            StatusCode::OK,
            Json(EnvelopeIngestResponse {
                id: envelope_id,
                accepted_items,
                dropped_items,
            }),
        )
            .into_response()),
        Err(EnqueueError::Full) => {
            warn!(
                project_id,
                tenant_id = cached_project.tenant_id,
                "ingest bounded channel is full; dropping envelope with 429 backpressure"
            );
            Ok(rate_limit_response(5, limit_category, "organization"))
        }
        Err(EnqueueError::Closed) => Err(AppError::DatabaseConnect(
            "ingest worker channel closed".to_owned(),
        )),
        Err(EnqueueError::Io(err)) => {
            error!(
                project_id,
                tenant_id = cached_project.tenant_id,
                error = %err,
                "failed to write envelope to ingest spool; rejecting with 503"
            );
            Ok(spool_unavailable_response())
        }
    }
}

/// Sentry SDKs drop a 413 response without retrying or logging (outside debug mode), so the
/// server log is the only place an operator can see that data was lost and which limit to raise.
fn log_oversized_rejection(err: &AppError, tenant_id: i64, project_id: i64, size_bytes: usize) {
    if let AppError::PayloadTooLarge(reason) = err {
        warn!(
            tenant_id,
            project_id,
            size_bytes,
            reason,
            "rejected oversized envelope with 413; the SDK drops it without retrying"
        );
    }
}

/// The envelope was not persisted or queued; SDKs retry on 5xx.
fn spool_unavailable_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER.as_str(), "5".to_owned())],
        Json(serde_json::json!({ "error": "ingest temporarily unavailable" })),
    )
        .into_response()
}

fn rate_limit_response(retry_after_secs: u64, category: &str, scope: &str) -> Response {
    let retry_secs = retry_after_secs.max(1);
    let retry_header = retry_secs.to_string();
    let sentry_limits = format!("{retry_secs}:{category}:{scope}");

    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (header::RETRY_AFTER.as_str(), retry_header),
            (X_SENTRY_RATE_LIMITS, sentry_limits),
        ],
        Json(RateLimitBody {
            error: "rate limit exceeded",
            retry_after_secs: retry_secs,
        }),
    )
        .into_response()
}
