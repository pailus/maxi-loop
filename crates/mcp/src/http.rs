use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{FromRef, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use maxi_loop_auth::mcp::{McpAuditStatus, McpAuthService};
use maxi_loop_issues::service::IssuesService;
use maxi_loop_org::service::OrgService;
use maxi_loop_platform::config::Config;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{error, warn};

use crate::protocol::{
    ERR_INVALID_PARAMS, ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, ERR_PARSE, JsonRpcRequest,
    JsonRpcResponse, build_initialize_result, build_tool_error_result, build_tool_success_result,
    negotiate_protocol_version,
};
use crate::rate_limit::McpRateLimiter;
use crate::tools::{ToolServices, execute_tool, is_known_tool, list_tool_definitions};

const MAX_MCP_BODY_BYTES: usize = 256 * 1024;
const MCP_SESSION_HEADER: &str = "mcp-session-id";

#[derive(Clone)]
pub struct McpState {
    pub config: Arc<Config>,
    pub auth: McpAuthService,
    pub org: OrgService,
    pub issues: IssuesService,
    pub rate_limiter: McpRateLimiter,
}

impl McpState {
    pub fn new(
        config: Arc<Config>,
        auth: McpAuthService,
        org: OrgService,
        issues: IssuesService,
        rate_limiter: McpRateLimiter,
    ) -> Self {
        Self {
            config,
            auth,
            org,
            issues,
            rate_limiter,
        }
    }
}

pub fn router<S>() -> Router<S>
where
    McpState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/mcp",
        post(mcp_post_handler)
            .get(mcp_get_handler)
            .delete(mcp_delete_handler),
    )
}

async fn mcp_get_handler() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, HeaderValue::from_static("POST, DELETE"))],
        Json(json!({
            "error": "GET SSE stream is not offered; send JSON-RPC 2.0 POST requests to /mcp"
        })),
    )
        .into_response()
}

async fn mcp_delete_handler(State(state): State<McpState>, headers: HeaderMap) -> Response {
    let Some(raw_token) = extract_bearer_token(&headers) else {
        return unauthorized_response("missing bearer token");
    };
    if state.auth.authenticate_bearer(&raw_token).await.is_err() {
        return unauthorized_response("invalid or revoked mcp token");
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn mcp_post_handler(
    State(state): State<McpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(raw_token) = extract_bearer_token(&headers) else {
        return unauthorized_response("missing bearer token");
    };

    let auth = match state.auth.authenticate_bearer(&raw_token).await {
        Ok(ctx) => ctx,
        Err(AppError::Unauthorized(msg)) => return unauthorized_response(msg),
        Err(err) => {
            error!(error = %err, "mcp authentication error");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "service unavailable"})),
            )
                .into_response();
        }
    };

    if body.len() > MAX_MCP_BODY_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(JsonRpcResponse::err(
                Value::Null,
                ERR_INVALID_REQUEST,
                "request payload exceeds 256 KiB limit",
                None,
            )),
        )
            .into_response();
    }

    let req: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(JsonRpcResponse::err(
                    Value::Null,
                    ERR_PARSE,
                    "invalid JSON-RPC 2.0 payload",
                    None,
                )),
            )
                .into_response();
        }
    };

    if req.jsonrpc != "2.0" || req.method.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(JsonRpcResponse::err(
                req.id.unwrap_or(Value::Null),
                ERR_INVALID_REQUEST,
                "invalid JSON-RPC 2.0 request",
                None,
            )),
        )
            .into_response();
    }

    // Per-token rate limit check before executing any MCP method (`F-MCP-9`).
    if let Err(retry_after_secs) = state
        .rate_limiter
        .check(auth.token_id, auth.rate_limit_per_minute)
    {
        let tool_hint = extract_audit_method_label(&req);
        let _ = state
            .auth
            .record_audit(
                &auth,
                &tool_hint,
                &json!({}),
                McpAuditStatus::RateLimited,
                Some("rate_limited"),
                0,
            )
            .await;
        warn!(token_id = auth.token_id, "mcp token rate limit exceeded");
        let retry_header = HeaderValue::from_str(&retry_after_secs.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("1"));
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_header)],
            Json(json!({
                "error": "rate limit exceeded",
                "retry_after_secs": retry_after_secs
            })),
        )
            .into_response();
    }

    // Notifications (no `id`) receive HTTP 202 Accepted per MCP Streamable HTTP spec.
    let Some(req_id) = req.id else {
        return StatusCode::ACCEPTED.into_response();
    };

    match req.method.as_str() {
        "initialize" => {
            let negotiated = negotiate_protocol_version(req.params.as_ref());
            let session_id = format!("mlmcp-{}-{}", auth.tenant_ctx.tenant_id(), auth.token_id);
            let mut resp = Json(JsonRpcResponse::ok(
                req_id,
                build_initialize_result(negotiated),
            ))
            .into_response();
            if let Ok(hdr_val) = HeaderValue::from_str(&session_id) {
                resp.headers_mut().insert(MCP_SESSION_HEADER, hdr_val);
            }
            resp
        }
        "ping" => Json(JsonRpcResponse::ok(req_id, json!({}))).into_response(),
        "tools/list" => {
            let tools = list_tool_definitions(auth.can_read, auth.can_triage);
            Json(JsonRpcResponse::ok(req_id, json!({ "tools": tools }))).into_response()
        }
        "tools/call" => handle_tool_call(&state, &auth, req_id, req.params).await,
        other => Json(JsonRpcResponse::err(
            req_id,
            ERR_METHOD_NOT_FOUND,
            format!("method not found: {other}"),
            None,
        ))
        .into_response(),
    }
}

