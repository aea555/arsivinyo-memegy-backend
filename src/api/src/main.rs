use api::{
    auth::revocation::TokenRevocationService, cache::feed_cache::FeedCacheService, create_router,
    services::rate_limiter::RateLimiter, state::AppState,
};
use sea_orm::Database;
use sea_orm_migration::prelude::*;
use shared::{config::Config, queue::QueueService, storage::S3Storage};
use std::{net::SocketAddr, sync::Arc};
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

    // 4. Run Migrations
    tracing::info!("Running database migrations...");
    migration::Migrator::up(&db, None).await?;
    tracing::info!("Database migrations completed");

    // 5. Services
    let storage = Arc::new(S3Storage::new(&config).await);
    let queue = QueueService::new(&config)?;
    let token_revocation = TokenRevocationService::new(queue.clone());
    let feed_cache = FeedCacheService::new(queue.clone());
    let rate_limiter = RateLimiter::new(queue.clone());

    // 6. State
    let state = AppState {
        db,
        config: Arc::new(config.clone()),
        storage,
        queue,
        token_revocation,
        feed_cache,
        rate_limiter,
    };

    // 7. Routes (via library)
    let app = create_router(state.clone());

    // 8. Server
    let addr = SocketAddr::from(([0, 0, 0, 0], state.config.server_port));
    tracing::info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
