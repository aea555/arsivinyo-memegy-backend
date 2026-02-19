use anyhow::Result;
use redis::AsyncCommands;
use shared::queue::QueueService;

/// Generic rate limiter service for user and IP-based limiting.
#[derive(Clone)]
pub struct RateLimiter {
    queue: QueueService,
    bypass: bool,
}

impl RateLimiter {
    pub fn new(queue: QueueService) -> Self {
        Self {
            queue,
            bypass: false,
        }
    }

    pub fn with_bypass(&self, bypass: bool) -> Self {
        Self {
            queue: self.queue.clone(),
            bypass,
        }
    }

    /// Check if an action is within the rate limit.
    /// Returns Ok(remaining) if allowed, Err with current count if exceeded.
    pub async fn check_rate(&self, key: &str, max_count: u64) -> Result<Result<u64, u64>> {
        if self.bypass {
            return Ok(Ok(u64::MAX));
        }

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                let current: Option<u64> = conn.get(key).await?;
                let count = current.unwrap_or(0);

                if count >= max_count {
                    Ok(Err(count))
                } else {
                    Ok(Ok(max_count - count - 1))
                }
            }
            Err(e) => {
                tracing::warn!("Redis unavailable for rate limit check: {:?}", e);
                // Fail open - allow the request if Redis is down
                Ok(Ok(max_count))
            }
        }
    }

    /// Increment the rate limit counter.
    /// Uses Redis INCR with EXPIRE for sliding window.
    pub async fn increment(&self, key: &str, window_secs: usize) -> Result<u64> {
        self.increment_by(key, 1, window_secs).await
    }

    /// Increment the rate limit counter by an arbitrary amount.
    /// Useful for byte-based limits.
    pub async fn increment_by(&self, key: &str, amount: u64, window_secs: usize) -> Result<u64> {
        if self.bypass {
            return Ok(0);
        }

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                // Use INCR + EXPIRE for atomic increment with TTL
                let new_count: u64 = conn.incr(key, amount as i64).await?;

                // Set expiry only when key is first created.
                if new_count == amount {
                    let _: () = conn.expire(key, window_secs as i64).await?;
                }

                Ok(new_count)
            }
            Err(e) => {
                tracing::warn!("Redis unavailable for rate limit increment: {:?}", e);
                Ok(0) // Fail open
            }
        }
    }

    /// Check and increment in one operation.
    /// Returns Ok(remaining) if allowed (and increments), Err(count) if exceeded.
    pub async fn check_and_increment(
        &self,
        key: &str,
        max_count: u64,
        window_secs: usize,
    ) -> Result<Result<u64, u64>> {
        if self.bypass {
            return Ok(Ok(u64::MAX));
        }

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                // First check current count
                let current: Option<u64> = conn.get(key).await?;
                let count = current.unwrap_or(0);

                if count >= max_count {
                    return Ok(Err(count));
                }

                // Increment and set expiry
                let new_count: u64 = conn.incr(key, 1).await?;
                if new_count == 1 {
                    let _: () = conn.expire(key, window_secs as i64).await?;
                }

                Ok(Ok(max_count - new_count))
            }
            Err(e) => {
                tracing::warn!("Redis unavailable for rate limit: {:?}", e);
                Ok(Ok(max_count)) // Fail open
            }
        }
    }

    /// Get the current count for a rate limit key.
    pub async fn get_count(&self, key: &str) -> Result<u64> {
        if self.bypass {
            return Ok(0);
        }

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                let count: Option<u64> = conn.get(key).await?;
                Ok(count.unwrap_or(0))
            }
            Err(_) => Ok(0),
        }
    }

    // Key generators

    /// Generate key for per-user upload rate limiting
    pub fn upload_bytes_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:upload:bytes:{}", user_id)
    }

    /// Generate key for per-user feed rate limiting
    pub fn feed_rpm_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:feed:rpm:{}", user_id)
    }

    /// Generate key for per-user like/unlike rate limiting
    pub fn like_actions_rpm_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:likes:rpm:{}", user_id)
    }

    /// Generate key for per-IP rate limiting
    pub fn ip_rpm_key(ip: &str) -> String {
        format!("ratelimit:ip:rpm:{}", ip)
    }

    /// Generate key for websocket connection attempts per IP
    pub fn ws_connect_ip_key(ip: &str) -> String {
        format!("ratelimit:ws:connect:ip:{}", ip)
    }

    /// Generate key for websocket connection attempts per user
    pub fn ws_connect_user_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:ws:connect:user:{}", user_id)
    }

    /// Generate key for username signup completion attempts per IP
    pub fn username_signup_ip_key(ip: &str) -> String {
        format!("ratelimit:username:signup:ip:{}", ip)
    }

    /// Generate key for username signup completion attempts per signup ticket
    pub fn username_signup_ticket_key(ticket: &str) -> String {
        format!("ratelimit:username:signup:ticket:{}", ticket)
    }

    /// Generate key for username update requests per user
    pub fn username_update_user_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:username:update:user:{}", user_id)
    }

    /// Generate key for keyboard search RPM limit per user
    pub fn keyboard_search_user_rpm_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:keyboard:search:user:{}", user_id)
    }

    /// Generate key for keyboard search RPM limit per extension session
    pub fn keyboard_search_session_rpm_key(session_jti: &uuid::Uuid) -> String {
        format!("ratelimit:keyboard:search:session:{}", session_jti)
    }

    /// Generate key for keyboard send-ticket RPM limit per user
    pub fn keyboard_send_user_rpm_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:keyboard:send:user:{}", user_id)
    }

    /// Generate key for keyboard send-ticket RPM limit per extension session
    pub fn keyboard_send_session_rpm_key(session_jti: &uuid::Uuid) -> String {
        format!("ratelimit:keyboard:send:session:{}", session_jti)
    }

    /// Generate key for keyboard send nonce replay prevention
    pub fn keyboard_nonce_key(user_id: &uuid::Uuid, nonce: &str) -> String {
        format!("keyboard:nonce:{}:{}", user_id, nonce)
    }

    /// Generate key for read-only mode status checks per user
    pub fn read_only_status_user_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:system:read_only_status:user:{}", user_id)
    }

    /// Generate key for maintenance mode status checks per user
    pub fn maintenance_status_user_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:system:maintenance_status:user:{}", user_id)
    }

    /// Generate key for onboarding status checks per user
    pub fn onboarding_status_user_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:onboarding:status:user:{}", user_id)
    }

    /// Generate key for onboarding completion attempts per user
    pub fn onboarding_complete_user_key(user_id: &uuid::Uuid) -> String {
        format!("ratelimit:onboarding:complete:user:{}", user_id)
    }
}
