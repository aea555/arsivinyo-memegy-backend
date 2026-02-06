use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::users::dtos::UserVideoDto;

pub const SIGNAL_VERSION: u8 = 1;

pub const EVENT_SNAPSHOT: &str = "video.status.snapshot";
pub const EVENT_PROCESSING: &str = "video.status.processing";
pub const EVENT_PUBLISHED: &str = "video.status.published";
pub const EVENT_FAILED: &str = "video.status.failed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeSignalMessage {
    #[serde(rename = "type")]
    pub event_type: String,
    pub event_id: Uuid,
    pub event_at: chrono::DateTime<chrono::FixedOffset>,
    pub version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<UserVideoDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub videos: Option<Vec<UserVideoDto>>,
}

impl RealtimeSignalMessage {
    pub fn snapshot(videos: Vec<UserVideoDto>) -> Self {
        Self {
            event_type: EVENT_SNAPSHOT.to_string(),
            event_id: Uuid::new_v4(),
            event_at: Utc::now().fixed_offset(),
            version: SIGNAL_VERSION,
            previous_status: None,
            video: None,
            videos: Some(videos),
        }
    }

    pub fn status(event_type: &str, previous_status: Option<String>, video: UserVideoDto) -> Self {
        Self {
            event_type: event_type.to_string(),
            event_id: Uuid::new_v4(),
            event_at: Utc::now().fixed_offset(),
            version: SIGNAL_VERSION,
            previous_status,
            video: Some(video),
            videos: None,
        }
    }
}
