pub mod audit;
pub mod auth;
pub mod cache;
pub mod error;
pub mod metrics;
pub mod middleware;
pub mod realtime;
pub mod services;
pub mod state;
pub mod users;
pub mod videos;

use axum::{
    Router,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use state::AppState;
use tower_http::cors::{Any, CorsLayer};

async fn serve_docs() -> impl IntoResponse {
    match std::fs::read_to_string("static/docs.html") {
        Ok(content) => Html(content).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Docs not found").into_response(),
    }
}

async fn serve_openapi() -> impl IntoResponse {
    match std::fs::read_to_string("openapi.yaml") {
        Ok(content) => (
            StatusCode::OK,
            [("content-type", "application/yaml")],
            content,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "OpenAPI definition not found").into_response(),
    }
}

pub fn create_router(state: AppState) -> Router {
    // CORS Logic
    let cors = if state.config.cors_allowed_origins.is_empty() {
        if state.config.environment == "development" {
            // Development mode: allow all origins
            tracing::warn!("CORS: Allowing all origins (development mode)");
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        } else {
            // Production mode (or any non-dev): reject all if not explicitly configured
            tracing::error!(
                "CORS: No allowed origins configured in {} mode. Rejecting all cross-origin requests.",
                state.config.environment
            );
            CorsLayer::new()
        }
    } else {
        // Production mode: restrict to specific origins
        let origins: Vec<_> = state
            .config
            .cors_allowed_origins
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<axum::http::HeaderValue>().ok())
            .collect();

        tracing::info!("CORS: Allowing {} specific origin(s)", origins.len());

        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::DELETE,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
            ])
            .allow_credentials(true)
    };

    metrics::register_metrics();

    // Core routes
    let mut router = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/docs", get(serve_docs))
        .route("/openapi.yaml", get(serve_openapi))
        .route("/auth/google/login", get(auth::handlers::google_login))
        .route(
            "/auth/google/callback",
            get(auth::handlers::google_callback),
        )
        .route("/auth/refresh", post(auth::handlers::refresh_token))
        .route("/auth/logout", post(auth::handlers::logout))
        .route(
            "/auth/extension/session",
            post(auth::handlers::create_extension_session),
        )
        .route("/auth/dev/login", post(auth::handlers::dev_login))
        .route("/auth/exchange-otc", post(auth::handlers::exchange_otc));

    // Conditionally expose /metrics only in development/test environments
    // Production metrics should be scraped via internal monitoring infrastructure
    if state.config.environment == "development" || state.config.environment == "test" {
        tracing::info!(
            "Metrics endpoint enabled at /metrics (environment: {})",
            state.config.environment
        );
        router = router.route("/metrics", get(|| async { metrics::metrics_handler() }));
    } else {
        tracing::info!("Metrics endpoint disabled in production mode");
    }

    router
        .merge(videos::router::videos_router(&state.config))
        .nest("/users", users::router::users_router(&state.config))
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::ip_rate_limit::ip_rate_limit,
        ))
        .with_state(state)
}
