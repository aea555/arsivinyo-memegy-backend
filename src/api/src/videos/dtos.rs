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
}

fn default_limit() -> u64 {
    20
}

fn default_sort() -> String {
    "relevance".to_string()
}
