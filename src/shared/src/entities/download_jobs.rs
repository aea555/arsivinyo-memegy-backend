use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "download_jobs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub video_ids: Vec<Uuid>,
    pub status: String,
    pub idempotency_key: Option<String>,
    pub zip_s3_key: Option<String>,
    pub zip_url: Option<String>,
    pub zip_size_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub partial_manifest: Option<Json>,
    pub retry_count: i32,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: Option<DateTimeWithTimeZone>,
    pub completed_at: Option<DateTimeWithTimeZone>,
    pub expires_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

// Job status enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl JobStatus {
    pub fn as_str(&self) -> &str {
        match self {
            JobStatus::Pending => "PENDING",
            JobStatus::Processing => "PROCESSING",
            JobStatus::Completed => "COMPLETED",
            JobStatus::Failed => "FAILED",
        }
    }
}

impl FromStr for JobStatus {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PENDING" => Ok(JobStatus::Pending),
            "PROCESSING" => Ok(JobStatus::Processing),
            "COMPLETED" => Ok(JobStatus::Completed),
            "FAILED" => Ok(JobStatus::Failed),
            _ => Err(()),
        }
    }
}
