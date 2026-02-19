use anyhow::Result;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use shared::queue::QueueService;

/// Uploader information for cached feed items
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CachedUploaderInfo {
    pub id: uuid::Uuid,
    pub username: String,
}

/// Cached video feed item (lightweight for Redis storage)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CachedVideoFeedItem {
    pub id: uuid::Uuid,
    pub title: Option<String>,
    pub url: String,
    pub thumbnail_url: Option<String>,
    pub like_count: i64,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    // Phase 5: New fields for soft-delete and anonymity support
    #[serde(default)]
    pub deleted_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    #[serde(default)]
    pub is_anonymous: bool,
    #[serde(default)]
    pub is_nsfw: Option<bool>,
    #[serde(default)]
    pub uploader: Option<CachedUploaderInfo>, // None if anonymous
}

/// Service for caching feed metadata in Redis.
#[derive(Clone)]
pub struct FeedCacheService {
    queue: QueueService,
}

impl FeedCacheService {
    pub fn new(queue: QueueService) -> Self {
        Self { queue }
    }

    /// Generate cache key for feed query
    fn cache_key(sort: &str, tag: Option<&str>, page: u64) -> String {
        match tag {
            Some(t) => format!("feed:v3:{}:tag:{}:page:{}", sort, t, page),
            None => format!("feed:v3:{}:page:{}", sort, page),
        }
    }

    /// Get feed from cache
    pub async fn get_feed(
        &self,
        sort: &str,
        tag: Option<&str>,
        page: u64,
    ) -> Result<Option<Vec<CachedVideoFeedItem>>> {
        let key = Self::cache_key(sort, tag, page);

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                let cached: Option<String> = conn.get(&key).await?;
                match cached {
                    Some(json) => {
                        let items: Vec<CachedVideoFeedItem> = serde_json::from_str(&json)?;

                        // Phase 5: Client-side filtering for soft-deleted videos
                        let filtered: Vec<CachedVideoFeedItem> = items
                            .into_iter()
                            .filter(|item| item.deleted_at.is_none())
                            .collect();

                        Ok(Some(filtered))
                    }
                    None => Ok(None),
                }
            }
            Err(e) => {
                tracing::warn!("Redis unavailable for cache read: {:?}", e);
                Ok(None) // Cache miss, fallback to DB
            }
        }
    }

    /// Set feed in cache
    pub async fn set_feed(
        &self,
        sort: &str,
        tag: Option<&str>,
        page: u64,
        items: &[CachedVideoFeedItem],
        ttl_secs: usize,
    ) -> Result<()> {
        let key = Self::cache_key(sort, tag, page);
        let json = serde_json::to_string(items)?;

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                let _: () = redis::cmd("SET")
                    .arg(&key)
                    .arg(&json)
                    .arg("EX")
                    .arg(ttl_secs)
                    .query_async(&mut conn)
                    .await?;
                Ok(())
            }
            Err(e) => {
                tracing::warn!("Redis unavailable for cache write: {:?}", e);
                Ok(()) // Non-fatal, skip caching
            }
        }
    }

    /// Invalidate all feed caches (call when video published/deleted)
    #[allow(dead_code)]
    pub async fn invalidate_all(&self) -> Result<()> {
        match self.queue.get_conn().await {
            Ok(mut conn) => {
                // Use SCAN to safely delete keys matching pattern
                let script = redis::Script::new(
                    r#"
                    local keys = redis.call('keys', 'feed:*')
                    for i=1,#keys,5000 do
                        redis.call('del', unpack(keys, i, math.min(i+4999, #keys)))
                    end
                    return #keys
                "#,
                );

                let deleted: i64 = script.invoke_async(&mut conn).await?;
                tracing::debug!("Invalidated {} feed cache entries", deleted);
                Ok(())
            }
            Err(e) => {
                tracing::warn!("Redis unavailable for cache invalidation: {:?}", e);
                Ok(()) // Non-fatal
            }
        }
    }
}
