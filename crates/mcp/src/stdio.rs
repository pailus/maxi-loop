//! `maxi-loop mcp` — minimal stdio-to-Streamable-HTTP MCP bridge.
//!
//! Mirrors the proven stdio proxy pattern used by `maxi-code` (`agentway mcp-board`):
//! reads line-delimited JSON-RPC 2.0 requests from `stdin`, writes single-line JSON-RPC
//! responses to `stdout` (all diagnostics go strictly to `stderr`), and relays every call
//! over HTTP/HTTPS (`hyper` + `tokio-rustls` + `webpki-roots`) to a Maxi Loop server's
//! `/mcp` endpoint using `Authorization: Bearer <MAXI_LOOP_MCP_TOKEN>`.

use std::fmt;
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

use http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST, RETRY_AFTER, USER_AGENT};
use http::{Method, Request};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tracing::{debug, warn};
use url::Url;

use crate::protocol::{
    ERR_METHOD_NOT_FOUND, ERR_PARSE, JsonRpcResponse, build_initialize_result,
    build_tool_error_result, negotiate_protocol_version,
};

const MAX_STDIO_LINE_BYTES: usize = 256 * 1024;
const MAX_HTTP_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_DELETE_TIMEOUT: Duration = Duration::from_secs(2);
const MCP_SESSION_HEADER: &str = "mcp-session-id";
const MCP_PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

const ERR_AUTH_FAILED: i32 = -32001;
const ERR_RATE_LIMITED: i32 = -32002;
const ERR_UPSTREAM_FAILED: i32 = -32000;

#[derive(Clone)]
pub struct StdioBridgeConfig {
    pub endpoint_url: Option<String>,
    pub token: Option<String>,
    pub timeout: Duration,
}

impl fmt::Debug for StdioBridgeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdioBridgeConfig")
            .field("endpoint_url", &self.endpoint_url)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl StdioBridgeConfig {
    pub fn from_env(
        url_override: Option<String>,
        token_env_name: Option<&str>,
    ) -> Result<Self, String> {
        let raw_url = url_override
            .or_else(|| std::env::var("MAXI_LOOP_URL").ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        let endpoint_url = match raw_url {
            Some(url) => Some(normalize_mcp_endpoint_url(&url)?),
            None => None,
        };

        let env_key = token_env_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("MAXI_LOOP_MCP_TOKEN");
        let token = std::env::var(env_key)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        Ok(Self {
            endpoint_url,
            token,
            timeout: DEFAULT_HTTP_TIMEOUT,
        })
    }
}

/// Normalize a user-provided Maxi Loop base URL or `/mcp` URL into a canonical HTTP/HTTPS `/mcp` URL.
pub fn normalize_mcp_endpoint_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("MAXI_LOOP_URL must not be empty".to_owned());
    }

    let mut parsed =
        Url::parse(trimmed).map_err(|err| format!("invalid MAXI_LOOP_URL '{trimmed}': {err}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "unsupported URL scheme '{other}' in MAXI_LOOP_URL (expected http or https)"
            ));
        }
    }
    if parsed.host_str().is_none() {
        return Err("MAXI_LOOP_URL must include a host".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("MAXI_LOOP_URL must not embed userinfo credentials".to_owned());
    }

    let path = parsed.path().trim_end_matches('/');
    if !path.ends_with("/mcp") {
        let next_path = if path.is_empty() {
            "/mcp".to_owned()
        } else {
            format!("{path}/mcp")
        };
        parsed.set_path(&next_path);
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed.to_string())
}

pub struct StdioBridge {
    config: StdioBridgeConfig,
    session_id: Option<String>,
    protocol_version: Option<String>,
}

impl StdioBridge {
    pub fn new(config: StdioBridgeConfig) -> Self {
        Self {
            config,
            session_id: None,
            protocol_version: None,
        }
    }

