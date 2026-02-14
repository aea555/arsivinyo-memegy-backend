use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserVideoDto {
    pub id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
    pub is_anonymous: bool,
    pub is_liked: bool,
    pub like_count: i64,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub processing_error_code: Option<String>,
    pub processing_error_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateUsernameRequest {
    pub username: String,
}
