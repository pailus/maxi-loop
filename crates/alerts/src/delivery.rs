use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use http::header::{CONTENT_TYPE, HOST, USER_AGENT};
use http::{Method, Request};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use maxi_loop_platform::config::SmtpConfig;
use maxi_loop_platform::error::AppError;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

use crate::model::AlertChannelType;
use crate::ssrf::resolve_and_validate_url;

type HmacSha256 = Hmac<Sha256>;

const DEFAULT_DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
const OFFICIAL_TELEGRAM_API_BASE: &str = "https://api.telegram.org";

#[derive(Clone, Debug)]
pub struct DeliveryPolicy {
    pub allow_private_ips: bool,
    pub telegram_api_base: String,
    pub timeout: Duration,
    pub smtp: Option<SmtpConfig>,
}

impl DeliveryPolicy {
    /// Strict production delivery policy: blocks all private/loopback/metadata IPs (SSRF),
    /// enforces official Telegram API origin, verifies TLS via `webpki-roots`, and bounds timeouts.
    pub fn strict(smtp: Option<SmtpConfig>) -> Self {
        Self {
            allow_private_ips: false,
            telegram_api_base: OFFICIAL_TELEGRAM_API_BASE.to_owned(),
            timeout: DEFAULT_DELIVERY_TIMEOUT,
            smtp,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AlertPayload {
    pub event: String,
    pub tenant_id: i64,
    pub project_id: i64,
    pub project_slug: String,
    /// Omitted for performance alerts (`F019`), which are not tied to an issue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<i64>,
    pub title: String,
    pub exception_type: String,
    pub exception_value: String,
    pub culprit: String,
    pub level: String,
    pub environment: String,
    pub release: Option<String>,
    pub event_count: i64,
    pub issue_url: String,
    pub triggered_at: DateTime<Utc>,
    pub details: Option<String>,
    /// `F019` performance alert context (omitted for issue alerts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance: Option<PerformanceAlertContext>,
}

/// Metric context of a performance alert (`F019`).
#[derive(Clone, Debug, Serialize)]
pub struct PerformanceAlertContext {
    /// `p95_ms` or `error_rate_pct`.
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
    pub window_minutes: i32,
    /// `None` = all transactions of the project.
    pub transaction: Option<String>,
    pub transaction_count: i64,
}

#[derive(Clone, Debug)]
pub struct DeliveryOutcome {
    pub ok: bool,
    pub http_status: Option<i32>,
    pub error_message: Option<String>,
}

pub async fn dispatch_alert(
    channel_type: AlertChannelType,
    channel_config: &Value,
    payload: &AlertPayload,
    policy: &DeliveryPolicy,
) -> DeliveryOutcome {
    let res = match channel_type {
        AlertChannelType::Webhook => send_webhook(channel_config, payload, policy).await,
        AlertChannelType::Telegram => send_telegram(channel_config, payload, policy).await,
        AlertChannelType::Email => send_email(channel_config, payload, policy).await,
    };

    match res {
        Ok(http_status) => DeliveryOutcome {
            ok: true,
            http_status,
            error_message: None,
        },
        Err((http_status, msg)) => DeliveryOutcome {
            ok: false,
            http_status,
            error_message: Some(truncate_error(&msg)),
        },
    }
}

async fn send_webhook(
    config: &Value,
    payload: &AlertPayload,
    policy: &DeliveryPolicy,
) -> Result<Option<i32>, (Option<i32>, String)> {
    let url = config
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| (None, "webhook channel missing url".to_owned()))?;
    let secret = config
        .get("secret")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let body = serde_json::to_vec(payload)
        .map_err(|err| (None, format!("serialize webhook payload: {err}")))?;

    let mut extra_headers = Vec::new();
    if let Some(secret_key) = secret {
        let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes())
            .map_err(|_| (None, "invalid webhook signing secret".to_owned()))?;
        mac.update(&body);
        let sig_hex = hex::encode(mac.finalize().into_bytes());
        extra_headers.push((
            "x-maxiloop-signature".to_owned(),
            format!("sha256={sig_hex}"),
        ));
    }
    extra_headers.push(("x-maxiloop-event".to_owned(), payload.event.clone()));

    let status = post_json_ssrf_safe(url, body, &extra_headers, policy).await?;
    if (200..300).contains(&status) {
        Ok(Some(i32::from(status)))
    } else {
        Err((
            Some(i32::from(status)),
            format!("webhook returned HTTP {status}"),
        ))
    }
}

async fn send_telegram(
    config: &Value,
    payload: &AlertPayload,
    policy: &DeliveryPolicy,
) -> Result<Option<i32>, (Option<i32>, String)> {
    let bot_token = config
        .get("bot_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| (None, "telegram channel missing bot_token".to_owned()))?;
    let chat_id = config
        .get("chat_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| (None, "telegram channel missing chat_id".to_owned()))?;

    let base = policy.telegram_api_base.trim_end_matches('/');
    let endpoint = format!("{base}/bot{bot_token}/sendMessage");

    let text = format_alert_text(payload);
    let body = serde_json::to_vec(&json!({
        "chat_id": chat_id,
        "text": text,
        "disable_web_page_preview": true
    }))
    .map_err(|err| (None, format!("serialize telegram payload: {err}")))?;

    let status = post_json_ssrf_safe(&endpoint, body, &[], policy)
        .await
        .map_err(|(code, err)| {
            // Never include the bot token in any error string.
            (code, err.replace(bot_token, "[REDACTED]"))
        })?;

    if (200..300).contains(&status) {
        Ok(Some(i32::from(status)))
    } else {
        Err((
            Some(i32::from(status)),
            format!("telegram api returned HTTP {status}"),
        ))
    }
}

async fn send_email(
    config: &Value,
    payload: &AlertPayload,
    policy: &DeliveryPolicy,
) -> Result<Option<i32>, (Option<i32>, String)> {
    let smtp = policy.smtp.as_ref().ok_or_else(|| {
        (
            None,
            "SMTP is not configured on this server (set SMTP_HOST & SMTP_FROM)".to_owned(),
        )
    })?;

    let recipients: Vec<String> = config
        .get("to")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    if recipients.is_empty() {
        return Err((None, "email channel has no recipients".to_owned()));
    }

    let prefix = config
        .get("subject_prefix")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("[MaxiLoop]");

    let subject = sanitize_header_line(&format!(
        "{prefix} [{}] {} — {}",
        payload.event.to_uppercase(),
        payload.project_slug,
        payload.title
    ));
    let body = format_alert_text(payload);

    timeout(
        policy.timeout,
        send_smtp_message(smtp, &recipients, &subject, &body),
    )
    .await
    .map_err(|_| (None, "SMTP delivery timed out".to_owned()))?
    .map_err(|err| (None, format!("SMTP delivery failed: {err}")))?;

    Ok(Some(250))
}

fn format_alert_text(payload: &AlertPayload) -> String {
    let mut lines = vec![
        format!(
            "[MaxiLoop] {} in {}",
            payload.event.to_uppercase(),
            payload.project_slug
        ),
        match payload.issue_id {
            Some(id) => format!("Issue #{id}: {}", payload.title),
            None => payload.title.clone(),
        },
        format!(
            "Level: {} | Env: {} | {}: {}",
            payload.level,
            payload.environment,
            if payload.performance.is_some() {
                "Transactions"
            } else {
                "Events"
            },
            payload.event_count
        ),
    ];
    if let Some(rel) = &payload.release {
        lines.push(format!("Release: {rel}"));
    }
    if !payload.culprit.is_empty() {
        lines.push(format!("Culprit: {}", payload.culprit));
    }
    if let Some(details) = &payload.details {
        lines.push(format!("Details: {details}"));
    }
    lines.push(format!("Link: {}", payload.issue_url));
    lines.join("\n")
}

/// Perform an SSRF-safe, redirect-free, timeout-bounded HTTP/1.1 JSON `POST` connecting directly
/// to the validated `SocketAddr`, using strict `rustls` + `webpki-roots` verification for `https://`.
async fn post_json_ssrf_safe(
    raw_url: &str,
    body: Vec<u8>,
    extra_headers: &[(String, String)],
    policy: &DeliveryPolicy,
) -> Result<u16, (Option<i32>, String)> {
    let target = resolve_and_validate_url(raw_url, policy.allow_private_ips)
        .await
        .map_err(|err| (None, err.to_string()))?;

    let host_header = if (target.scheme == "https" && target.port == 443)
        || (target.scheme == "http" && target.port == 80)
    {
        target.host.clone()
    } else if target.host.contains(':') {
        format!("[{}]:{}", target.host, target.port)
    } else {
        format!("{}:{}", target.host, target.port)
    };

    let mut req_builder = Request::builder()
        .method(Method::POST)
        .uri(&target.path_and_query)
        .header(HOST, &host_header)
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, "MaxiLoop-Alerts/0.1");

    for (k, v) in extra_headers {
        req_builder = req_builder.header(k.as_str(), v.as_str());
    }

    let req = req_builder
        .body(Full::new(Bytes::from(body)))
        .map_err(|err| (None, format!("build outbound request: {err}")))?;

    let fut = async {
        let tcp = TcpStream::connect(target.resolved_addr)
            .await
            .map_err(|err| format!("connect failed: {err}"))?;

        if target.scheme == "https" {
            let connector = build_tls_connector()?;
            let server_name = ServerName::try_from(target.host.clone())
                .map_err(|_| "invalid TLS server name".to_owned())?;
            let tls_stream = connector
                .connect(server_name, tcp)
                .await
                .map_err(|err| format!("TLS handshake failed: {err}"))?;
            execute_http1_request(TokioIo::new(tls_stream), req).await
        } else {
            execute_http1_request(TokioIo::new(tcp), req).await
        }
    };

    timeout(policy.timeout, fut)
        .await
        .map_err(|_| (None, "outbound HTTP request timed out".to_owned()))?
        .map_err(|err| (None, err))
}

async fn execute_http1_request<I>(io: I, req: Request<Full<Bytes>>) -> Result<u16, String>
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, conn) = http1::handshake(io)
        .await
        .map_err(|err| format!("HTTP handshake failed: {err}"))?;
    let conn_task = tokio::spawn(async move {
        let _ = conn.await;
    });

