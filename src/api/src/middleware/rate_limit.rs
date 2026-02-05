use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct RateLimiter {
    // IP -> (attempt_count, window_start)
    attempts: Arc<RwLock<HashMap<String, (u32, Instant)>>>,
    max_attempts: u32,
    window_duration: Duration,
}

impl RateLimiter {
    pub fn new(max_attempts: u32, window_seconds: u64) -> Self {
        Self {
            attempts: Arc::new(RwLock::new(HashMap::new())),
            max_attempts,
            window_duration: Duration::from_secs(window_seconds),
        }
    }

    pub async fn check_rate_limit(&self, key: &str) -> Result<(), &'static str> {
        let mut attempts = self.attempts.write().await;
        let now = Instant::now();

        let entry = attempts.entry(key.to_string()).or_insert((0, now));

        // Reset if window expired
        if now.duration_since(entry.1) > self.window_duration {
            entry.0 = 0;
            entry.1 = now;
        }

        // Check limit
        if entry.0 >= self.max_attempts {
            return Err("Rate limit exceeded");
        }

        entry.0 += 1;
        Ok(())
    }

    // Cleanup old entries periodically
    pub async fn cleanup(&self) {
        let mut attempts = self.attempts.write().await;
        let now = Instant::now();
        attempts.retain(|_, (_, time)| now.duration_since(*time) < self.window_duration * 2);
    }
}