    /// Process a single line read from `stdin`. Returns `Some(response)` to emit on `stdout`,
    /// or `None` for empty lines and JSON-RPC notifications (no `id`).
    pub async fn handle_line(&mut self, line: &str) -> Option<Value> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                return Some(
                    serde_json::to_value(JsonRpcResponse::err(
                        Value::Null,
                        ERR_PARSE,
                        "invalid JSON-RPC 2.0 message",
                        None,
                    ))
                    .unwrap_or_else(|_| {
                        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}})
                    }),
                );
            }
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params");

        let (Some(endpoint_url), Some(token)) =
            (self.config.endpoint_url.clone(), self.config.token.clone())
        else {
            return self.handle_unconfigured(id, method, params);
        };

        let payload = match serde_json::to_vec(&req) {
            Ok(bytes) => bytes,
            Err(_) => {
                return id.map(|req_id| {
                    serde_json::to_value(JsonRpcResponse::err(
                        req_id,
                        ERR_PARSE,
                        "failed to encode JSON-RPC payload",
                        None,
                    ))
                    .unwrap_or_else(|_| json!({"jsonrpc":"2.0","id":null}))
                });
            }
        };

        let http_res = post_mcp_json(
            &endpoint_url,
            &token,
            payload,
            self.session_id.as_deref(),
            self.protocol_version.as_deref(),
            self.config.timeout,
        )
        .await;

        // Notifications (no `id`) never produce a JSON-RPC reply on stdout.
        let req_id = id?;

        match http_res {
            Ok(reply) => {
                if let Some(sid) = reply.session_id {
                    self.session_id = Some(sid);
                }
                if let Some(ver) = reply.protocol_version {
                    self.protocol_version = Some(ver);
                }

                if (200..300).contains(&reply.status) {
                    match serde_json::from_slice::<Value>(&reply.body) {
                        Ok(parsed) => {
                            if method == "initialize"
                                && let Some(ver) = parsed
                                    .get("result")
                                    .and_then(|r| r.get("protocolVersion"))
                                    .and_then(Value::as_str)
                            {
                                self.protocol_version = Some(ver.to_owned());
                            }
                            Some(parsed)
                        }
                        Err(err) => {
                            let msg =
                                format!("malformed JSON reply from Maxi Loop MCP server: {err}");
                            Some(error_reply_for_method(
                                method,
                                req_id,
                                ERR_UPSTREAM_FAILED,
                                msg,
                            ))
                        }
                    }
                } else if reply.status == 401 {
                    let msg =
                        "authentication failed: invalid, expired, or revoked Maxi Loop MCP token"
                            .to_owned();
                    Some(error_reply_for_method(method, req_id, ERR_AUTH_FAILED, msg))
                } else if reply.status == 429 {
                    let retry = reply.retry_after.as_deref().unwrap_or("a few");
                    let msg = format!(
                        "rate limit exceeded on Maxi Loop MCP token; retry after {retry} seconds"
                    );
                    Some(error_reply_for_method(
                        method,
                        req_id,
                        ERR_RATE_LIMITED,
                        msg,
                    ))
                } else {
                    let msg = format!("Maxi Loop MCP server returned HTTP {}", reply.status);
                    Some(error_reply_for_method(
                        method,
                        req_id,
                        ERR_UPSTREAM_FAILED,
                        msg,
                    ))
                }
            }
            Err(err) => {
                let safe = redact_token(&err, &token);
                let msg = format!("failed to reach Maxi Loop MCP server: {safe}");
                Some(error_reply_for_method(
                    method,
                    req_id,
                    ERR_UPSTREAM_FAILED,
                    msg,
                ))
            }
        }
    }

    fn handle_unconfigured(
        &mut self,
        id: Option<Value>,
        method: &str,
        params: Option<&Value>,
    ) -> Option<Value> {
        let req_id = id?;
        match method {
            "initialize" => {
                let negotiated = negotiate_protocol_version(params);
                self.protocol_version = Some(negotiated.to_owned());
                serde_json::to_value(JsonRpcResponse::ok(
                    req_id,
                    build_initialize_result(negotiated),
                ))
                .ok()
            }
            "ping" => serde_json::to_value(JsonRpcResponse::ok(req_id, json!({}))).ok(),
            "tools/list" => {
                serde_json::to_value(JsonRpcResponse::ok(req_id, json!({ "tools": [] }))).ok()
            }
            "prompts/list" => {
                serde_json::to_value(JsonRpcResponse::ok(req_id, json!({ "prompts": [] }))).ok()
            }
            "tools/call" => serde_json::to_value(JsonRpcResponse::ok(
                req_id,
                build_tool_error_result(
                    "error: missing MAXI_LOOP_URL or MAXI_LOOP_MCP_TOKEN environment variable",
                ),
            ))
            .ok(),
            _ => serde_json::to_value(JsonRpcResponse::err(
                req_id,
                ERR_METHOD_NOT_FOUND,
                format!("method unavailable without MAXI_LOOP_URL/MAXI_LOOP_MCP_TOKEN: {method}"),
                None,
            ))
            .ok(),
        }
    }

    /// Best-effort `DELETE /mcp` on stdin EOF to close the remote MCP session cleanly.
    pub async fn shutdown(&self) {
        let (Some(endpoint_url), Some(token), Some(session_id)) = (
            self.config.endpoint_url.as_deref(),
            self.config.token.as_deref(),
            self.session_id.as_deref(),
        ) else {
            return;
        };

        if let Err(err) = delete_mcp_session(
            endpoint_url,
            token,
            session_id,
            self.protocol_version.as_deref(),
            SHUTDOWN_DELETE_TIMEOUT,
        )
        .await
        {
            debug!(error = %redact_token(&err, token), "best-effort MCP session DELETE failed");
        }
    }
}