    sender
        .ready()
        .await
        .map_err(|err| format!("HTTP sender not ready: {err}"))?;
    let resp = sender
        .send_request(req)
        .await
        .map_err(|err| format!("HTTP send failed: {err}"))?;
    let status = resp.status().as_u16();
    let _ = resp.into_body().collect().await;
    conn_task.abort();
    Ok(status)
}

fn build_tls_connector() -> Result<TlsConnector, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(cfg)))
}

async fn send_smtp_message(
    smtp: &SmtpConfig,
    recipients: &[String],
    subject: &str,
    body: &str,
) -> Result<(), String> {
    let tcp = TcpStream::connect((smtp.host.as_str(), smtp.port))
        .await
        .map_err(|err| format!("connect SMTP: {err}"))?;

    if smtp.use_tls && smtp.port == 465 {
        let connector = build_tls_connector()?;
        let server_name = ServerName::try_from(smtp.host.clone())
            .map_err(|_| "invalid SMTP TLS hostname".to_owned())?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|err| format!("SMTP TLS handshake: {err}"))?;
        let mut reader = BufReader::new(tls);
        expect_smtp_code(&mut reader, 220).await?;
        smtp_cmd(&mut reader, "EHLO maxi-loop.local", 250).await?;
        smtp_deliver_body(&mut reader, smtp, recipients, subject, body).await
    } else {
        let mut reader = BufReader::new(tcp);
        expect_smtp_code(&mut reader, 220).await?;
        smtp_cmd(&mut reader, "EHLO maxi-loop.local", 250).await?;

        if smtp.use_tls {
            smtp_cmd(&mut reader, "STARTTLS", 220).await?;
            let raw_tcp = reader.into_inner();
            let connector = build_tls_connector()?;
            let server_name = ServerName::try_from(smtp.host.clone())
                .map_err(|_| "invalid SMTP TLS hostname".to_owned())?;
            let tls = connector
                .connect(server_name, raw_tcp)
                .await
                .map_err(|err| format!("SMTP STARTTLS handshake: {err}"))?;
            let mut tls_reader = BufReader::new(tls);
            smtp_cmd(&mut tls_reader, "EHLO maxi-loop.local", 250).await?;
            smtp_deliver_body(&mut tls_reader, smtp, recipients, subject, body).await
        } else {
            smtp_deliver_body(&mut reader, smtp, recipients, subject, body).await
        }
    }
}

