use api::{
    auth::revocation::TokenRevocationService,
    cache::feed_cache::FeedCacheService,
    cache::otc_cache::OtcCacheService,
    create_router, metrics,
    realtime::{hub::RealtimeHub, subscriber::spawn_realtime_subscriber},
    services::rate_limiter::RateLimiter,
    services::{ban_service::BanService, security_event_service::SecurityEventService},
    state::AppState,
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

    // Register metrics
    metrics::register_metrics();

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
    let otc_cache = OtcCacheService::new(queue.clone());
    let rate_limiter = RateLimiter::new(queue.clone());
    let ban_service = BanService::new(db.clone(), queue.clone(), Arc::new(config.clone()));
    let security_event_service =
        SecurityEventService::new(db.clone(), queue.clone(), Arc::new(config.clone()));

    // 6. OTC Rate Limiter
    let otc_rate_limiter = api::middleware::rate_limit::RateLimiter::new(
        config.otc_rate_limit_max_attempts,
        config.otc_rate_limit_window_seconds,
    );
    let realtime_hub = RealtimeHub::new(
        config.video_ws_max_conn_per_user,
        config.video_ws_max_conn_global,
        config.video_ws_send_buffer,
    );

    // 7. State
    let state = AppState {
        db,
        config: Arc::new(config.clone()),
        storage,
        queue,
        token_revocation,
        feed_cache,
        otc_cache,
        rate_limiter,
        otc_rate_limiter,
        realtime_hub,
        ban_service,
        security_event_service,
    };

    // 8. Spawn rate limiter cleanup task
    let cleanup_limiter = state.otc_rate_limiter.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // 5 minutes
        loop {
            interval.tick().await;
            cleanup_limiter.cleanup().await;
        }
    });

    // 9. Spawn realtime pubsub listener
    spawn_realtime_subscriber(state.clone());

    // 10. Spawn periodic purge for security events retention policy
    let security_event_service = state.security_event_service.clone();
    let purge_interval_secs = state.config.security_events_purge_interval_secs.max(60);
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(purge_interval_secs));
        loop {
            interval.tick().await;
            if let Err(err) = security_event_service.purge_expired().await {
                tracing::warn!("Failed to purge expired security events: {:?}", err);
            }
        }
    });

    // 11. Routes (via library)
    let app = create_router(state.clone());

    // 12. Server
    let addr = SocketAddr::from(([0, 0, 0, 0], state.config.server_port));
    tracing::info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
