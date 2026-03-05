use anyhow::Result;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use shared::queue::QueueService;

/// Token data stored temporarily for OTC exchange
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OtcAuthTokenData {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: uuid::Uuid,
    pub username: String,
    pub email: String,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub age_confirmed: bool,
    #[serde(default)]
    pub terms_accepted: bool,
    #[serde(default)]
    pub required_terms_version: String,
    #[serde(default)]
    pub accepted_terms_version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OtcSignupRequiredData {
    pub signup_ticket: String,
    pub suggested_username: String,
    pub required_terms_version: String,
    pub terms_url: Option<String>,
    pub requires_age_confirmation: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OtcTokenData {
    AuthSuccess(OtcAuthTokenData),
    SignupRequired(OtcSignupRequiredData),
}

/// Service for storing and retrieving one-time codes
#[derive(Clone)]
pub struct OtcCacheService {
    queue: QueueService,
}

impl OtcCacheService {
    pub fn new(queue: QueueService) -> Self {
        Self { queue }
    }

    /// Store token data with OTC (60s TTL)
    pub async fn store_otc(&self, code: &str, tokens: &OtcTokenData) -> Result<()> {
        let key = format!("otc:{}", code);
        let json = serde_json::to_string(tokens)?;

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                let _: () = redis::cmd("SET")
                    .arg(&key)
                    .arg(&json)
                    .arg("EX")
                    .arg(60) // 60 second TTL
                    .query_async(&mut conn)
                    .await?;
                Ok(())
            }
            Err(e) => {
                tracing::error!("Redis unavailable for OTC storage: {:?}", e);
                Err(e)
            }
        }
    }

    /// Consume OTC (get and delete atomically)
    pub async fn consume_otc(&self, code: &str) -> Result<Option<OtcTokenData>> {
        let key = format!("otc:{}", code);

        match self.queue.get_conn().await {
            Ok(mut conn) => {
                // Get value
                let cached: Option<String> = conn.get(&key).await?;

                if let Some(json) = cached {
                    // Delete key immediately (single-use)
                    let _: () = conn.del(&key).await?;

                    let tokens: OtcTokenData = serde_json::from_str(&json)?;
                    Ok(Some(tokens))
                } else {
                    Ok(None)
                }
            }
            Err(e) => {
                tracing::error!("Redis unavailable for OTC retrieval: {:?}", e);
                Err(e)
            }
        }
    }
}
