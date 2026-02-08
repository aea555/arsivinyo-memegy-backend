use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
