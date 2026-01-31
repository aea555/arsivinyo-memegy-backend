use crate::auth::revocation::TokenRevocationService;
use crate::cache::feed_cache::FeedCacheService;
use crate::services::rate_limiter::RateLimiter;
use sea_orm::DatabaseConnection;
use shared::{config::Config, queue::QueueService, storage::StorageService};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Arc<Config>,
    pub storage: StorageService,
    pub queue: QueueService,
    pub token_revocation: TokenRevocationService,
    pub feed_cache: FeedCacheService,
    pub rate_limiter: RateLimiter,
}
