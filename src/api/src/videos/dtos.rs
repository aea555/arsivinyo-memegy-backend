use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
pub struct LikeVideoResponse {
    pub is_liked: bool,
    pub like_count: i64,
}

#[derive(Deserialize)]
pub struct InitUploadRequest {
    pub filename: String,
    pub size_bytes: i64,
    pub is_nsfw: bool,
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
    pub is_nsfw: Option<bool>,       // Toggle NSFW flag
}

#[derive(Serialize)]
pub struct RefreshDownloadResponse {
    pub download_url: String,
    pub expires_in_seconds: u64,
}

// Bulk Download DTOs
#[derive(Deserialize)]
pub struct CreateBulkDownloadRequest {
    pub video_ids: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Serialize)]
pub struct BulkDownloadJobResponse {
    pub job_id: Uuid,
    pub status: String,
    pub video_count: usize,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zip_size_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
}

#[derive(Serialize)]
pub struct BulkDownloadStatus {
    pub job_id: Uuid,
    pub status: String,
    pub video_ids: Vec<Uuid>,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zip_size_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_manifest: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub retry_count: i32,
}

// Search DTOs
#[derive(Deserialize)]
pub struct SearchVideosQuery {
    pub q: String, // Search query
    #[serde(default = "default_limit")]
    pub limit: u64, // Max results (default: 20)
    #[serde(default)]
    pub offset: u64, // Pagination offset
    #[serde(default = "default_sort")]
    pub sort: String, // relevance | recent | popular
    pub include_nsfw: Option<bool>,
}

fn default_limit() -> u64 {
    20
}

fn default_sort() -> String {
    "relevance".to_string()
}

#[derive(Deserialize)]
pub struct KeyboardSearchQuery {
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_sort")]
    pub sort: String,
}

#[derive(Serialize)]
pub struct KeyboardSearchItemDto {
    pub id: Uuid,
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration_seconds: Option<i32>,
    pub safe_size_bytes: u64,
    pub published: bool,
}

#[derive(Deserialize)]
pub struct CreateSendTicketRequest {
    pub host_app_hint: Option<String>,
    pub nonce: String,
}

#[derive(Serialize)]
pub struct SendTicketResponse {
    pub ticket_id: Uuid,
    pub media_url: String,
    pub fallback_share_url: String,
    pub expires_in_seconds: u64,
}

#[derive(Deserialize)]
pub struct BulkDeleteRequest {
    pub video_ids: Vec<Uuid>,
}
