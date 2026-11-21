use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRef, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use maxi_loop_platform::config::Config;
use maxi_loop_platform::error::AppError;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::warn;

use crate::cache::DsnCache;
use crate::decompress::decompress_envelope_body;
use crate::envelope::{
    EnvelopeTarget, extract_dsn_public_key, parse_envelope_header_line, parse_envelope_items,
};
use crate::model::{EnvelopeIngestResponse, QueuedEnvelope};

pub const X_SENTRY_RATE_LIMITS: &str = "x-sentry-rate-limits";

#[derive(Clone)]
pub struct IngestState {
    pub config: Arc<Config>,
    pub dsn_cache: DsnCache,
    pub queue_tx: mpsc::Sender<QueuedEnvelope>,
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
    Router::new()
        .route("/api/{project_id}/envelope/", post(ingest_envelope_handler))
        .route("/api/{project_id}/envelope", post(ingest_envelope_handler))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
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
    )?;

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
        state.config.max_event_payload_bytes,
        EnvelopeTarget {
            tenant_id: cached_project.tenant_id,
            workspace_id: cached_project.workspace_id,
            project_id: cached_project.project_id,
            received_at: Utc::now(),
        },
    )?;

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

    // Push into bounded tokio::mpsc worker channel with non-blocking backpressure.
    match state.queue_tx.try_send(parsed.envelope) {
        Ok(()) => Ok((
            StatusCode::OK,
            Json(EnvelopeIngestResponse {
                id: envelope_id,
                accepted_items,
                dropped_items,
            }),
        )
            .into_response()),
        Err(TrySendError::Full(_)) => {
            warn!(
                project_id,
                tenant_id = cached_project.tenant_id,
                "ingest bounded channel is full; dropping envelope with 429 backpressure"
            );
            Ok(rate_limit_response(5, limit_category, "organization"))
        }
        Err(TrySendError::Closed(_)) => Err(AppError::DatabaseConnect(
            "ingest worker channel closed".to_owned(),
        )),
    }
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
