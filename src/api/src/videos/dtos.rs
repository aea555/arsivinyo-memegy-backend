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
