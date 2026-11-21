use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::thread;
use std::time::Duration;

use sentry::protocol::{Request, SpanStatus};
use tracing_subscriber::prelude::*;

#[derive(Debug)]
struct BillingChargeError {
    invoice_id: u64,
    reason: &'static str,
}

impl fmt::Display for BillingChargeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "billing charge failed for invoice {}: {}",
            self.invoice_id, self.reason
        )
    }
}

impl Error for BillingChargeError {}

#[inline(never)]
fn charge_invoice(invoice_id: u64) -> Result<(), BillingChargeError> {
    call_payment_gateway(invoice_id)
}

#[inline(never)]
fn call_payment_gateway(invoice_id: u64) -> Result<(), BillingChargeError> {
    Err(BillingChargeError {
        invoice_id,
        reason: "gateway timeout",
    })
}

fn build_sensitive_request() -> Request {
    let bearer = env::var("E2E_BEARER_TOKEN").unwrap_or_else(|_| "ephemeral-bearer".to_owned());
    let cookie = env::var("E2E_COOKIE_VALUE").unwrap_or_else(|_| "ephemeral-cookie".to_owned());

    let mut headers = BTreeMap::new();
    headers.insert("Authorization".to_owned(), format!("Bearer {bearer}"));
    headers.insert("Cookie".to_owned(), format!("session_id={cookie}"));

    Request {
        url: Some("https://billing.internal/api/rust/charge?token=secret-url-tok".parse().unwrap()),
        method: Some("POST".to_owned()),
        headers,
        ..Default::default()
    }
}

fn run_error_and_trace() {
    let dsn = env::var("SENTRY_DSN").expect("SENTRY_DSN must be set");

    let guard = sentry::init((
        dsn,
        sentry::ClientOptions {
            release: Some("e2e-rust-sha-d4e5f6a".into()),
            environment: Some("e2e-rust".into()),
            attach_stacktrace: true,
            traces_sample_rate: 1.0,
            send_default_pii: true,
            ..Default::default()
        },
    ));

    let subscriber =
        tracing_subscriber::registry().with(sentry::integrations::tracing::layer());
    let _sub_guard = tracing::subscriber::set_default(subscriber);

    let sensitive_req = build_sensitive_request();

    // 1. Capture the same Rust error 3 times with different invoice IDs -> should group into 1 issue.
    for invoice_id in [2001_u64, 2002, 2003] {
        let req_clone = sensitive_req.clone();
        sentry::with_scope(
            |scope| {
                scope.set_tag("sdk", "sentry-rust");
                scope.set_tag("invoice_id", invoice_id.to_string());
                scope.add_event_processor(move |mut event| {
                    event.request = Some(req_clone.clone());
                    Some(event)
                });
            },
            || {
                if let Err(err) = charge_invoice(invoice_id) {
                    sentry::capture_error(&err);
                }
            },
        );
    }

    // 2. Capture a `tracing::error!` event through `sentry-tracing` -> 2nd grouped issue.
    let req_clone = sensitive_req.clone();
    sentry::with_scope(
        |scope| {
            scope.set_tag("sdk", "sentry-rust-tracing");
            scope.add_event_processor(move |mut event| {
                event.request = Some(req_clone.clone());
                Some(event)
            });
        },
        || {
            tracing::error!(ledger_id = 42, "ledger reconciliation checksum mismatch");
        },
    );

    // 3. Send a real Rust performance transaction with 2 child spans (error status -> 100% sampled).
    let tx_ctx = sentry::TransactionContext::new("POST /api/rust/billing", "http.server");
    let tx = sentry::start_transaction(tx_ctx);
    tx.set_data("sdk", "sentry-rust".into());
    let trace_id = tx.get_trace_context().trace_id.to_string();

    {
        let db_span = tx.start_child("db.sql.execute", "INSERT INTO invoices (id, amount) VALUES ($1, $2)");
        thread::sleep(Duration::from_millis(25));
        db_span.set_status(SpanStatus::Ok);
        db_span.finish();
    }

    {
        let psp_span = tx.start_child("http.client", "POST https://psp.internal/charge");
        thread::sleep(Duration::from_millis(15));
        psp_span.set_status(SpanStatus::InternalError);
        psp_span.finish();
    }

    tx.set_status(SpanStatus::InternalError);
    tx.finish();

    drop(_sub_guard);
    guard.close(Some(Duration::from_secs(10)));

    println!("TRACE_ID={trace_id}");
}

fn run_invalid_dsn() {
    let dsn = env::var("SENTRY_DSN").expect("SENTRY_DSN must be set");
    let guard = sentry::init((
        dsn,
        sentry::ClientOptions {
            environment: Some("e2e-rust-negative".into()),
            ..Default::default()
        },
    ));
    sentry::capture_message("should be rejected by invalid dsn", sentry::Level::Error);
    guard.flush(Some(Duration::from_secs(5)));
}

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "error-and-trace".to_owned());
    match mode.as_str() {
        "error-and-trace" => run_error_and_trace(),
        "invalid-dsn" => run_invalid_dsn(),
        other => {
            eprintln!("unknown mode: {other}");
            std::process::exit(2);
        }
    }
}
