use maxi_loop_loadgen::cli::{parse_args_with_env, usage_text};
use maxi_loop_loadgen::runner::{LoadgenReport, run_load_test};

#[tokio::main]
async fn main() {
    let env_dsn = std::env::var("SENTRY_DSN").ok();
    let config = match parse_args_with_env(std::env::args(), env_dsn.as_deref()) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            println!("{}", usage_text());
            return;
        }
        Err(err) => {
            eprintln!("argument error: {err}\n\n{}", usage_text());
            std::process::exit(2);
        }
    };

    let json_output = config.json_output;
    match run_load_test(config).await {
        Ok(report) => {
            if json_output {
                match serde_json::to_string_pretty(&report) {
                    Ok(s) => println!("{s}"),
                    Err(err) => {
                        eprintln!("failed to serialize report: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                print_pretty_report(&report);
            }
        }
        Err(err) => {
            eprintln!("load test failed: {err}");
            std::process::exit(1);
        }
    }
}

fn print_pretty_report(r: &LoadgenReport) {
    println!("=== Maxi Loop Envelope Load Test ===");
    println!("Target:           {}", r.target);
    println!(
        "Config:           concurrency={}, compression={}, unique_issues={}, unsupported_item={}",
        r.concurrency, r.compression, r.unique_issues, r.include_unsupported_item
    );
    println!(
        "Summary:          {} requests in {:.3}s ({:.1} req/s total, {:.1} req/s 2xx)",
        r.total_requests, r.elapsed_secs, r.throughput_rps, r.accepted_rps
    );
    println!(
        "Status codes:     2xx={}, 429={}, 4xx(other)={}, 5xx={}, transport_err={}",
        r.status_2xx, r.status_429, r.status_4xx_other, r.status_5xx, r.transport_errors
    );
    if r.status_429 > 0 {
        println!(
            "Rate limit (429): retry_after_hdr={}, x_sentry_rate_limits_hdr={}, sample={}",
            r.rate_limit_with_retry_after,
            r.rate_limit_with_sentry_header,
            r.sample_sentry_rate_limits.as_deref().unwrap_or("<none>")
        );
    }
    println!(
        "Latency (ms):     min={:.2}, p50={:.2}, p95={:.2}, p99={:.2}, max={:.2}",
        r.latency_ms.min, r.latency_ms.p50, r.latency_ms.p95, r.latency_ms.p99, r.latency_ms.max
    );
    println!("Payload sent:     {} bytes", r.bytes_sent);
}
