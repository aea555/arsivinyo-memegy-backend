mod auth;
mod videos;
mod state;

use axum::{
    routing::{get, post},
    Router,
};
use sea_orm::Database;
use shared::{config::Config, storage::StorageService, queue::QueueService};
use state::AppState;
use std::{net::SocketAddr, sync::Arc};
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
    
    // 4. Services
    let storage = StorageService::new(&config).await;
    let queue = QueueService::new(&config)?;

    // 5. State
    let state = AppState {
        db,
        config: Arc::new(config),
        storage,
        queue,
    };

    // 5. CORS
    let cors = CorsLayer::new()
        .allow_origin(Any) // For now allow all, will restrict later based on config
        .allow_methods(Any)
        .allow_headers(Any);

    // 6. Routes
    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/auth/google/login", get(auth::handlers::google_login))
        .route("/auth/google/callback", get(auth::handlers::google_callback))
        .route("/auth/refresh", post(auth::handlers::refresh_token))
        .route("/auth/logout", post(auth::handlers::logout))
        .route("/videos/init", post(videos::handlers::init_upload))
        .route("/videos/:id/confirm", post(videos::handlers::confirm_upload))
        .route("/videos/:id/like", post(videos::handlers::like_video))
        .route("/feed", get(videos::handlers::get_feed))
        .layer(cors)
        .with_state(state.clone());

    // 7. Server
    let addr = SocketAddr::from(([0, 0, 0, 0], state.config.server_port));
    tracing::info!("Server listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
