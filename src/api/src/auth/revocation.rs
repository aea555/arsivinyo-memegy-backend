use anyhow::Result;
use redis::AsyncCommands;
use shared::queue::QueueService;
use uuid::Uuid;

/// Service for managing revoked JWT tokens.
/// Uses Redis to maintain a blacklist of revoked token JTIs with TTL.
#[derive(Clone)]
pub struct TokenRevocationService {
    queue: QueueService,
}

impl TokenRevocationService {
    pub fn new(queue: QueueService) -> Self {
        Self { queue }
    }

    /// Revoke a token by its JTI (JWT ID).
    /// The revocation entry expires after `ttl_secs` (matching token expiry).
    pub async fn revoke_token(&self, jti: Uuid, ttl_secs: usize) -> Result<()> {
        let key = format!("revoked:token:{}", jti);
        let mut conn = self.queue.get_conn().await?;

        let _: () = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("EX")
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await?;

        Ok(())
    }

    /// Check if a token has been revoked.
    pub async fn is_revoked(&self, jti: Uuid) -> bool {
        let key = format!("revoked:token:{}", jti);

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                let exists: Result<bool, _> = conn.exists(&key).await;
                exists.unwrap_or(false)
            }
            Err(_) => {
                // Fail open - if Redis is down, allow the token
                // (short-lived tokens expire quickly anyway)
                tracing::warn!("Redis unavailable for revocation check, allowing token");
                false
            }
        }
    }
}
