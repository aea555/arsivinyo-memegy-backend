use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoStatus {
    Draft,
    Processing,
    Published,
    Failed,
}

impl VideoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoStatus::Draft => "DRAFT",
            VideoStatus::Processing => "PROCESSING",
            VideoStatus::Published => "PUBLISHED",
            VideoStatus::Failed => "FAILED",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "DRAFT" => Some(VideoStatus::Draft),
            "PROCESSING" => Some(VideoStatus::Processing),
            "PUBLISHED" => Some(VideoStatus::Published),
            "FAILED" => Some(VideoStatus::Failed),
            _ => None,
        }
    }
}