/// Run the stdio MCP bridge loop until `stdin` reaches EOF.
pub async fn run_stdio_bridge(config: StdioBridgeConfig) -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel::<Result<String, String>>(32);

    std::thread::Builder::new()
        .name("mcp-stdio-reader".to_owned())
        .spawn(move || {
            let stdin = io::stdin();
            let mut lock = stdin.lock();
            loop {
                match read_bounded_line(&mut lock, MAX_STDIO_LINE_BYTES) {
                    Ok(None) => break,
                    Ok(Some(line)) => {
                        if tx.blocking_send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.blocking_send(Err(err));
                        break;
                    }
                }
            }
        })
        .map_err(|err| format!("spawn mcp stdio reader thread: {err}"))?;

    let mut bridge = StdioBridge::new(config);
    let mut stdout = io::stdout();

    while let Some(item) = rx.recv().await {
        let line = match item {
            Ok(l) => l,
            Err(err) => {
                warn!(error = %err, "mcp stdio read error");
                break;
            }
        };

        if let Some(resp) = bridge.handle_line(&line).await {
            let encoded = serde_json::to_string(&resp)
                .map_err(|err| format!("serialize mcp stdio response: {err}"))?;
            if let Err(err) = stdout
                .write_all(encoded.as_bytes())
                .and_then(|()| stdout.write_all(b"\n"))
                .and_then(|()| stdout.flush())
            {
                return Err(format!("write mcp stdio response: {err}"));
            }
        }
    }

    bridge.shutdown().await;
    Ok(())
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    let mut buf = Vec::new();
    let mut total = 0usize;
    loop {
        let available = reader
            .fill_buf()
            .map_err(|err| format!("read stdin: {err}"))?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }

        let newline_pos = available.iter().position(|&b| b == b'\n');
        let take = newline_pos.map_or(available.len(), |pos| pos + 1);
        total = total.saturating_add(take);
        if total > max_bytes {
            return Err(format!(
                "stdin JSON-RPC line exceeds {max_bytes} bytes limit"
            ));
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline_pos.is_some() {
            break;
        }
    }

    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| format!("stdin line is not valid UTF-8: {err}"))
}

fn error_reply_for_method(method: &str, req_id: Value, code: i32, message: String) -> Value {
    if method == "tools/call" {
        serde_json::to_value(JsonRpcResponse::ok(
            req_id,
            build_tool_error_result(format!("error: {message}")),
        ))
        .unwrap_or_else(|_| json!({"jsonrpc":"2.0","id":null}))
    } else {
        serde_json::to_value(JsonRpcResponse::err(req_id, code, message, None))
            .unwrap_or_else(|_| json!({"jsonrpc":"2.0","id":null}))
    }
}

fn redact_token(input: &str, token: &str) -> String {
    if token.is_empty() {
        input.to_owned()
    } else {
        input.replace(token, "[REDACTED]")
    }
}

struct HttpReply {
    status: u16,
    body: Bytes,
    session_id: Option<String>,
    protocol_version: Option<String>,
    retry_after: Option<String>,
}

async fn post_mcp_json(
    endpoint_url: &str,
    token: &str,
    body: Vec<u8>,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
    req_timeout: Duration,
) -> Result<HttpReply, String> {
    send_mcp_http(
        Method::POST,
        endpoint_url,
        token,
        Some(body),
        session_id,
        protocol_version,
        req_timeout,
    )
    .await
}

async fn delete_mcp_session(
    endpoint_url: &str,
    token: &str,
    session_id: &str,
    protocol_version: Option<&str>,
    req_timeout: Duration,
) -> Result<HttpReply, String> {
    send_mcp_http(
        Method::DELETE,
        endpoint_url,
        token,
        None,
        Some(session_id),
        protocol_version,
        req_timeout,
    )
    .await
}