async fn smtp_deliver_body<S>(
    reader: &mut BufReader<S>,
    smtp: &SmtpConfig,
    recipients: &[String],
    subject: &str,
    body: &str,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let (Some(user), Some(pass)) = (&smtp.username, &smtp.password) {
        let raw_auth = format!("\0{user}\0{pass}");
        let encoded = BASE64_STD.encode(raw_auth.as_bytes());
        smtp_cmd(reader, &format!("AUTH PLAIN {encoded}"), 235).await?;
    }

    let from_clean = sanitize_header_line(&smtp.from_address);
    smtp_cmd(reader, &format!("MAIL FROM:<{from_clean}>"), 250).await?;

    let mut clean_recipients = Vec::with_capacity(recipients.len());
    for rcpt in recipients {
        let rcpt_clean = sanitize_header_line(rcpt);
        smtp_cmd(reader, &format!("RCPT TO:<{rcpt_clean}>"), 250).await?;
        clean_recipients.push(rcpt_clean);
    }

    smtp_cmd(reader, "DATA", 354).await?;

    let to_header = clean_recipients.join(", ");
    let dot_stuffed_body = body
        .replace("\r\n", "\n")
        .lines()
        .map(|line| {
            if line.starts_with('.') {
                format!(".{line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n");

    let message = format!(
        "From: <{from_clean}>\r\n\
         To: {to_header}\r\n\
         Subject: {subject}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {dot_stuffed_body}\r\n\
         .\r\n"
    );

    reader
        .get_mut()
        .write_all(message.as_bytes())
        .await
        .map_err(|err| format!("write SMTP DATA: {err}"))?;
    reader
        .get_mut()
        .flush()
        .await
        .map_err(|err| format!("flush SMTP DATA: {err}"))?;
    expect_smtp_code(reader, 250).await?;

    let _ = smtp_cmd(reader, "QUIT", 221).await;
    Ok(())
}

async fn smtp_cmd<S>(reader: &mut BufReader<S>, cmd: &str, expected: u16) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    reader
        .get_mut()
        .write_all(format!("{cmd}\r\n").as_bytes())
        .await
        .map_err(|err| format!("write SMTP command: {err}"))?;
    reader
        .get_mut()
        .flush()
        .await
        .map_err(|err| format!("flush SMTP command: {err}"))?;
    expect_smtp_code(reader, expected).await
}

async fn expect_smtp_code<S>(reader: &mut BufReader<S>, expected: u16) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|err| format!("read SMTP response: {err}"))?;
        if n == 0 {
            return Err("unexpected EOF from SMTP server".to_owned());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.len() < 3 {
            return Err("malformed SMTP response".to_owned());
        }
        let code: u16 = trimmed[..3]
            .parse()
            .map_err(|_| "invalid SMTP status code".to_owned())?;
        let is_multiline = trimmed.as_bytes().get(3) == Some(&b'-');
        if !is_multiline {
            if code == expected {
                return Ok(());
            }
            return Err(format!("SMTP server returned {code} (expected {expected})"));
        }
    }
}

