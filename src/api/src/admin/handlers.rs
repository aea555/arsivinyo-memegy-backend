use axum::{
    Json,
    extract::{ConnectInfo, Path, Query, State, ws::WebSocketUpgrade},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use redis::AsyncCommands;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, Statement, TransactionTrait,
};
use serde_json::json;
use shared::entities::{admin_audit_logs, users, videos};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

use crate::{
    auth::{
        dtos::{
            AuthResponse, ExtensionSessionRequest, ExtensionSessionResponse, RefreshRequest,
            RefreshResponse, UserDto,
        },
        extractors::{AuthUser, ExtensionAuth},
        service::{AuthService, RefreshAccessTokenError},
    },
    error::{ApiErrorResponse, ApiResult},
    state::AppState,
    users::{dtos::UpdateUsernameRequest, handlers::video_model_to_user_dto},
    videos::{
        dtos::{
            BulkDeleteRequest, BulkDownloadJobResponse, BulkDownloadStatus,
            CreateBulkDownloadRequest, CreateSendTicketRequest, InitUploadRequest,
            InitUploadResponse, KeyboardSearchItemDto, KeyboardSearchQuery, LikeVideoResponse,
            RefreshDownloadResponse, SearchVideosQuery, SendTicketResponse, UpdateVideoRequest,
        },
        handlers::{
            FeedQuery, VideoFeedItem, confirm_upload, create_send_ticket, get_bulk_download_status,
            get_feed, init_anonymous_upload, init_upload, redeem_send_ticket_media, search_videos,
            search_videos_keyboard, set_like_video, unset_like_video,
        },
    },
};
use shared::security::create_access_token;

use super::{
    dtos::{
        AdminEnvelope, AdminResponseMeta, AdminTableInfo, GetRowResponse, HardDeleteResponse,
        ListRowsQuery, ListRowsResponse, ListTablesResponse,
    },
    extractors::AdminPrincipal,
};

#[derive(Debug, serde::Deserialize)]
pub struct AdminUserVideosQuery {
    pub page: Option<u64>,
    pub per_page: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
enum TablePrimaryKey {
    Single(&'static str),
    Composite(&'static str, &'static str),
}

#[derive(Debug, Clone, Copy)]
struct TableSpec {
    name: &'static str,
    pk: TablePrimaryKey,
    order_by: &'static str,
}

const TABLE_SPECS: &[TableSpec] = &[
    TableSpec {
        name: "users",
        pk: TablePrimaryKey::Single("id"),
        order_by: "created_at DESC, id DESC",
    },
    TableSpec {
        name: "videos",
        pk: TablePrimaryKey::Single("id"),
        order_by: "created_at DESC, id DESC",
    },
    TableSpec {
        name: "likes",
        pk: TablePrimaryKey::Composite("user_id", "video_id"),
        order_by: "created_at DESC, user_id DESC, video_id DESC",
    },
    TableSpec {
        name: "refresh_tokens",
        pk: TablePrimaryKey::Single("id"),
        order_by: "created_at DESC, id DESC",
    },
    TableSpec {
        name: "download_jobs",
        pk: TablePrimaryKey::Single("id"),
        order_by: "created_at DESC, id DESC",
    },
    TableSpec {
        name: "send_tickets",
        pk: TablePrimaryKey::Single("ticket_id"),
        order_by: "created_at DESC, ticket_id DESC",
    },
    TableSpec {
        name: "extension_sessions",
        pk: TablePrimaryKey::Single("jti"),
        order_by: "issued_at DESC, jti DESC",
    },
    TableSpec {
        name: "admin_audit_logs",
        pk: TablePrimaryKey::Single("id"),
        order_by: "created_at DESC, id DESC",
    },
];

fn table_spec(name: &str) -> Option<&'static TableSpec> {
    TABLE_SPECS.iter().find(|spec| spec.name == name)
}

fn pk_display(pk: TablePrimaryKey) -> String {
    match pk {
        TablePrimaryKey::Single(col) => col.to_string(),
        TablePrimaryKey::Composite(a, b) => format!("{},{}", a, b),
    }
}

fn request_id_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn user_agent_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn response_meta(request_id: String, principal: &AdminPrincipal) -> AdminResponseMeta {
    AdminResponseMeta {
        request_id,
        actor_sub: principal.sub.clone(),
        timestamp: Utc::now(),
    }
}

async fn write_admin_audit(
    state: &AppState,
    principal: &AdminPrincipal,
    request_id: &str,
    action: &str,
    target_table: &str,
    target_id: Option<&str>,
    outcome: &str,
    user_agent: Option<&str>,
    metadata: serde_json::Value,
) {
    let active = admin_audit_logs::ActiveModel {
        id: Set(Uuid::new_v4()),
        actor_sub: Set(principal.sub.clone()),
        action: Set(action.to_string()),
        target_table: Set(target_table.to_string()),
        target_id: Set(target_id.map(str::to_string)),
        request_id: Set(request_id.to_string()),
        ip: Set(Some(principal.client_ip.to_string())),
        user_agent: Set(user_agent.map(str::to_string)),
        outcome: Set(outcome.to_string()),
        metadata_json: Set(metadata),
        created_at: Set(Utc::now().fixed_offset()),
    };

    if let Err(err) = active.insert(&state.db).await {
        tracing::warn!("Failed to persist admin audit log: {:?}", err);
    }
}

fn parse_cursor(query: &ListRowsQuery) -> Result<u64, ApiErrorResponse> {
    match query.cursor.as_deref() {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| ApiErrorResponse::bad_request("Invalid cursor")),
        None => Ok(0),
    }
}