async fn send_mcp_http(
    method: Method,
    endpoint_url: &str,
    token: &str,
    body: Option<Vec<u8>>,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
    req_timeout: Duration,
) -> Result<HttpReply, String> {
    let parsed = Url::parse(endpoint_url).map_err(|err| format!("parse endpoint url: {err}"))?;
    let scheme = parsed.scheme().to_owned();
    let host = parsed
        .host_str()
        .ok_or_else(|| "endpoint url missing host".to_owned())?
        .to_owned();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| "endpoint url missing port".to_owned())?;
    let path_and_query = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_owned(),
    };

    let host_header = if (scheme == "https" && port == 443) || (scheme == "http" && port == 80) {
        host.clone()
    } else if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };

    let mut builder = Request::builder()
        .method(method)
        .uri(&path_and_query)
        .header(HOST, &host_header)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(ACCEPT, "application/json, text/event-stream")
        .header(
            USER_AGENT,
            concat!("maxi-loop-mcp-stdio/", env!("CARGO_PKG_VERSION")),
        );

    if body.is_some() {
        builder = builder.header(CONTENT_TYPE, "application/json");
    }
    if let Some(sid) = session_id {
        builder = builder.header(MCP_SESSION_HEADER, sid);
    }
    if let Some(ver) = protocol_version {
        builder = builder.header(MCP_PROTOCOL_VERSION_HEADER, ver);
    }

    let req = builder
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .map_err(|err| format!("build request: {err}"))?;

    let connect_target = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };

    let fut = async move {
        let tcp = TcpStream::connect(&connect_target)
            .await
            .map_err(|err| format!("connect to {connect_target}: {err}"))?;

        if scheme == "https" {
            let connector = build_tls_connector()?;
            let server_name =
                ServerName::try_from(host).map_err(|_| "invalid TLS server name".to_owned())?;
            let tls_stream = connector
                .connect(server_name, tcp)
                .await
                .map_err(|err| format!("TLS handshake failed: {err}"))?;
            execute_http1(TokioIo::new(tls_stream), req).await
        } else if scheme == "http" {
            execute_http1(TokioIo::new(tcp), req).await
        } else {
            Err(format!("unsupported URL scheme '{scheme}'"))
        }
    };

    timeout(req_timeout, fut)
        .await
        .map_err(|_| format!("request timed out after {}s", req_timeout.as_secs()))?
}

