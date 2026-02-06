use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub struct UserVideoDto {
    pub id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub is_anonymous: bool,
    pub like_count: i64,
    pub url: Option<String>,
}
