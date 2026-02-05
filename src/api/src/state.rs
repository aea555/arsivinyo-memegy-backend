use crate::auth::revocation::TokenRevocationService;
use crate::cache::feed_cache::FeedCacheService;
use crate::cache::otc_cache::OtcCacheService;
use crate::middleware::rate_limit::RateLimiter as OtcRateLimiter;
use crate::services::rate_limiter::RateLimiter;
use sea_orm::DatabaseConnection;
use shared::{config::Config, queue::QueueService, storage::StorageBackend as StorageService};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Arc<Config>,
    pub storage: Arc<dyn StorageService + Send + Sync>,
    pub queue: QueueService,
    pub token_revocation: TokenRevocationService,
    pub feed_cache: FeedCacheService,
    pub otc_cache: OtcCacheService,
    pub rate_limiter: RateLimiter,
    pub otc_rate_limiter: OtcRateLimiter,
}
