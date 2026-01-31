mod auth;
mod cache;
mod error;
mod middleware;
mod services;
mod state;
mod videos;

use auth::revocation::TokenRevocationService;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use cache::feed_cache::FeedCacheService;
use sea_orm::Database;
use sea_orm_migration::prelude::*;
use services::rate_limiter::RateLimiter;
use shared::{config::Config, queue::QueueService, storage::StorageService};
use state::AppState;
use std::{net::SocketAddr, sync::Arc};
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

async fn serve_docs() -> impl IntoResponse {
    Html(include_str!("../../../static/docs.html"))
}

async fn serve_openapi() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/yaml")],
        include_str!("../../../openapi.yaml"),
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "debug".to_string()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 2. Config
    let config = Config::from_env()?;

    // 3. Database
    let db = Database::connect(&config.database_url).await?;
    tracing::info!("Connected to database");

    // 4. Run Migrations
    tracing::info!("Running database migrations...");
    migration::Migrator::up(&db, None).await?;
    tracing::info!("Database migrations completed");

    // 5. Services
    let storage = StorageService::new(&config).await;
    let queue = QueueService::new(&config)?;
    let token_revocation = TokenRevocationService::new(queue.clone());
    let feed_cache = FeedCacheService::new(queue.clone());
    let rate_limiter = RateLimiter::new(queue.clone());

    // 6. State
    let state = AppState {
        db,
        config: Arc::new(config),
        storage,
        queue,
        token_revocation,
        feed_cache,
        rate_limiter,
    };

    // 7. CORS
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

    // 8. Routes
    let app = Router::new()
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
        .route("/videos/init", post(videos::handlers::init_upload))
        .route(
            "/videos/init/anonymous",
            post(videos::handlers::init_anonymous_upload),
        )
        .route(
            "/videos/:id/confirm",
            post(videos::handlers::confirm_upload),
        )
        .route("/videos/:id/like", post(videos::handlers::like_video))
        .route("/feed", get(videos::handlers::get_feed))
        .layer(cors)
        .with_state(state.clone());

    // 9. Server
    let addr = SocketAddr::from(([0, 0, 0, 0], state.config.server_port));
    tracing::info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
