use serde::{Deserialize, Serialize};
use std::str::FromStr;

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
}

impl FromStr for VideoStatus {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DRAFT" => Ok(VideoStatus::Draft),
            "PROCESSING" => Ok(VideoStatus::Processing),
            "PUBLISHED" => Ok(VideoStatus::Published),
            "FAILED" => Ok(VideoStatus::Failed),
            _ => Err(()),
        }
    }
}
