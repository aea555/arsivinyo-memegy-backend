use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct InitUploadRequest {
    pub filename: String,
    pub size_bytes: i64,
}

#[derive(Serialize)]
pub struct InitUploadResponse {
    pub video_id: Uuid,
    pub upload_url: String,
}

#[derive(Deserialize)]
pub struct UpdateVideoRequest {
    pub title: Option<String>,       // Max 200 chars
    pub description: Option<String>, // Max 2000 chars
    pub is_anonymous: Option<bool>,  // Toggle anonymity
}
