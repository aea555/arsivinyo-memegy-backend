use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::videos::dtos::VideoReportDto;

#[derive(Debug, Clone, Serialize)]
pub struct AdminResponseMeta {
    pub request_id: String,
    pub actor_sub: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminEnvelope<T: Serialize> {
    pub metadata: AdminResponseMeta,
    pub data: T,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminTableInfo {
    pub name: String,
    pub primary_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListTablesResponse {
    pub tables: Vec<AdminTableInfo>,
}

#[derive(Debug, Deserialize)]
pub struct ListRowsQuery {
    pub limit: Option<u64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListRowsResponse {
    pub table: String,
    pub rows: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GetRowResponse {
    pub table: String,
    pub row: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct HardDeleteResponse {
    pub table: String,
    pub id: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertBanRequest {
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserBanStatusResponse {
    pub user_id: Uuid,
    pub active: bool,
    pub banned_until: Option<DateTime<Utc>>,
    pub reason: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub created_by_admin_sub: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpBanQuery {
    pub target: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpBanUpsertRequest {
    pub target: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IpBanStatusResponse {
    pub target: String,
    pub target_kind: String,
    pub active: bool,
    pub banned_until: Option<DateTime<Utc>>,
    pub reason: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub created_by_admin_sub: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InvestigationQuery {
    pub window_days: Option<i64>,
    pub limit: Option<u64>,
    pub cursor: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserIpInvestigationItem {
    pub ip: String,
    pub last_seen: DateTime<Utc>,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct IpUserInvestigationItem {
    pub user_id: Uuid,
    pub last_seen: DateTime<Utc>,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvestigationResponse<T: Serialize> {
    pub items: Vec<T>,
    pub next_cursor: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminAbuseReportListQuery {
    pub status: Option<String>,
    pub reason: Option<String>,
    pub severity_min: Option<i16>,
    pub video_id: Option<Uuid>,
    pub limit: Option<u64>,
    pub cursor: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminAbuseReportItem {
    pub report: VideoReportDto,
    pub reporter_username: Option<String>,
    pub video_title: Option<String>,
    pub video_status: Option<String>,
    pub video_moderation_state: Option<String>,
    pub uploader_user_id: Option<Uuid>,
    pub uploader_username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminAbuseReportListResponse {
    pub items: Vec<AdminAbuseReportItem>,
    pub next_cursor: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResolveAbuseReportRequest {
    pub resolution_code: String,
    #[serde(default)]
    pub resolution_note: Option<String>,
    #[serde(default)]
    pub status: Option<String>, // resolved|rejected
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminVideoModerationActionRequest {
    pub action: String, // quarantine|remove|restore
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub resolution_note: Option<String>,
    #[serde(default)]
    pub source_report_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminVideoModerationActionResponse {
    pub video_id: Uuid,
    pub moderation_state: String,
    pub moderation_reason_code: Option<String>,
    pub moderation_updated_at: Option<DateTime<Utc>>,
    pub moderation_updated_by: Option<String>,
    pub changed: bool,
}