pub async fn list_tables(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
) -> ApiResult<Json<AdminEnvelope<ListTablesResponse>>> {
    let request_id = request_id_from_headers(&headers);
    let user_agent = user_agent_from_headers(&headers);
    let tables = TABLE_SPECS
        .iter()
        .map(|spec| AdminTableInfo {
            name: spec.name.to_string(),
            primary_key: pk_display(spec.pk),
        })
        .collect::<Vec<_>>();

    write_admin_audit(
        &state,
        &principal,
        &request_id,
        "list_tables",
        "admin_catalog",
        None,
        "success",
        user_agent.as_deref(),
        json!({ "table_count": tables.len() }),
    )
    .await;

    Ok(Json(AdminEnvelope {
        metadata: response_meta(request_id, &principal),
        data: ListTablesResponse { tables },
    }))
}

pub async fn list_table_rows(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(table): Path<String>,
    Query(query): Query<ListRowsQuery>,
) -> ApiResult<Json<AdminEnvelope<ListRowsResponse>>> {
    let request_id = request_id_from_headers(&headers);
    let user_agent = user_agent_from_headers(&headers);
    let spec = match table_spec(&table) {
        Some(spec) => spec,
        None => {
            write_admin_audit(
                &state,
                &principal,
                &request_id,
                "list_rows",
                &table,
                None,
                "rejected_invalid_table",
                user_agent.as_deref(),
                json!({}),
            )
            .await;
            return Err(ApiErrorResponse::bad_request("Table is not allowed"));
        }
    };

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = parse_cursor(&query)?;
    let sql = format!(
        "SELECT row_to_json(t) AS row FROM (SELECT * FROM {} ORDER BY {} LIMIT $1 OFFSET $2) t",
        spec.name, spec.order_by
    );

    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            vec![(limit as i64).into(), (offset as i64).into()],
        ))
        .await
        .map_err(ApiErrorResponse::db_error)?;

    let mut serialized_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let value: sea_orm::JsonValue = row
            .try_get("", "row")
            .map_err(|e| ApiErrorResponse::internal_error(e.to_string()))?;
        serialized_rows.push(value);
    }
    let next_cursor = if serialized_rows.len() as u64 == limit {
        Some((offset + limit).to_string())
    } else {
        None
    };

    write_admin_audit(
        &state,
        &principal,
        &request_id,
        "list_rows",
        spec.name,
        None,
        "success",
        user_agent.as_deref(),
        json!({
            "limit": limit,
            "offset": offset,
            "row_count": serialized_rows.len(),
        }),
    )
    .await;

    Ok(Json(AdminEnvelope {
        metadata: response_meta(request_id, &principal),
        data: ListRowsResponse {
            table: spec.name.to_string(),
            rows: serialized_rows,
            next_cursor,
        },
    }))
}

pub async fn get_table_row(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((table, id)): Path<(String, String)>,
) -> ApiResult<Json<AdminEnvelope<GetRowResponse>>> {
    let request_id = request_id_from_headers(&headers);
    let user_agent = user_agent_from_headers(&headers);
    let spec = match table_spec(&table) {
        Some(spec) => spec,
        None => {
            write_admin_audit(
                &state,
                &principal,
                &request_id,
                "get_row",
                &table,
                Some(&id),
                "rejected_invalid_table",
                user_agent.as_deref(),
                json!({}),
            )
            .await;
            return Err(ApiErrorResponse::bad_request("Table is not allowed"));
        }
    };

    let row = match spec.pk {
        TablePrimaryKey::Single(pk_col) => {
            let parsed_id = Uuid::parse_str(&id)
                .map_err(|_| ApiErrorResponse::bad_request("Invalid row identifier"))?;
            let sql = format!(
                "SELECT row_to_json(t) AS row FROM (SELECT * FROM {} WHERE {} = $1 LIMIT 1) t",
                spec.name, pk_col
            );
            state
                .db
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    sql,
                    vec![parsed_id.into()],
                ))
                .await
                .map_err(ApiErrorResponse::db_error)?
        }
        TablePrimaryKey::Composite(first, second) => {
            let mut pieces = id.split(':');
            let first_id = pieces
                .next()
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or_else(|| ApiErrorResponse::bad_request("Invalid composite row identifier"))?;
            let second_id = pieces
                .next()
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or_else(|| ApiErrorResponse::bad_request("Invalid composite row identifier"))?;
            let sql = format!(
                "SELECT row_to_json(t) AS row FROM (SELECT * FROM {} WHERE {} = $1 AND {} = $2 LIMIT 1) t",
                spec.name, first, second
            );
            state
                .db
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    sql,
                    vec![first_id.into(), second_id.into()],
                ))
                .await
                .map_err(ApiErrorResponse::db_error)?
        }
    };

    let row = match row {
        Some(row) => row,
        None => {
            write_admin_audit(
                &state,
                &principal,
                &request_id,
                "get_row",
                spec.name,
                Some(&id),
                "not_found",
                user_agent.as_deref(),
                json!({}),
            )
            .await;
            return Err(ApiErrorResponse::not_found("Row not found"));
        }
    };
    let value: sea_orm::JsonValue = row
        .try_get("", "row")
        .map_err(|e| ApiErrorResponse::internal_error(e.to_string()))?;

    write_admin_audit(
        &state,
        &principal,
        &request_id,
        "get_row",
        spec.name,
        Some(&id),
        "success",
        user_agent.as_deref(),
        json!({}),
    )
    .await;

    Ok(Json(AdminEnvelope {
        metadata: response_meta(request_id, &principal),
        data: GetRowResponse {
            table: spec.name.to_string(),
            row: value,
        },
    }))
}