async fn execute_http1<IO>(io: TokioIo<IO>, req: Request<Full<Bytes>>) -> Result<HttpReply, String>
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, conn) = http1::handshake(io)
        .await
        .map_err(|err| format!("HTTP/1.1 handshake failed: {err}"))?;
    let conn_task = tokio::spawn(async move {
        let _ = conn.await;
    });

    let resp = sender
        .send_request(req)
        .await
        .map_err(|err| format!("send request failed: {err}"))?;

    let status = resp.status().as_u16();
    let session_id = resp
        .headers()
        .get(MCP_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let protocol_version = resp
        .headers()
        .get(MCP_PROTOCOL_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let retry_after = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let mut body = resp.into_body();
    let mut collected = Vec::new();
    while let Some(frame_res) = body.frame().await {
        let frame = frame_res.map_err(|err| format!("read response body: {err}"))?;
        if let Some(chunk) = frame.data_ref() {
            if collected.len().saturating_add(chunk.len()) > MAX_HTTP_RESPONSE_BYTES {
                conn_task.abort();
                return Err(format!(
                    "MCP response exceeds {MAX_HTTP_RESPONSE_BYTES} bytes limit"
                ));
            }
            collected.extend_from_slice(chunk);
        }
    }

    conn_task.abort();
    Ok(HttpReply {
        status,
        body: Bytes::from(collected),
        session_id,
        protocol_version,
        retry_after,
    })
}

fn build_tls_connector() -> Result<TlsConnector, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|err| format!("configure TLS protocol versions: {err}"))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use tokio::net::TcpListener;

    #[test]
    fn normalizes_base_and_mcp_urls_and_rejects_bad_schemes() {
        assert_eq!(
            normalize_mcp_endpoint_url("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080/mcp"
        );
        assert_eq!(
            normalize_mcp_endpoint_url("https://errors.example.com/").unwrap(),
            "https://errors.example.com/mcp"
        );
        assert_eq!(
            normalize_mcp_endpoint_url("https://errors.example.com/sub/mcp").unwrap(),
            "https://errors.example.com/sub/mcp"
        );
        assert!(normalize_mcp_endpoint_url("file:///etc/passwd").is_err());
        assert!(normalize_mcp_endpoint_url("http://user:pass@127.0.0.1:8080").is_err());
    }

    #[tokio::test]
    async fn unconfigured_bridge_initializes_and_reports_missing_env_on_tool_call() {
        let mut bridge = StdioBridge::new(StdioBridgeConfig {
            endpoint_url: None,
            token: None,
            timeout: Duration::from_secs(5),
        });

        let init = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#)
            .await
            .expect("initialize reply");
        assert_eq!(init["result"]["serverInfo"]["name"], "maxi-loop");

        // Notification yields no output.
        assert!(
            bridge
                .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .await
                .is_none()
        );

        let list = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .expect("tools/list reply");
        assert_eq!(list["result"]["tools"], json!([]));

        let call = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_issues","arguments":{}}}"#,
            )
            .await
            .expect("tools/call reply");
        assert_eq!(call["result"]["isError"], true);
        assert!(
            call["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("MAXI_LOOP_URL")
        );
    }

    #[tokio::test]
    async fn relays_requests_tracks_session_headers_and_handles_auth_and_rate_limit() {
        async fn mock_mcp(
            State(expected_tok): State<String>,
            headers: HeaderMap,
            body: Bytes,
        ) -> impl IntoResponse {
            let auth = headers
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if auth != format!("Bearer {expected_tok}") {
                return (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"error": "unauthorized"})),
                )
                    .into_response();
            }

            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let method = req["method"].as_str().unwrap_or("");
            let id = req.get("id").cloned();
            if id.is_none() {
                return StatusCode::ACCEPTED.into_response();
            }
            let req_id = id.unwrap();

            if method == "initialize" {
                let mut resp = axum::Json(json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "maxi-loop", "version": "0.1.0"}
                    }
                }))
                .into_response();
                resp.headers_mut()
                    .insert(MCP_SESSION_HEADER, HeaderValue::from_static("mlmcp-1-99"));
                resp.headers_mut().insert(
                    MCP_PROTOCOL_VERSION_HEADER,
                    HeaderValue::from_static("2025-06-18"),
                );
                return resp;
            }

            if method == "tools/call" && req["params"]["name"].as_str() == Some("rate_limit_me") {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(RETRY_AFTER, HeaderValue::from_static("12"))],
                    axum::Json(json!({"error": "rate limit exceeded"})),
                )
                    .into_response();
            }

            // Echo headers so we can assert session_id & protocol_version propagation.
            let sid = headers
                .get(MCP_SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let ver = headers
                .get(MCP_PROTOCOL_VERSION_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            axum::Json(json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "tools": [],
                    "seen_session_id": sid,
                    "seen_protocol_version": ver
                }
            }))
            .into_response()
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let token = "mlmcp_test_secret_token_123456".to_owned();
        let app = Router::new()
            .route("/mcp", post(mock_mcp))
            .with_state(token.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut bridge = StdioBridge::new(StdioBridgeConfig {
            endpoint_url: Some(format!("http://{addr}/mcp")),
            token: Some(token.clone()),
            timeout: Duration::from_secs(5),
        });

        let init = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#)
            .await
            .expect("init");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

        // Notification produces no stdout output.
        assert!(
            bridge
                .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .await
                .is_none()
        );

        let list = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .expect("list");
        assert_eq!(list["result"]["seen_session_id"], "mlmcp-1-99");
        assert_eq!(list["result"]["seen_protocol_version"], "2025-06-18");

        // 429 on tools/call surfaces cleanly as tool error with Retry-After.
        let limited = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rate_limit_me","arguments":{}}}"#,
            )
            .await
            .expect("429");
        assert_eq!(limited["result"]["isError"], true);
        assert!(
            limited["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("12 seconds")
        );

        // Wrong token fails with 401 and never leaks the token string.
        let mut bad_bridge = StdioBridge::new(StdioBridgeConfig {
            endpoint_url: Some(format!("http://{addr}/mcp")),
            token: Some("mlmcp_wrong_secret_999".to_owned()),
            timeout: Duration::from_secs(5),
        });
        let unauth = bad_bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#)
            .await
            .expect("401");
        assert_eq!(unauth["error"]["code"], ERR_AUTH_FAILED);
        assert!(!unauth.to_string().contains("mlmcp_wrong_secret_999"));

        server.abort();
    }

    #[test]
    fn bounded_line_reader_rejects_oversized_lines() {
        let mut cursor = io::Cursor::new(b"abcdefg\n".to_vec());
        assert!(read_bounded_line(&mut cursor, 4).is_err());

        let mut ok_cursor = io::Cursor::new(b"abc\n".to_vec());
        assert_eq!(
            read_bounded_line(&mut ok_cursor, 4).unwrap().as_deref(),
            Some("abc\n")
        );
    }
}
