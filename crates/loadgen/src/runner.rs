use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use http::header::{CONTENT_ENCODING, CONTENT_TYPE, HOST, RETRY_AFTER};
use http::{Method, Request, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::cli::{LoadMode, LoadgenConfig};
use crate::generator::build_synthetic_envelope;

const X_SENTRY_RATE_LIMITS: &str = "x-sentry-rate-limits";

#[derive(Clone, Debug, Serialize)]
pub struct LatencyStatsMs {
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LoadgenReport {
    pub target: String,
    pub compression: String,
    pub concurrency: usize,
    pub unique_issues: usize,
    pub include_unsupported_item: bool,
    pub elapsed_secs: f64,
    pub total_requests: u64,
    pub status_2xx: u64,
    pub status_429: u64,
    pub status_4xx_other: u64,
    pub status_5xx: u64,
    pub transport_errors: u64,
    pub rate_limit_with_retry_after: u64,
    pub rate_limit_with_sentry_header: u64,
    pub sample_sentry_rate_limits: Option<String>,
    pub throughput_rps: f64,
    pub accepted_rps: f64,
    pub bytes_sent: u64,
    pub latency_ms: LatencyStatsMs,
}

#[derive(Default)]
struct WorkerStats {
    status_2xx: u64,
    status_429: u64,
    status_4xx_other: u64,
    status_5xx: u64,
    transport_errors: u64,
    rate_limit_with_retry_after: u64,
    rate_limit_with_sentry_header: u64,
    sample_sentry_rate_limits: Option<String>,
    bytes_sent: u64,
    latencies_us: Vec<u64>,
}

struct KeepAliveClient {
    sender: http1::SendRequest<Full<Bytes>>,
    conn_task: JoinHandle<()>,
}

impl KeepAliveClient {
    async fn connect(addr: &str, req_timeout: Duration) -> Result<Self, String> {
        let stream = timeout(req_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| "connect timeout".to_owned())?
            .map_err(|err| format!("connect error: {err}"))?;
        let _ = stream.set_nodelay(true);

        let io = TokioIo::new(stream);
        let (sender, conn) = timeout(req_timeout, http1::handshake(io))
            .await
            .map_err(|_| "http1 handshake timeout".to_owned())?
            .map_err(|err| format!("http1 handshake error: {err}"))?;

        let conn_task = tokio::spawn(async move {
            let _ = conn.await;
        });

        Ok(Self { sender, conn_task })
    }

    fn is_closed(&self) -> bool {
        self.sender.is_closed() || self.conn_task.is_finished()
    }
}

impl Drop for KeepAliveClient {
    fn drop(&mut self) {
        self.conn_task.abort();
    }
}

pub async fn run_load_test(config: LoadgenConfig) -> Result<LoadgenReport, String> {
    let config = Arc::new(config);
    let counter = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let deadline = match config.mode {
        LoadMode::TotalRequests(_) => None,
        LoadMode::Duration(dur) => Some(start + dur),
    };

    let mut handles = Vec::with_capacity(config.concurrency);
    for _ in 0..config.concurrency {
        let cfg = Arc::clone(&config);
        let seq_counter = Arc::clone(&counter);
        handles.push(tokio::spawn(async move {
            run_worker(cfg, seq_counter, deadline).await
        }));
    }

    let mut merged = WorkerStats::default();
    for handle in handles {
        let stats = handle
            .await
            .map_err(|err| format!("worker task panicked: {err}"))?;
        merged.status_2xx += stats.status_2xx;
        merged.status_429 += stats.status_429;
        merged.status_4xx_other += stats.status_4xx_other;
        merged.status_5xx += stats.status_5xx;
        merged.transport_errors += stats.transport_errors;
        merged.rate_limit_with_retry_after += stats.rate_limit_with_retry_after;
        merged.rate_limit_with_sentry_header += stats.rate_limit_with_sentry_header;
        if merged.sample_sentry_rate_limits.is_none() {
            merged.sample_sentry_rate_limits = stats.sample_sentry_rate_limits;
        }
        merged.bytes_sent += stats.bytes_sent;
        merged.latencies_us.extend(stats.latencies_us);
    }

    let elapsed_secs = start.elapsed().as_secs_f64().max(1e-6);
    let total_requests = merged.status_2xx
        + merged.status_429
        + merged.status_4xx_other
        + merged.status_5xx
        + merged.transport_errors;

    let latency_ms = compute_latency_stats(&mut merged.latencies_us);

    Ok(LoadgenReport {
        target: config.dsn.masked_target(),
        compression: config.compression.as_str().to_owned(),
        concurrency: config.concurrency,
        unique_issues: config.unique_issues,
        include_unsupported_item: config.include_unsupported_item,
        elapsed_secs,
        total_requests,
        status_2xx: merged.status_2xx,
        status_429: merged.status_429,
        status_4xx_other: merged.status_4xx_other,
        status_5xx: merged.status_5xx,
        transport_errors: merged.transport_errors,
        rate_limit_with_retry_after: merged.rate_limit_with_retry_after,
        rate_limit_with_sentry_header: merged.rate_limit_with_sentry_header,
        sample_sentry_rate_limits: merged.sample_sentry_rate_limits,
        throughput_rps: (total_requests as f64) / elapsed_secs,
        accepted_rps: (merged.status_2xx as f64) / elapsed_secs,
        bytes_sent: merged.bytes_sent,
        latency_ms,
    })
}

async fn run_worker(
    config: Arc<LoadgenConfig>,
    seq_counter: Arc<AtomicU64>,
    deadline: Option<Instant>,
) -> WorkerStats {
    let mut stats = WorkerStats::default();
    let connect_addr = config.dsn.connect_addr();
    let host_header = config.dsn.host_header();
    let envelope_path = config.dsn.envelope_path();
    let sentry_auth = config.dsn.sentry_auth_header();

    let mut client: Option<KeepAliveClient> = None;

    loop {
        if let Some(end_at) = deadline
            && Instant::now() >= end_at
        {
            break;
        }

        let seq = seq_counter.fetch_add(1, Ordering::Relaxed);
        if let LoadMode::TotalRequests(limit) = config.mode
            && seq >= limit
        {
            break;
        }

        let envelope = match build_synthetic_envelope(
            seq,
            config.unique_issues,
            &config.environment,
            config.release.as_deref(),
            config.include_unsupported_item,
            config.compression,
        ) {
            Ok(env) => env,
            Err(_) => {
                stats.transport_errors += 1;
                continue;
            }
        };

        let body_len = u64::try_from(envelope.body.len()).unwrap_or(0);
        let req_start = Instant::now();

        if client.as_ref().is_none_or(KeepAliveClient::is_closed) {
            match KeepAliveClient::connect(&connect_addr, config.request_timeout).await {
                Ok(c) => client = Some(c),
                Err(_) => {
                    stats.transport_errors += 1;
                    continue;
                }
            }
        }

        let mut req_builder = Request::builder()
            .method(Method::POST)
            .uri(&envelope_path)
            .header(HOST, &host_header)
            .header(CONTENT_TYPE, "application/x-sentry-envelope")
            .header("x-sentry-auth", &sentry_auth);

        if let Some(enc) = envelope.content_encoding {
            req_builder = req_builder.header(CONTENT_ENCODING, enc);
        }

        let req = match req_builder.body(Full::new(Bytes::from(envelope.body))) {
            Ok(r) => r,
            Err(_) => {
                stats.transport_errors += 1;
                continue;
            }
        };

        let Some(conn) = client.as_mut() else {
            stats.transport_errors += 1;
            continue;
        };

        let response_res = timeout(config.request_timeout, async {
            conn.sender.ready().await?;
            let resp = conn.sender.send_request(req).await?;
            let status = resp.status();
            let headers = resp.headers().clone();
            let _ = resp.into_body().collect().await?;
            Ok::<_, hyper::Error>((status, headers))
        })
        .await;

        let elapsed_us = u64::try_from(req_start.elapsed().as_micros()).unwrap_or(u64::MAX);

        match response_res {
            Ok(Ok((status, headers))) => {
                stats.bytes_sent += body_len;
                stats.latencies_us.push(elapsed_us);
                if status.is_success() {
                    stats.status_2xx += 1;
                } else if status == StatusCode::TOO_MANY_REQUESTS {
                    stats.status_429 += 1;
                    if headers.contains_key(RETRY_AFTER) {
                        stats.rate_limit_with_retry_after += 1;
                    }
                    if let Some(val) = headers
                        .get(X_SENTRY_RATE_LIMITS)
                        .and_then(|v| v.to_str().ok())
                    {
                        stats.rate_limit_with_sentry_header += 1;
                        if stats.sample_sentry_rate_limits.is_none() {
                            stats.sample_sentry_rate_limits = Some(val.to_owned());
                        }
                    }
                } else if status.is_client_error() {
                    stats.status_4xx_other += 1;
                } else {
                    stats.status_5xx += 1;
                }
            }
            Ok(Err(_)) | Err(_) => {
                stats.transport_errors += 1;
                client = None;
            }
        }
    }

    stats
}

pub fn compute_latency_stats(samples_us: &mut [u64]) -> LatencyStatsMs {
    if samples_us.is_empty() {
        return LatencyStatsMs {
            min: 0.0,
            p50: 0.0,
            p95: 0.0,
            p99: 0.0,
            max: 0.0,
        };
    }

    samples_us.sort_unstable();
    let to_ms = |us: u64| (us as f64) / 1000.0;

    LatencyStatsMs {
        min: to_ms(samples_us[0]),
        p50: to_ms(percentile_sorted(samples_us, 0.50)),
        p95: to_ms(percentile_sorted(samples_us, 0.95)),
        p99: to_ms(percentile_sorted(samples_us, 0.99)),
        max: to_ms(samples_us[samples_us.len() - 1]),
    }
}

fn percentile_sorted(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (((sorted.len() - 1) as f64) * pct).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use tokio::net::TcpListener;

    use super::*;
    use crate::cli::{CompressionKind, ParsedDsn};

    #[test]
    fn computes_latency_percentiles_accurately() {
        let mut samples: Vec<u64> = (1..=100).map(|v| v * 1000).collect();
        let stats = compute_latency_stats(&mut samples);
        assert!((stats.min - 1.0).abs() < 1e-6);
        assert!((stats.p50 - 50.0).abs() < 1.5);
        assert!((stats.p95 - 95.0).abs() < 1.5);
        assert!((stats.p99 - 99.0).abs() < 1.5);
        assert!((stats.max - 100.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn benchmarks_200_and_429_rate_limit_headers_against_live_http_server() {
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/api/{project_id}/envelope/",
                post(
                    |State(cnt): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
                        let auth_ok = headers
                            .get("x-sentry-auth")
                            .and_then(|v| v.to_str().ok())
                            .is_some_and(|v| v.contains("sentry_key=testkey123"));
                        if !auth_ok {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        let n = cnt.fetch_add(1, Ordering::Relaxed);
                        if n < 10 {
                            (StatusCode::OK, "{\"accepted_items\":1,\"dropped_items\":0}")
                                .into_response()
                        } else {
                            (
                                StatusCode::TOO_MANY_REQUESTS,
                                [
                                    ("retry-after", "3"),
                                    ("x-sentry-rate-limits", "3:error:project"),
                                ],
                                "{\"error\":\"rate limit exceeded\"}",
                            )
                                .into_response()
                        }
                    },
                ),
            )
            .with_state(counter);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let cfg = LoadgenConfig {
            dsn: ParsedDsn::parse(&format!("http://testkey123@127.0.0.1:{}/1", addr.port()))
                .expect("valid dsn"),
            mode: LoadMode::TotalRequests(25),
            concurrency: 4,
            unique_issues: 5,
            compression: CompressionKind::Gzip,
            environment: "test".to_owned(),
            release: Some("rev-test".to_owned()),
            request_timeout: Duration::from_secs(2),
            include_unsupported_item: true,
            json_output: false,
        };

        let report = run_load_test(cfg).await.expect("load test succeeds");
        server_task.abort();

        assert_eq!(report.total_requests, 25);
        assert_eq!(report.status_2xx, 10);
        assert_eq!(report.status_429, 15);
        assert_eq!(report.status_4xx_other, 0);
        assert_eq!(report.status_5xx, 0);
        assert_eq!(report.transport_errors, 0);
        assert_eq!(report.rate_limit_with_retry_after, 15);
        assert_eq!(report.rate_limit_with_sentry_header, 15);
        assert_eq!(
            report.sample_sentry_rate_limits.as_deref(),
            Some("3:error:project")
        );
        assert!(report.throughput_rps > 0.0);
    }
}