#[derive(Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn handle_tool_call(
    state: &McpState,
    auth: &maxi_loop_auth::mcp::McpAuthContext,
    req_id: Value,
    params: Option<Value>,
) -> Response {
    let started = Instant::now();
    let parsed: ToolCallParams = match params.and_then(|v| serde_json::from_value(v).ok()) {
        Some(p) => p,
        None => {
            return Json(JsonRpcResponse::err(
                req_id,
                ERR_INVALID_PARAMS,
                "tools/call requires params.name and optional params.arguments",
                None,
            ))
            .into_response();
        }
    };

    let tool_name = parsed.name.trim().to_owned();
    let args_for_audit = parsed.arguments.clone();

    if !is_known_tool(&tool_name) {
        let duration_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
        let _ = state
            .auth
            .record_audit(
                auth,
                &tool_name,
                &args_for_audit,
                McpAuditStatus::Error,
                Some("unknown_tool"),
                duration_ms,
            )
            .await;
        return Json(JsonRpcResponse::err(
            req_id,
            ERR_INVALID_PARAMS,
            format!("unknown tool: {tool_name}"),
            None,
        ))
        .into_response();
    }

    let services = ToolServices {
        org: &state.org,
        issues: &state.issues,
        public_base_url: &state.config.public_base_url,
    };

    let result = execute_tool(&services, auth, &tool_name, parsed.arguments).await;
    let duration_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);

    match result {
        Ok(output) => {
            if let Err(err) = state
                .auth
                .record_audit(
                    auth,
                    &tool_name,
                    &args_for_audit,
                    McpAuditStatus::Ok,
                    None,
                    duration_ms,
                )
                .await
            {
                warn!(error = %err, "failed to persist mcp audit log");
            }
            Json(JsonRpcResponse::ok(
                req_id,
                build_tool_success_result(output.text, output.structured),
            ))
            .into_response()
        }
        Err(AppError::Forbidden(msg)) => {
            let _ = state
                .auth
                .record_audit(
                    auth,
                    &tool_name,
                    &args_for_audit,
                    McpAuditStatus::Denied,
                    Some("forbidden"),
                    duration_ms,
                )
                .await;
            Json(JsonRpcResponse::ok(
                req_id,
                build_tool_error_result(format!("Permission denied: {msg}")),
            ))
            .into_response()
        }
        Err(AppError::Validation(msg)) => {
            let _ = state
                .auth
                .record_audit(
                    auth,
                    &tool_name,
                    &args_for_audit,
                    McpAuditStatus::Error,
                    Some("validation_error"),
                    duration_ms,
                )
                .await;
            Json(JsonRpcResponse::ok(
                req_id,
                build_tool_error_result(format!("Invalid arguments: {msg}")),
            ))
            .into_response()
        }
        Err(AppError::NotFound) => {
            let _ = state
                .auth
                .record_audit(
                    auth,
                    &tool_name,
                    &args_for_audit,
                    McpAuditStatus::Error,
                    Some("not_found"),
                    duration_ms,
                )
                .await;
            Json(JsonRpcResponse::ok(
                req_id,
                build_tool_error_result("Resource not found within your authorized scope."),
            ))
            .into_response()
        }
        Err(err) => {
            error!(error = %err, tool = %tool_name, "mcp tool execution failed");
            let _ = state
                .auth
                .record_audit(
                    auth,
                    &tool_name,
                    &args_for_audit,
                    McpAuditStatus::Error,
                    Some("internal_error"),
                    duration_ms,
                )
                .await;
            Json(JsonRpcResponse::ok(
                req_id,
                build_tool_error_result("Internal server error while executing MCP tool."),
            ))
            .into_response()
        }
    }
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    if raw.len() < 8 {
        return None;
    }
    let (scheme, rest) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}

fn extract_audit_method_label(req: &JsonRpcRequest) -> String {
    if req.method == "tools/call"
        && let Some(tool) = req
            .params
            .as_ref()
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
    {
        return tool.to_owned();
    }
    req.method.clone()
}

fn unauthorized_response(message: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
        Json(json!({ "error": message })),
    )
        .into_response()
}
