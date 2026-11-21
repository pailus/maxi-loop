pub mod assets;
pub mod health;

use axum::Router;
use axum::http::header::{self, HeaderValue};
use axum::routing::get;
use maxi_loop_platform::error::AppError;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::AppState;

pub fn build_router(state: AppState) -> Router {
    let api_router = Router::new()
        .route("/health", get(health::check))
        .merge(maxi_loop_auth::http::router::<AppState>())
        .merge(maxi_loop_org::http::router::<AppState>())
        .merge(maxi_loop_issues::http::router::<AppState>())
        .merge(maxi_loop_alerts::http::router::<AppState>())
        .merge(maxi_loop_performance::http::router::<AppState>())
        .fallback(api_not_found);

    Router::new()
        .route("/healthz", get(health::check))
        .merge(maxi_loop_ingest::http::router::<AppState>())
        .merge(maxi_loop_mcp::http::router::<AppState>())
        .nest("/api/v1", api_router)
        .fallback(assets::serve_spa)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'",
            ),
        ))
}

async fn api_not_found() -> AppError {
    AppError::NotFound
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use maxi_loop_platform::config::{Config, LogFormat};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    fn test_state_unreachable_db() -> AppState {
        let config = Config {
            app_host: "127.0.0.1".parse().expect("valid ip"),
            app_port: 8080,
            public_base_url: "http://127.0.0.1:8080".to_owned(),
            database_url: "postgres://user:secret_test_pw@127.0.0.1:1/maxi_loop".to_owned(),
            db_max_connections: 1,
            retention_default_days: 30,
            max_full_events_per_issue: 50,
            trace_slow_threshold_ms: 500,
            trace_random_sample_rate: 0.05,
            max_envelope_compressed_bytes: 1_048_576,
            max_envelope_decompressed_bytes: 2_097_152,
            max_event_payload_bytes: 524_288,
            ingest_channel_capacity: 1024,
            log_format: LogFormat::Pretty,
            single_tenant_mode: true,
            cookie_secure: false,
            session_ttl_hours: 168,
            default_tenant_slug: "default".to_owned(),
            default_tenant_name: "Default Organization".to_owned(),
            default_workspace_slug: "default".to_owned(),
            default_workspace_name: "Default Workspace".to_owned(),
            bootstrap_admin_email: None,
            bootstrap_admin_password: None,
            bootstrap_admin_name: "Administrator".to_owned(),
            smtp: None,
        };
        let db = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .expect("lazy pool creation should not fail");
        AppState::new(config, db)
    }

    #[tokio::test]
    async fn healthz_returns_503_without_leaking_db_details() {
        let app = build_router(test_state_unreachable_db());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
            Some(&HeaderValue::from_static("nosniff"))
        );
        assert_eq!(
            response.headers().get(header::X_FRAME_OPTIONS),
            Some(&HeaderValue::from_static("DENY"))
        );

        let body_bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body bytes");
        let body_str = String::from_utf8_lossy(&body_bytes);
        assert!(body_str.contains("\"status\":\"degraded\""));
        assert!(!body_str.contains("secret_test_pw"));
        assert!(!body_str.contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn unknown_api_route_returns_json_404() {
        let app = build_router(test_state_unreachable_db());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/nonexistent")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body_bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body bytes");
        assert_eq!(body_bytes, r#"{"error":"not found"}"#);
    }

    #[tokio::test]
    async fn path_traversal_attempt_is_rejected() {
        let app = build_router(test_state_unreachable_db());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/..%2F..%2Fetc/passwd")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn root_serves_embedded_spa_when_built() {
        let app = build_router(test_state_unreachable_db());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.contains("text/html"));
    }
}