async fn delete_record_by_uuid(
    db: &sea_orm::DatabaseConnection,
    table: &str,
    column: &str,
    id: Uuid,
) -> Result<u64, ApiErrorResponse> {
    let sql = format!("DELETE FROM {} WHERE {} = $1", table, column);
    let result = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            vec![id.into()],
        ))
        .await
        .map_err(ApiErrorResponse::db_error)?;

    Ok(result.rows_affected())
}

async fn invalidate_user_related_cache(state: &AppState, user_id: Uuid) {
    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), _> = conn.del(format!("user:{}:profile", user_id)).await;
        let pattern = format!("user:{}:videos:*", user_id);
        let keys: Vec<String> = conn.keys(&pattern).await.unwrap_or_default();
        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(keys).await;
        }
    }
}

async fn invalidate_feed_cache(state: &AppState) {
    if let Ok(mut conn) = state.queue.get_conn().await {
        let keys: Vec<String> = conn.keys("feed:*").await.unwrap_or_default();
        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(keys).await;
        }
    }
}

pub async fn hard_delete_user(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AdminEnvelope<HardDeleteResponse>>> {
    let request_id = request_id_from_headers(&headers);
    let user_agent = user_agent_from_headers(&headers);
    let user = users::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    if user.is_none() {
        write_admin_audit(
            &state,
            &principal,
            &request_id,
            "hard_delete_user",
            "users",
            Some(&id.to_string()),
            "not_found",
            user_agent.as_deref(),
            json!({}),
        )
        .await;
        return Err(ApiErrorResponse::not_found("User not found"));
    }

    let user_videos = videos::Entity::find()
        .filter(videos::Column::UserId.eq(id))
        .all(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    for video in &user_videos {
        if let Err(err) = state
            .storage
            .delete_file(&video.s3_bucket, &video.s3_key)
            .await
        {
            write_admin_audit(
                &state,
                &principal,
                &request_id,
                "hard_delete_user",
                "users",
                Some(&id.to_string()),
                "storage_cleanup_failed",
                user_agent.as_deref(),
                json!({ "error": err.to_string(), "video_id": video.id.to_string() }),
            )
            .await;
            return Err(ApiErrorResponse::internal_error(
                "Failed to remove user media from storage",
            ));
        }
    }

    let txn = state.db.begin().await.map_err(ApiErrorResponse::db_error)?;
    let delete_result = users::Entity::delete_by_id(id)
        .exec(&txn)
        .await
        .map_err(ApiErrorResponse::db_error)?;
    txn.commit().await.map_err(ApiErrorResponse::db_error)?;
    if delete_result.rows_affected == 0 {
        return Err(ApiErrorResponse::not_found("User not found"));
    }

    invalidate_user_related_cache(&state, id).await;

    write_admin_audit(
        &state,
        &principal,
        &request_id,
        "hard_delete_user",
        "users",
        Some(&id.to_string()),
        "success",
        user_agent.as_deref(),
        json!({ "deleted_video_objects": user_videos.len() }),
    )
    .await;

    Ok(Json(AdminEnvelope {
        metadata: response_meta(request_id, &principal),
        data: HardDeleteResponse {
            table: "users".to_string(),
            id: id.to_string(),
            deleted: true,
        },
    }))
}

pub async fn hard_delete_video(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AdminEnvelope<HardDeleteResponse>>> {
    let request_id = request_id_from_headers(&headers);
    let user_agent = user_agent_from_headers(&headers);
    let video = videos::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found"))?;

    if let Err(err) = state
        .storage
        .delete_file(&video.s3_bucket, &video.s3_key)
        .await
    {
        write_admin_audit(
            &state,
            &principal,
            &request_id,
            "hard_delete_video",
            "videos",
            Some(&id.to_string()),
            "storage_cleanup_failed",
            user_agent.as_deref(),
            json!({ "error": err.to_string() }),
        )
        .await;
        return Err(ApiErrorResponse::internal_error(
            "Failed to remove video object from storage",
        ));
    }

    let txn = state.db.begin().await.map_err(ApiErrorResponse::db_error)?;
    let deleted = videos::Entity::delete_by_id(id)
        .exec(&txn)
        .await
        .map_err(ApiErrorResponse::db_error)?;
    txn.commit().await.map_err(ApiErrorResponse::db_error)?;
    if deleted.rows_affected == 0 {
        return Err(ApiErrorResponse::not_found("Video not found"));
    }

    invalidate_user_related_cache(&state, video.user_id).await;

    write_admin_audit(
        &state,
        &principal,
        &request_id,
        "hard_delete_video",
        "videos",
        Some(&id.to_string()),
        "success",
        user_agent.as_deref(),
        json!({ "owner_user_id": video.user_id.to_string() }),
    )
    .await;

    Ok(Json(AdminEnvelope {
        metadata: response_meta(request_id, &principal),
        data: HardDeleteResponse {
            table: "videos".to_string(),
            id: id.to_string(),
            deleted: true,
        },
    }))
}

async fn hard_delete_simple(
    state: &AppState,
    principal: &AdminPrincipal,
    headers: &HeaderMap,
    id: Uuid,
    table: &str,
    column: &str,
    action: &str,
) -> ApiResult<AdminEnvelope<HardDeleteResponse>> {
    let request_id = request_id_from_headers(headers);
    let user_agent = user_agent_from_headers(headers);

    let deleted = delete_record_by_uuid(&state.db, table, column, id).await?;
    if deleted == 0 {
        write_admin_audit(
            state,
            principal,
            &request_id,
            action,
            table,
            Some(&id.to_string()),
            "not_found",
            user_agent.as_deref(),
            json!({}),
        )
        .await;
        return Err(ApiErrorResponse::not_found("Record not found"));
    }

    write_admin_audit(
        state,
        principal,
        &request_id,
        action,
        table,
        Some(&id.to_string()),
        "success",
        user_agent.as_deref(),
        json!({}),
    )
    .await;

    Ok(AdminEnvelope {
        metadata: response_meta(request_id, principal),
        data: HardDeleteResponse {
            table: table.to_string(),
            id: id.to_string(),
            deleted: true,
        },
    })
}

pub async fn hard_delete_download_job(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AdminEnvelope<HardDeleteResponse>>> {
    let response = hard_delete_simple(
        &state,
        &principal,
        &headers,
        id,
        "download_jobs",
        "id",
        "hard_delete_download_job",
    )
    .await?;
    Ok(Json(response))
}

pub async fn hard_delete_send_ticket(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AdminEnvelope<HardDeleteResponse>>> {
    let response = hard_delete_simple(
        &state,
        &principal,
        &headers,
        id,
        "send_tickets",
        "ticket_id",
        "hard_delete_send_ticket",
    )
    .await?;
    Ok(Json(response))
}

pub async fn hard_delete_extension_session(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AdminEnvelope<HardDeleteResponse>>> {
    let response = hard_delete_simple(
        &state,
        &principal,
        &headers,
        id,
        "extension_sessions",
        "jti",
        "hard_delete_extension_session",
    )
    .await?;
    Ok(Json(response))
}

pub async fn hard_delete_refresh_token(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AdminEnvelope<HardDeleteResponse>>> {
    let response = hard_delete_simple(
        &state,
        &principal,
        &headers,
        id,
        "refresh_tokens",
        "id",
        "hard_delete_refresh_token",
    )
    .await?;
    Ok(Json(response))
}

fn admin_passthrough_state(state: &AppState) -> AppState {
    let mut cfg = (*state.config).clone();
    cfg.limit_feed_rpm = u64::MAX / 4;
    cfg.limit_upload_bytes_hourly = i64::MAX / 4;
    cfg.like_actions_rpm_limit = u64::MAX / 4;
    cfg.username_signup_rpm_per_ip = u64::MAX / 4;
    cfg.username_signup_attempts_per_ticket = u64::MAX / 4;
    cfg.username_update_rpm_per_user = u64::MAX / 4;
    cfg.keyboard_search_rpm = u64::MAX / 4;
    cfg.keyboard_send_ticket_rpm = u64::MAX / 4;
    cfg.keyboard_daily_send_cap = u64::MAX / 4;
    cfg.search_rpm_limit = u64::MAX / 4;
    cfg.ip_rate_limit_rpm = u64::MAX / 4;
    cfg.video_ws_connect_rpm_per_ip = u64::MAX / 4;
    cfg.video_ws_connect_rpm_per_user = u64::MAX / 4;
    cfg.video_ws_max_conn_per_user = usize::MAX / 4;
    cfg.video_ws_max_conn_global = usize::MAX / 4;
    AppState {
        config: Arc::new(cfg),
        rate_limiter: state.rate_limiter.with_bypass(true),
        ..state.clone()
    }
}

fn admin_extension_auth(user_id: Uuid, scopes: &[&str]) -> ExtensionAuth {
    ExtensionAuth {
        user_id,
        session_jti: Uuid::new_v4(),
        platform: "admin".to_string(),
        device_id_hash: format!("admin:{}", user_id),
        scope: scopes.iter().map(|s| s.to_string()).collect(),
    }
}

async fn audit_as_user_action(
    state: &AppState,
    principal: &AdminPrincipal,
    headers: &HeaderMap,
    action: &str,
    user_id: Uuid,
    outcome: &str,
    metadata: serde_json::Value,
) {
    let request_id = request_id_from_headers(headers);
    let user_agent = user_agent_from_headers(headers);
    write_admin_audit(
        state,
        principal,
        &request_id,
        action,
        "as_user",
        Some(&user_id.to_string()),
        outcome,
        user_agent.as_deref(),
        metadata,
    )
    .await;
}

pub async fn as_user_feed(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Json<Vec<VideoFeedItem>>> {
    let admin_state = admin_passthrough_state(&state);
    let result = get_feed(State(admin_state), AuthUser(user_id), Query(query)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_feed",
        user_id,
        outcome,
        json!({}),
    )
    .await;
    result
}

pub async fn as_user_init_upload(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<InitUploadRequest>,
) -> ApiResult<Json<InitUploadResponse>> {
    let admin_state = admin_passthrough_state(&state);
    let result = init_upload(State(admin_state), AuthUser(user_id), Json(payload)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_init_upload",
        user_id,
        outcome,
        json!({}),
    )
    .await;
    result
}

pub async fn as_user_init_anonymous_upload(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<InitUploadRequest>,
) -> ApiResult<Json<InitUploadResponse>> {
    let admin_state = admin_passthrough_state(&state);
    let result = init_anonymous_upload(State(admin_state), AuthUser(user_id), Json(payload)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_init_anonymous_upload",
        user_id,
        outcome,
        json!({}),
    )
    .await;
    result
}

pub async fn as_user_confirm_upload(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<axum::http::StatusCode> {
    let admin_state = admin_passthrough_state(&state);
    let result = confirm_upload(State(admin_state), AuthUser(user_id), Path(video_id)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_confirm_upload",
        user_id,
        outcome,
        json!({ "video_id": video_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_search_videos(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(params): Query<SearchVideosQuery>,
) -> ApiResult<Json<Vec<VideoFeedItem>>> {
    let admin_state = admin_passthrough_state(&state);
    let result = search_videos(State(admin_state), AuthUser(user_id), Query(params)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_search_videos",
        user_id,
        outcome,
        json!({}),
    )
    .await;
    result
}

pub async fn as_user_search_videos_keyboard(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(params): Query<KeyboardSearchQuery>,
) -> ApiResult<Json<Vec<KeyboardSearchItemDto>>> {
    let admin_state = admin_passthrough_state(&state);
    let extension = admin_extension_auth(user_id, &["keyboard.search", "keyboard.send"]);
    let result = search_videos_keyboard(State(admin_state), extension, Query(params)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_search_videos_keyboard",
        user_id,
        outcome,
        json!({}),
    )
    .await;
    result
}

pub async fn as_user_create_send_ticket(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<CreateSendTicketRequest>,
) -> ApiResult<Json<SendTicketResponse>> {
    let admin_state = admin_passthrough_state(&state);
    let extension = admin_extension_auth(user_id, &["keyboard.send", "keyboard.search"]);
    let result =
        create_send_ticket(State(admin_state), extension, Path(video_id), Json(payload)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_create_send_ticket",
        user_id,
        outcome,
        json!({ "video_id": video_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_redeem_send_ticket_media(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, ticket_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Response> {
    let admin_state = admin_passthrough_state(&state);
    let extension = admin_extension_auth(user_id, &["keyboard.send", "keyboard.search"]);
    let result = redeem_send_ticket_media(State(admin_state), extension, Path(ticket_id)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_redeem_send_ticket_media",
        user_id,
        outcome,
        json!({ "ticket_id": ticket_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_download_video(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Response> {
    use axum::response::{IntoResponse, Redirect};
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::DeletedAt.is_null())
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or has been deleted"))?;

    let presigned_url = state
        .storage
        .generate_presigned_get(
            &state.config.minio_bucket_videos,
            &video.s3_key,
            Duration::from_secs(60),
        )
        .await
        .map_err(|e| ApiErrorResponse::internal_error(format!("Failed to generate URL: {}", e)))?;

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_download_video",
        user_id,
        "success",
        json!({ "video_id": video_id.to_string() }),
    )
    .await;

    Ok(Redirect::temporary(&presigned_url).into_response())
}

pub async fn as_user_refresh_download_url(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<RefreshDownloadResponse>> {
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::DeletedAt.is_null())
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or has been deleted"))?;

    let presigned_url = state
        .storage
        .generate_presigned_get(
            &state.config.minio_bucket_videos,
            &video.s3_key,
            Duration::from_secs(60),
        )
        .await
        .map_err(|e| ApiErrorResponse::internal_error(format!("Failed to generate URL: {}", e)))?;

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_refresh_download_url",
        user_id,
        "success",
        json!({ "video_id": video_id.to_string() }),
    )
    .await;

    Ok(Json(RefreshDownloadResponse {
        download_url: presigned_url,
        expires_in_seconds: 60,
    }))
}

pub async fn as_user_create_bulk_download(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<CreateBulkDownloadRequest>,
) -> ApiResult<Json<BulkDownloadJobResponse>> {
    use shared::entities::download_jobs;

    if payload.video_ids.is_empty() {
        return Err(ApiErrorResponse::bad_request("No video IDs provided"));
    }

    let idempotency_key = payload
        .idempotency_key
        .clone()
        .unwrap_or_else(|| format!("admin-bulk:{}:{}", user_id, Uuid::new_v4()));
    let idempotency_cache_key = format!("bulk_download:idempotency:{}", idempotency_key);

    if let Ok(mut conn) = state.queue.get_conn().await {
        let existing_job_id: Option<String> = redis::cmd("GET")
            .arg(&idempotency_cache_key)
            .query_async(&mut conn)
            .await
            .ok()
            .flatten();

        if let Some(job_id_str) = existing_job_id
            && let Ok(job_id) = Uuid::parse_str(&job_id_str)
            && let Ok(Some(existing_job)) = download_jobs::Entity::find_by_id(job_id)
                .one(&state.db)
                .await
        {
            audit_as_user_action(
                &state,
                &principal,
                &headers,
                "as_user_create_bulk_download",
                user_id,
                "success",
                json!({ "idempotent_replay": true, "job_id": existing_job.id.to_string() }),
            )
            .await;
            return Ok(Json(BulkDownloadJobResponse {
                job_id: existing_job.id,
                status: existing_job.status,
                video_count: existing_job.video_ids.len(),
                created_at: existing_job.created_at,
                download_url: existing_job.zip_url,
                zip_size_bytes: existing_job.zip_size_bytes,
                error_message: existing_job.error_message,
                completed_at: existing_job.completed_at,
                expires_at: existing_job.expires_at,
            }));
        }
    }

    let videos_found = videos::Entity::find()
        .filter(videos::Column::Id.is_in(payload.video_ids.clone()))
        .filter(videos::Column::DeletedAt.is_null())
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .all(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    if videos_found.len() != payload.video_ids.len() {
        let found_ids: std::collections::HashSet<_> = videos_found.iter().map(|v| v.id).collect();
        let missing: Vec<_> = payload
            .video_ids
            .iter()
            .filter(|id| !found_ids.contains(id))
            .collect();
        return Err(ApiErrorResponse::bad_request(format!(
            "Some videos are not available: {:?}",
            missing
        )));
    }

    let job_id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    let expires_at = now + chrono::Duration::days(7);
    let new_job = download_jobs::ActiveModel {
        id: Set(job_id),
        user_id: Set(user_id),
        video_ids: Set(payload.video_ids.clone()),
        status: Set("PENDING".to_string()),
        idempotency_key: Set(Some(idempotency_key.clone())),
        zip_s3_key: Set(None),
        zip_url: Set(None),
        zip_size_bytes: Set(None),
        error_message: Set(None),
        partial_manifest: Set(None),
        retry_count: Set(0),
        created_at: Set(now),
        updated_at: Set(None),
        completed_at: Set(None),
        expires_at: Set(Some(expires_at)),
    };
    let job = new_job
        .insert(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), redis::RedisError> = redis::cmd("SETEX")
            .arg(&idempotency_cache_key)
            .arg(86400)
            .arg(job_id.to_string())
            .query_async(&mut conn)
            .await;
        let job_payload = serde_json::json!({
            "job_id": job_id,
            "user_id": user_id,
            "video_ids": payload.video_ids,
        });
        let _: Result<(), redis::RedisError> = redis::cmd("LPUSH")
            .arg("bulk_download_queue")
            .arg(job_payload.to_string())
            .query_async(&mut conn)
            .await;
    }

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_create_bulk_download",
        user_id,
        "success",
        json!({ "job_id": job.id.to_string(), "video_count": job.video_ids.len() }),
    )
    .await;

    Ok(Json(BulkDownloadJobResponse {
        job_id: job.id,
        status: job.status,
        video_count: job.video_ids.len(),
        created_at: job.created_at,
        download_url: None,
        zip_size_bytes: None,
        error_message: None,
        completed_at: None,
        expires_at: job.expires_at,
    }))
}

pub async fn as_user_get_bulk_download_status(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, job_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<BulkDownloadStatus>> {
    let admin_state = admin_passthrough_state(&state);
    let result =
        get_bulk_download_status(State(admin_state), AuthUser(user_id), Path(job_id)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_get_bulk_download_status",
        user_id,
        outcome,
        json!({ "job_id": job_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_set_like_video(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<LikeVideoResponse>> {
    let admin_state = admin_passthrough_state(&state);
    let result = set_like_video(State(admin_state), AuthUser(user_id), Path(video_id)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_set_like_video",
        user_id,
        outcome,
        json!({ "video_id": video_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_unset_like_video(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<LikeVideoResponse>> {
    let admin_state = admin_passthrough_state(&state);
    let result = unset_like_video(State(admin_state), AuthUser(user_id), Path(video_id)).await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_unset_like_video",
        user_id,
        outcome,
        json!({ "video_id": video_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_delete_video(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<axum::http::StatusCode> {
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::UserId.eq(user_id))
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not owned by user"))?;

    if video.deleted_at.is_none() {
        let was_published = video.status == "PUBLISHED";
        let mut active: videos::ActiveModel = video.into();
        active.deleted_at = Set(Some(Utc::now().into()));
        active
            .update(&state.db)
            .await
            .map_err(ApiErrorResponse::db_error)?;

        if was_published {
            invalidate_feed_cache(&state).await;
        }
        invalidate_user_related_cache(&state, user_id).await;
    }

    let result = Ok(axum::http::StatusCode::NO_CONTENT);
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_delete_video",
        user_id,
        "success",
        json!({ "video_id": video_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_update_video_metadata(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path((user_id, video_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<UpdateVideoRequest>,
) -> ApiResult<axum::http::StatusCode> {
    if let Some(title) = payload.title.as_ref()
        && title.len() > 200
    {
        return Err(ApiErrorResponse::bad_request(
            "Title too long. Maximum 200 characters.",
        ));
    }
    if let Some(description) = payload.description.as_ref()
        && description.len() > 2000
    {
        return Err(ApiErrorResponse::bad_request(
            "Description too long. Maximum 2000 characters.",
        ));
    }

    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::UserId.eq(user_id))
        .filter(videos::Column::DeletedAt.is_null())
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not owned by user"))?;

    let was_published = video.status == "PUBLISHED";
    let mut active: videos::ActiveModel = video.into();
    let mut has_changes = false;

    if let Some(title) = payload.title {
        let next = Some(title);
        if active.title.as_ref() != &next {
            active.title = Set(next);
            has_changes = true;
        }
    }
    if let Some(description) = payload.description {
        let next = Some(description);
        if active.description.as_ref() != &next {
            active.description = Set(next);
            has_changes = true;
        }
    }
    if let Some(is_anonymous) = payload.is_anonymous
        && active.is_anonymous.as_ref() != &is_anonymous
    {
        active.is_anonymous = Set(is_anonymous);
        has_changes = true;
    }

    if has_changes {
        active.updated_at = Set(Utc::now().into());
        active
            .update(&state.db)
            .await
            .map_err(ApiErrorResponse::db_error)?;

        if was_published {
            invalidate_feed_cache(&state).await;
        }
        invalidate_user_related_cache(&state, user_id).await;
    }

    let result = Ok(axum::http::StatusCode::OK);
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_update_video_metadata",
        user_id,
        "success",
        json!({ "video_id": video_id.to_string() }),
    )
    .await;
    result
}

pub async fn as_user_bulk_delete_videos(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<BulkDeleteRequest>,
) -> ApiResult<axum::http::StatusCode> {
    if !payload.video_ids.is_empty() {
        videos::Entity::update_many()
            .col_expr(
                videos::Column::DeletedAt,
                sea_orm::sea_query::Expr::value(Utc::now().fixed_offset()),
            )
            .filter(videos::Column::UserId.eq(user_id))
            .filter(videos::Column::Id.is_in(payload.video_ids))
            .exec(&state.db)
            .await
            .map_err(ApiErrorResponse::db_error)?;
        invalidate_user_related_cache(&state, user_id).await;
    }

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_bulk_delete_videos",
        user_id,
        "success",
        json!({}),
    )
    .await;

    Ok(axum::http::StatusCode::OK)
}

pub async fn as_user_get_profile(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<UserDto>> {
    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;
    let dto = UserDto {
        id: user.id,
        username: user.username,
        email: user.email,
        avatar_url: user.avatar_url,
    };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_get_profile",
        user_id,
        "success",
        json!({}),
    )
    .await;
    Ok(Json(dto))
}

pub async fn as_user_delete_account(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<axum::http::StatusCode> {
    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;

    let mut active: users::ActiveModel = user.into();
    active.deleted_at = Set(Some(Utc::now().fixed_offset()));
    active
        .update(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;
    AuthService::logout_all(&state.db, user_id)
        .await
        .map_err(ApiErrorResponse::from)?;
    AuthService::revoke_extension_sessions(&state.db, user_id)
        .await
        .map_err(ApiErrorResponse::from)?;
    invalidate_user_related_cache(&state, user_id).await;

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_delete_account",
        user_id,
        "success",
        json!({}),
    )
    .await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn as_user_get_videos(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<AdminUserVideosQuery>,
) -> ApiResult<Json<Vec<crate::users::dtos::UserVideoDto>>> {
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);

    let items = videos::Entity::find()
        .filter(videos::Column::UserId.eq(user_id))
        .filter(videos::Column::DeletedAt.is_null())
        .order_by_desc(videos::Column::CreatedAt)
        .paginate(&state.db, per_page)
        .fetch_page(page - 1)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    let video_ids: Vec<Uuid> = items.iter().map(|v| v.id).collect();
    let liked_video_ids: Vec<Uuid> = shared::entities::likes::Entity::find()
        .select_only()
        .column(shared::entities::likes::Column::VideoId)
        .filter(
            sea_orm::Condition::all()
                .add(shared::entities::likes::Column::UserId.eq(user_id))
                .add(shared::entities::likes::Column::VideoId.is_in(video_ids)),
        )
        .into_tuple()
        .all(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;
    let liked_set: std::collections::HashSet<Uuid> = liked_video_ids.into_iter().collect();

    let dtos = items
        .into_iter()
        .map(|v| {
            let is_liked = liked_set.contains(&v.id);
            video_model_to_user_dto(v, &state.config, is_liked)
        })
        .collect::<Vec<_>>();

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_get_videos",
        user_id,
        "success",
        json!({ "count": dtos.len() }),
    )
    .await;
    Ok(Json(dtos))
}

pub async fn as_user_update_username(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<UpdateUsernameRequest>,
) -> ApiResult<Json<UserDto>> {
    let validated = crate::users::username::validate_username(&payload.username, &state.config)
        .map_err(|e| ApiErrorResponse::bad_request(e.message()))?;

    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;

    if crate::users::username::username_exists_case_insensitive(
        &state.db,
        &validated.normalized,
        Some(user_id),
    )
    .await
    .map_err(ApiErrorResponse::db_error)?
    {
        return Err(ApiErrorResponse::conflict("Username already taken"));
    }

    let mut active: users::ActiveModel = user.into();
    active.username = Set(validated.original);
    active.username_normalized = Set(Some(validated.normalized));
    active.username_updated_at = Set(Some(Utc::now().fixed_offset()));
    let updated = active.update(&state.db).await.map_err(|e| {
        if crate::users::username::is_username_unique_violation(&e) {
            ApiErrorResponse::conflict("Username already taken")
        } else {
            ApiErrorResponse::db_error(e)
        }
    })?;

    invalidate_user_related_cache(&state, user_id).await;
    let dto = UserDto {
        id: updated.id,
        username: updated.username,
        email: updated.email,
        avatar_url: updated.avatar_url,
    };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_update_username",
        user_id,
        "success",
        json!({}),
    )
    .await;
    Ok(Json(dto))
}

pub async fn create_user_session(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<AuthResponse>> {
    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;

    let (access_token, refresh_token) = AuthService::issue_tokens_for_user(
        &state.db,
        &user,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
        state.config.refresh_token_ttl_days,
    )
    .await
    .map_err(ApiErrorResponse::from)?;

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "create_user_session",
        user_id,
        "success",
        json!({}),
    )
    .await;

    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
        user: UserDto {
            id: user.id,
            username: user.username,
            email: user.email,
            avatar_url: user.avatar_url,
        },
    }))
}

pub async fn create_user_extension_session(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<ExtensionSessionRequest>,
) -> ApiResult<Json<ExtensionSessionResponse>> {
    let admin_state = admin_passthrough_state(&state);
    let result = crate::auth::handlers::create_extension_session(
        State(admin_state),
        AuthUser(user_id),
        Json(payload),
    )
    .await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "create_user_extension_session",
        user_id,
        outcome,
        json!({}),
    )
    .await;
    result
}

pub async fn as_user_refresh_session(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<RefreshRequest>,
) -> ApiResult<Json<RefreshResponse>> {
    let (new_access_token, new_refresh_token) = match AuthService::refresh_access_token(
        &state.db,
        &state.token_revocation,
        &payload.access_token,
        &payload.refresh_token,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
        state.config.refresh_token_ttl_days,
    )
    .await
    {
        Ok(tokens) => tokens,
        Err(RefreshAccessTokenError::InvalidAccessToken)
        | Err(RefreshAccessTokenError::InvalidOrExpiredRefreshToken) => {
            return Err(ApiErrorResponse::unauthorized("Invalid or expired token"));
        }
        Err(RefreshAccessTokenError::ReuseDetected) => {
            return Err(ApiErrorResponse::unauthorized(
                "Session revoked due to refresh token reuse",
            ));
        }
        Err(RefreshAccessTokenError::Internal(e)) => {
            tracing::error!("Admin refresh failed: {:?}", e);
            return Err(ApiErrorResponse::internal_error("Failed to refresh token"));
        }
    };

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_refresh_session",
        user_id,
        "success",
        json!({}),
    )
    .await;

    Ok(Json(RefreshResponse {
        access_token: new_access_token,
        refresh_token: new_refresh_token,
    }))
}

pub async fn as_user_logout_all(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<axum::http::StatusCode> {
    users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;

    AuthService::logout_all(&state.db, user_id)
        .await
        .map_err(ApiErrorResponse::from)?;
    AuthService::revoke_extension_sessions(&state.db, user_id)
        .await
        .map_err(ApiErrorResponse::from)?;

    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_logout_all",
        user_id,
        "success",
        json!({}),
    )
    .await;

    Ok(axum::http::StatusCode::OK)
}

pub async fn as_user_my_videos_ws(
    State(state): State<AppState>,
    principal: AdminPrincipal,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    ws: WebSocketUpgrade,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
) -> ApiResult<impl IntoResponse> {
    let access_token = create_access_token(
        user_id,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
    )
    .map_err(ApiErrorResponse::from)?;

    let mut proxied_headers = headers.clone();
    let auth_header_value = axum::http::HeaderValue::from_str(&format!("Bearer {}", access_token))
        .map_err(|_| ApiErrorResponse::internal_error("Failed to compose auth header"))?;
    proxied_headers.insert(axum::http::header::AUTHORIZATION, auth_header_value);

    let admin_state = admin_passthrough_state(&state);
    let result =
        crate::users::handlers::my_videos_ws(State(admin_state), ws, proxied_headers, connect_info)
            .await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    audit_as_user_action(
        &state,
        &principal,
        &headers,
        "as_user_my_videos_ws",
        user_id,
        outcome,
        json!({}),
    )
    .await;
    result
}