fn sanitize_header_line(raw: &str) -> String {
    raw.chars()
        .filter(|&c| c != '\r' && c != '\n' && c != '\0')
        .take(256)
        .collect()
}

fn truncate_error(msg: &str) -> String {
    if msg.chars().count() <= 300 {
        msg.to_owned()
    } else {
        msg.chars().take(300).collect()
    }
}

pub fn validate_channel_config(
    channel_type: AlertChannelType,
    config: &Value,
    allow_private_ips: bool,
) -> Result<Value, AppError> {
    let obj = config
        .as_object()
        .ok_or_else(|| AppError::Validation("channel config must be a JSON object".to_owned()))?;

    match channel_type {
        AlertChannelType::Webhook => {
            let url_raw = obj
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::Validation("webhook config requires 'url'".to_owned()))?;
            let validated_url =
                crate::ssrf::validate_webhook_url_static(url_raw, allow_private_ips)?;

            let mut clean = serde_json::Map::new();
            clean.insert("url".to_owned(), Value::String(validated_url.to_string()));
            if let Some(secret) = obj.get("secret").and_then(Value::as_str) {
                let s = secret.trim();
                if !s.is_empty() {
                    if s.len() > 256 {
                        return Err(AppError::Validation(
                            "webhook secret must be at most 256 characters".to_owned(),
                        ));
                    }
                    clean.insert("secret".to_owned(), Value::String(s.to_owned()));
                }
            }
            Ok(Value::Object(clean))
        }
        AlertChannelType::Telegram => {
            let bot_token = obj
                .get("bot_token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    AppError::Validation("telegram config requires 'bot_token'".to_owned())
                })?;
            if bot_token.len() < 10
                || bot_token.len() > 128
                || bot_token.chars().any(char::is_whitespace)
                || !bot_token.contains(':')
            {
                return Err(AppError::Validation(
                    "invalid telegram bot_token format".to_owned(),
                ));
            }
            let chat_id = obj
                .get("chat_id")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.trim().to_owned()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    AppError::Validation("telegram config requires 'chat_id'".to_owned())
                })?;
            if chat_id.len() > 64 || chat_id.chars().any(char::is_whitespace) {
                return Err(AppError::Validation("invalid telegram chat_id".to_owned()));
            }
            Ok(json!({
                "bot_token": bot_token,
                "chat_id": chat_id
            }))
        }
        AlertChannelType::Email => {
            let to_arr = obj.get("to").and_then(Value::as_array).ok_or_else(|| {
                AppError::Validation("email config requires 'to' array of emails".to_owned())
            })?;
            if to_arr.is_empty() || to_arr.len() > 20 {
                return Err(AppError::Validation(
                    "email 'to' must contain between 1 and 20 addresses".to_owned(),
                ));
            }
            let mut clean_to = Vec::with_capacity(to_arr.len());
            for item in to_arr {
                let addr = item
                    .as_str()
                    .map(str::trim)
                    .filter(|s| maxi_loop_platform::config::is_valid_email(s))
                    .ok_or_else(|| {
                        AppError::Validation(
                            "email 'to' contains an invalid email address".to_owned(),
                        )
                    })?;
                clean_to.push(addr.to_ascii_lowercase());
            }
            let mut clean = serde_json::Map::new();
            clean.insert("to".to_owned(), json!(clean_to));
            if let Some(prefix) = obj
                .get("subject_prefix")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if prefix.len() > 40 || prefix.chars().any(|c| c == '\r' || c == '\n') {
                    return Err(AppError::Validation(
                        "email subject_prefix must be <= 40 single-line chars".to_owned(),
                    ));
                }
                clean.insert(
                    "subject_prefix".to_owned(),
                    Value::String(prefix.to_owned()),
                );
            }
            Ok(Value::Object(clean))
        }
    }
}

pub fn redact_channel_config_for_api(channel_type: &str, config: &Value) -> Value {
    match channel_type {
        "webhook" => {
            let url = config.get("url").and_then(Value::as_str).unwrap_or("");
            let has_secret = config
                .get("secret")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty());
            json!({
                "url": url,
                "has_secret": has_secret
            })
        }
        "telegram" => {
            let chat_id = config.get("chat_id").and_then(Value::as_str).unwrap_or("");
            let masked = config
                .get("bot_token")
                .and_then(Value::as_str)
                .map(crate::model::mask_secret)
                .unwrap_or_else(|| "********".to_owned());
            json!({
                "chat_id": chat_id,
                "bot_token_masked": masked
            })
        }
        "email" => {
            let to = config.get("to").cloned().unwrap_or_else(|| json!([]));
            let subject_prefix = config
                .get("subject_prefix")
                .and_then(Value::as_str)
                .unwrap_or("[MaxiLoop]");
            json!({
                "to": to,
                "subject_prefix": subject_prefix
            })
        }
        _ => json!({}),
    }
}
