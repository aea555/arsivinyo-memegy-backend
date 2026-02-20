use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::videos::dtos::VideoReportDto;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserVideoDto {
    pub id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
    pub is_anonymous: bool,
    pub is_nsfw: Option<bool>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingStatusResponse {
    pub completed: bool,
    pub age_confirmed: bool,
    pub terms_accepted: bool,
    pub required_terms_version: String,
    pub accepted_terms_version: Option<String>,
    pub terms_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompleteOnboardingRequest {
    pub age_confirmed: bool,
    pub terms_version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MyReportsQuery {
    pub limit: Option<u64>,
    pub cursor: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyReportItemDto {
    pub report: VideoReportDto,
    pub video_title: Option<String>,
    pub video_status: Option<String>,
    pub video_moderation_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyReportsResponse {
    pub items: Vec<MyReportItemDto>,
    pub next_cursor: Option<u64>,
}
