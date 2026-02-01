pub mod auth;
pub mod cache;
pub mod error;
pub mod middleware;
pub mod services;
pub mod state;
pub mod users;
pub mod videos;

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
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
        // Development mode: allow all origins
        tracing::warn!("CORS: Allowing all origins (development mode)");
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
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

    Router::new()
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
        .route("/auth/dev/login", post(auth::handlers::dev_login))
        .merge(videos::router::videos_router(&state.config))
        .nest("/users", users::router::users_router(&state.config))
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::ip_rate_limit::ip_rate_limit,
        ))
        .with_state(state)
}
