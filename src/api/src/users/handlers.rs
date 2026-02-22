use axum::{
    Json,
    extract::{
        ConnectInfo, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, header},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use redis::AsyncCommands;
use sea_orm::*;
use serde::Deserialize;
use shared::entities::{abuse_reports, likes, users, videos};
use tokio::time::{Duration, Instant};
use uuid::Uuid;

use crate::{
    audit::logger::{AuditEvent, log_audit_event},
    auth::{
        dtos::UserDto,
        extractors::{AuthUser, SessionUser, ensure_user_onboarding_complete},
        service::AuthService,
    },
    error::{ApiErrorResponse, ApiResult},
    metrics::{
        USERNAME_RATE_LIMIT_EXCEEDED_TOTAL, USERNAME_UPDATE_CONFLICT_TOTAL, USERNAME_UPDATE_TOTAL,
        WS_CONNECTION_REJECTED_TOTAL,
    },
    realtime::{hub::HubRegisterError, messages::RealtimeSignalMessage},
    services::client_ip::ClientIp,
    services::rate_limiter::RateLimiter,
    state::AppState,
    users::username::{
        is_username_unique_violation, username_exists_case_insensitive, validate_username,
    },
    videos::handlers::abuse_report_model_to_dto,
};

use super::dtos::{
    CompleteOnboardingRequest, MyReportItemDto, MyReportsQuery, MyReportsResponse,
    OnboardingStatusResponse, UpdateUsernameRequest, UserVideoDto,
};

#[derive(Deserialize)]
pub struct PaginationQuery {
    pub page: Option<u64>,
    pub per_page: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct UsernameUpdateIdempotencyRecord {
    request_fingerprint: String,
    response: UserDto,
}

pub(crate) fn video_model_to_user_dto(
    v: videos::Model,
    config: &shared::config::Config,
    is_liked: bool,
) -> UserVideoDto {
    let is_published_like =
        v.status.eq_ignore_ascii_case("PUBLISHED") || v.status.eq_ignore_ascii_case("COMPLETED");
    // Legacy compatibility:
    // 1) For PUBLISHED rows, keep using configured public bucket.
    //    Some historical rows may have stale s3_bucket values.
    // 2) For non-PUBLISHED rows, only expose URL when row already points to public bucket.
    let has_public_object = is_published_like || v.s3_bucket == config.minio_bucket_videos;
    let url_bucket = if is_published_like {
        config.minio_bucket_videos.clone()
    } else {
        v.s3_bucket.clone()
    };

    let url = if has_public_object {
        Some(format!(
            "{}/{}/{}",
            config.minio_public_endpoint, url_bucket, v.s3_key
        ))
    } else {
        None
    };
    let thumbnail_url = if has_public_object {
        Some(format!(
            "{}/{}/{}_thumb.jpg",
            config.minio_public_endpoint, config.minio_bucket_videos, v.id
        ))
    } else {
        None
    };

    UserVideoDto {
        id: v.id,
        title: v.title,
        description: v.description,
        status: v.status,
        created_at: v.created_at,
        updated_at: v.updated_at,
        is_anonymous: v.is_anonymous,
        is_nsfw: v.is_nsfw,
        is_liked,
        like_count: v.like_count,
        url,
        thumbnail_url,
        processing_error_code: v.processing_error_code,
        processing_error_message: v.processing_error_message,
    }
}

async fn load_liked_video_id_set(
    db: &DatabaseConnection,
    user_id: Uuid,
    video_ids: &[Uuid],
) -> ApiResult<std::collections::HashSet<Uuid>> {
    if video_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }

    let liked_video_ids: Vec<Uuid> = likes::Entity::find()
        .select_only()
        .column(likes::Column::VideoId)
        .filter(
            Condition::all()
                .add(likes::Column::UserId.eq(user_id))
                .add(likes::Column::VideoId.is_in(video_ids.to_vec())),
        )
        .into_tuple()
        .all(db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    Ok(liked_video_ids.into_iter().collect())
}

fn user_profile_cache_key(user_id: Uuid, terms_current_version: &str) -> String {
    format!("user:{}:profile:v2:{}", user_id, terms_current_version)
}

/// GET /users/me
/// Returns the authenticated user's profile.
/// Cached for 24 hours.
pub async fn get_me(
    State(state): State<AppState>,
    SessionUser(user_id): SessionUser,
) -> ApiResult<Json<UserDto>> {
    let cache_key = user_profile_cache_key(user_id, &state.config.terms_current_version);

    // Try cache first
    if let Ok(mut conn) = state.queue.get_conn().await
        && let Ok(json) = conn.get::<_, String>(&cache_key).await
        && let Ok(cached) = serde_json::from_str::<UserDto>(&json)
    {
        return Ok(Json(cached));
    }

    // Fetch from DB
    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;
    if user.deleted_at.is_some() {
        return Err(ApiErrorResponse::unauthorized("Invalid token"));
    }

    let dto = UserDto::from_user_model(&user, &state.config.terms_current_version);

    // Cache result
    if let Ok(mut conn) = state.queue.get_conn().await
        && let Ok(json) = serde_json::to_string(&dto)
    {
        let _: Result<(), _> = conn.set_ex(&cache_key, json, 24 * 3600).await;
    }

    Ok(Json(dto))
}

pub(crate) fn build_onboarding_status(
    user: &users::Model,
    config: &shared::config::Config,
) -> OnboardingStatusResponse {
    let age_confirmed = user.age_confirmed_at.is_some();
    let terms_accepted = user.terms_accepted_at.is_some()
        && user.terms_accepted_version.as_deref() == Some(config.terms_current_version.as_str());
    OnboardingStatusResponse {
        completed: age_confirmed && terms_accepted,
        age_confirmed,
        terms_accepted,
        required_terms_version: config.terms_current_version.clone(),
        accepted_terms_version: user.terms_accepted_version.clone(),
        terms_url: Some("/system/terms".to_string()),
    }
}

pub async fn get_onboarding_status(
    State(state): State<AppState>,
    SessionUser(user_id): SessionUser,
) -> ApiResult<Json<OnboardingStatusResponse>> {
    let key = RateLimiter::onboarding_status_user_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(&key, state.config.onboarding_status_rpm_per_user, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            return Err(ApiErrorResponse::too_many_requests(
                "Too many onboarding status requests",
            ));
        }
        Err(_) => {
            return Err(ApiErrorResponse::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Onboarding status temporarily unavailable",
            ));
        }
    }

    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;

    Ok(Json(build_onboarding_status(&user, &state.config)))
}

pub async fn complete_onboarding(
    State(state): State<AppState>,
    SessionUser(user_id): SessionUser,
    ClientIp(client_ip): ClientIp,
    Json(payload): Json<CompleteOnboardingRequest>,
) -> ApiResult<Json<OnboardingStatusResponse>> {
    let key = RateLimiter::onboarding_complete_user_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(&key, state.config.onboarding_complete_rpm_per_user, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            return Err(ApiErrorResponse::too_many_requests(
                "Too many onboarding completion attempts",
            ));
        }
        Err(_) => {
            return Err(ApiErrorResponse::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Onboarding completion temporarily unavailable",
            ));
        }
    }

    if !payload.age_confirmed {
        return Err(ApiErrorResponse::bad_request("age_confirmed must be true"));
    }
    if payload.terms_version != state.config.terms_current_version {
        return Err(ApiErrorResponse::bad_request(format!(
            "terms_version must match current version: {}",
            state.config.terms_current_version
        )));
    }

    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;

    let now = chrono::Utc::now().fixed_offset();
    let already_complete = user.age_confirmed_at.is_some()
        && user.terms_accepted_at.is_some()
        && user.terms_accepted_version.as_deref()
            == Some(state.config.terms_current_version.as_str());

    let updated = if already_complete {
        user
    } else {
        let mut active: users::ActiveModel = user.into();
        active.age_confirmed_at = Set(Some(now));
        active.terms_accepted_at = Set(Some(now));
        active.terms_accepted_version = Set(Some(state.config.terms_current_version.clone()));
        active
            .update(&state.db)
            .await
            .map_err(ApiErrorResponse::db_error)?
    };

    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), _> = conn
            .del(user_profile_cache_key(
                user_id,
                &state.config.terms_current_version,
            ))
            .await;
    }
    let _ = state
        .security_event_service
        .record(
            "users.onboarding_complete",
            Some(user_id),
            &client_ip.to_string(),
            None,
            None,
            serde_json::json!({ "already_complete": already_complete }),
        )
        .await;

    Ok(Json(build_onboarding_status(&updated, &state.config)))
}

/// DELETE /users/me
/// Soft-deletes the user account and revokes all tokens.
pub async fn delete_account(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    ClientIp(client_ip): ClientIp,
) -> ApiResult<axum::http::StatusCode> {
    // 1. Soft Delete User
    let active_model = users::ActiveModel {
        id: Set(user_id),
        deleted_at: Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    };

    active_model
        .update(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    // 2. Revoke all tokens
    AuthService::logout_all(&state.db, user_id).await?;
    AuthService::revoke_extension_sessions(&state.db, user_id).await?;

    // 3. Invalidate Cache
    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), _> = conn
            .del(user_profile_cache_key(
                user_id,
                &state.config.terms_current_version,
            ))
            .await;
    }
    let _ = state
        .security_event_service
        .record(
            "users.delete_account",
            Some(user_id),
            &client_ip.to_string(),
            None,
            None,
            serde_json::json!({}),
        )
        .await;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// GET /users/me/videos
/// Returns a paginated list of videos uploaded by the user (excluding soft-deleted ones).
pub async fn get_my_videos(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<Vec<UserVideoDto>>> {
    let page = pagination.page.unwrap_or(1).max(1);
    let per_page = pagination.per_page.unwrap_or(20).clamp(1, 100);

    // Cache key specific to user and pagination
    // Versioned key to prevent stale schema/URL semantics from older cache entries.
    let cache_key = format!("user:{}:videos:v5:{}:{}", user_id, page, per_page);

    // Try cache
    if let Ok(mut conn) = state.queue.get_conn().await
        && let Ok(json) = conn.get::<_, String>(&cache_key).await
        && let Ok(cached) = serde_json::from_str::<Vec<UserVideoDto>>(&json)
    {
        return Ok(Json(cached));
    }

    // Query DB
    // Filter by user_id AND deleted_at IS NULL
    let videos = videos::Entity::find()
        .filter(videos::Column::UserId.eq(user_id))
        .filter(videos::Column::DeletedAt.is_null())
        .order_by_desc(videos::Column::CreatedAt)
        .paginate(&state.db, per_page);

    let items = videos
        .fetch_page(page - 1)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    let video_ids: Vec<Uuid> = items.iter().map(|v| v.id).collect();
    let liked_video_ids = load_liked_video_id_set(&state.db, user_id, &video_ids).await?;

    let dtos: Vec<UserVideoDto> = items
        .into_iter()
        .map(|v| {
            let is_liked = liked_video_ids.contains(&v.id);
            video_model_to_user_dto(v, &state.config, is_liked)
        })
        .collect();

    // Cache result (short TTL, e.g., 5 mins, invalidated on upload/delete)
    if let Ok(mut conn) = state.queue.get_conn().await
        && let Ok(json) = serde_json::to_string(&dtos)
    {
        let _: Result<(), _> = conn.set_ex(&cache_key, json, 300).await;
    }

    Ok(Json(dtos))
}

pub async fn get_my_reports(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Query(query): Query<MyReportsQuery>,
) -> ApiResult<Json<MyReportsResponse>> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let cursor = query.cursor.unwrap_or(0);

    let reports = abuse_reports::Entity::find()
        .filter(abuse_reports::Column::ReporterUserId.eq(user_id))
        .order_by_desc(abuse_reports::Column::CreatedAt)
        .paginate(&state.db, limit + 1)
        .fetch_page(cursor)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    let mut reports = reports;
    let next_cursor = if reports.len() as u64 > limit {
        reports.truncate(limit as usize);
        Some(cursor + 1)
    } else {
        None
    };

    let video_ids: Vec<Uuid> = reports.iter().map(|r| r.video_id).collect();
    let video_map: std::collections::HashMap<Uuid, videos::Model> = if video_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        videos::Entity::find()
            .filter(videos::Column::Id.is_in(video_ids))
            .all(&state.db)
            .await
            .map_err(ApiErrorResponse::db_error)?
            .into_iter()
            .map(|v| (v.id, v))
            .collect()
    };

    let items = reports
        .into_iter()
        .map(|report| {
            let video = video_map.get(&report.video_id);
            MyReportItemDto {
                report: abuse_report_model_to_dto(report),
                video_title: video.and_then(|v| v.title.clone()),
                video_status: video.map(|v| v.status.clone()),
                video_moderation_state: video.map(|v| v.moderation_state.clone()),
            }
        })
        .collect();

    Ok(Json(MyReportsResponse { items, next_cursor }))
}

pub async fn update_username(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(payload): Json<UpdateUsernameRequest>,
) -> ApiResult<Json<UserDto>> {
    let rate_key = RateLimiter::username_update_user_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(&rate_key, state.config.username_update_rpm_per_user, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            USERNAME_RATE_LIMIT_EXCEEDED_TOTAL.inc();
            log_audit_event(AuditEvent::UsernameUpdateRejected {
                user_id: Some(user_id),
                reason: "rate_limit_exceeded".to_string(),
                timestamp: chrono::Utc::now(),
            });
            return Err(ApiErrorResponse::too_many_requests(
                "Too many username update attempts",
            ));
        }
        Err(e) => {
            tracing::warn!(
                "Username update rate limit check failed (fail-open): {:?}",
                e
            );
        }
    }

    let validated = validate_username(&payload.username, &state.config).map_err(|e| {
        log_audit_event(AuditEvent::UsernameUpdateRejected {
            user_id: Some(user_id),
            reason: format!("invalid_username:{}", e.message()),
            timestamp: chrono::Utc::now(),
        });
        ApiErrorResponse::bad_request(e.message())
    })?;

    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let idempotency_cache_key = idempotency_key
        .as_ref()
        .map(|key| format!("user:{}:username:update:idempotency:{}", user_id, key));

    if let Some(ref cache_key) = idempotency_cache_key
        && let Ok(mut conn) = state.queue.get_conn().await
        && let Ok(raw) = conn.get::<_, String>(cache_key).await
        && let Ok(record) = serde_json::from_str::<UsernameUpdateIdempotencyRecord>(&raw)
    {
        if record.request_fingerprint != validated.normalized {
            return Err(ApiErrorResponse::conflict("idempotency_key_reuse_mismatch"));
        }
        return Ok(Json(record.response));
    }

    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::not_found("User not found"))?;

    let current_normalized = user
        .username_normalized
        .clone()
        .unwrap_or_else(|| user.username.to_ascii_lowercase());

    if current_normalized == validated.normalized && user.username == validated.original {
        let response = UserDto {
            id: user.id,
            username: user.username,
            email: user.email,
            avatar_url: user.avatar_url,
            age_confirmed: user.age_confirmed_at.is_some(),
            terms_accepted: user.terms_accepted_at.is_some()
                && user.terms_accepted_version.as_deref()
                    == Some(state.config.terms_current_version.as_str()),
            required_terms_version: state.config.terms_current_version.clone(),
            accepted_terms_version: user.terms_accepted_version,
        };
        if let Some(ref cache_key) = idempotency_cache_key
            && let Ok(mut conn) = state.queue.get_conn().await
        {
            let record = UsernameUpdateIdempotencyRecord {
                request_fingerprint: validated.normalized,
                response: response.clone(),
            };
            if let Ok(json) = serde_json::to_string(&record) {
                let _: Result<(), _> = conn.set_ex(cache_key, json, 600).await;
            }
        }
        return Ok(Json(response));
    }

    if username_exists_case_insensitive(&state.db, &validated.normalized, Some(user_id))
        .await
        .map_err(ApiErrorResponse::db_error)?
    {
        USERNAME_UPDATE_CONFLICT_TOTAL.inc();
        log_audit_event(AuditEvent::UsernameUpdateRejected {
            user_id: Some(user_id),
            reason: "username_taken".to_string(),
            timestamp: chrono::Utc::now(),
        });
        return Err(ApiErrorResponse::conflict("Username already taken"));
    }

    let mut active_model: users::ActiveModel = user.into();
    active_model.username = Set(validated.original);
    active_model.username_normalized = Set(Some(validated.normalized.clone()));
    active_model.username_updated_at = Set(Some(chrono::Utc::now().fixed_offset()));

    let updated = active_model.update(&state.db).await.map_err(|e| {
        if is_username_unique_violation(&e) {
            USERNAME_UPDATE_CONFLICT_TOTAL.inc();
            ApiErrorResponse::conflict("Username already taken")
        } else {
            ApiErrorResponse::db_error(e)
        }
    })?;

    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), _> = conn
            .del(user_profile_cache_key(
                user_id,
                &state.config.terms_current_version,
            ))
            .await;
    }

    let response = UserDto::from_user_model(&updated, &state.config.terms_current_version);

    if let Some(ref cache_key) = idempotency_cache_key
        && let Ok(mut conn) = state.queue.get_conn().await
    {
        let record = UsernameUpdateIdempotencyRecord {
            request_fingerprint: validated.normalized,
            response: response.clone(),
        };
        if let Ok(json) = serde_json::to_string(&record) {
            let _: Result<(), _> = conn.set_ex(cache_key, json, 600).await;
        }
    }

    USERNAME_UPDATE_TOTAL.inc();
    log_audit_event(AuditEvent::UsernameUpdated {
        user_id,
        timestamp: chrono::Utc::now(),
    });
    let _ = state
        .security_event_service
        .record(
            "users.username_update",
            Some(user_id),
            &client_ip.to_string(),
            None,
            None,
            serde_json::json!({}),
        )
        .await;

    Ok(Json(response))
}

/// GET /users/me/videos/ws
/// Realtime per-user status stream for upload lifecycle events.
pub async fn my_videos_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
) -> ApiResult<impl IntoResponse> {
    if !state.config.video_ws_enabled {
        return Err(ApiErrorResponse::not_found("Realtime endpoint disabled"));
    }

    validate_ws_origin(&state, &headers)?;

    let token = extract_bearer_token(&headers)
        .ok_or_else(|| ApiErrorResponse::unauthorized("Missing or invalid bearer token"))?;
    let claims = AuthService::validate_token(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    if state.token_revocation.is_revoked(claims.jti).await {
        return Err(ApiErrorResponse::unauthorized("Token revoked"));
    }

    let user_id = claims.sub;
    ensure_user_onboarding_complete(&state, user_id).await?;

    let client_ip = connect_info
        .map(|c| c.0.ip().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    let ip_key = RateLimiter::ws_connect_ip_key(&client_ip);
    match state
        .rate_limiter
        .check_and_increment(&ip_key, state.config.video_ws_connect_rpm_per_ip, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            WS_CONNECTION_REJECTED_TOTAL.inc();
            return Err(ApiErrorResponse::too_many_requests(
                "Too many websocket connection attempts from this IP",
            ));
        }
        Err(_) => {}
    }

    let user_key = RateLimiter::ws_connect_user_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(&user_key, state.config.video_ws_connect_rpm_per_user, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            WS_CONNECTION_REJECTED_TOTAL.inc();
            return Err(ApiErrorResponse::too_many_requests(
                "Too many websocket connection attempts for this user",
            ));
        }
        Err(_) => {}
    }

    let (conn_id, rx) = match state.realtime_hub.register_connection(user_id).await {
        Ok(value) => value,
        Err(HubRegisterError::GlobalLimitExceeded) => {
            return Err(ApiErrorResponse::too_many_requests(
                "Realtime connection capacity reached",
            ));
        }
        Err(HubRegisterError::PerUserLimitExceeded) => {
            return Err(ApiErrorResponse::too_many_requests(
                "Too many concurrent realtime connections for this user",
            ));
        }
    };

    let jti = claims.jti;
    let exp = claims.exp;
    let state_for_upgrade = state.clone();
    Ok(ws.on_upgrade(move |socket| async move {
        handle_ws_connection(state_for_upgrade, socket, user_id, conn_id, rx, jti, exp).await;
    }))
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ")
}

fn validate_ws_origin(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    // Native clients (RN / mobile) may include non-browser origins.
    // Enforce strict origin checks only for browser-like user agents.
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let is_browser_like = user_agent.contains("Mozilla/");
    if !is_browser_like {
        return Ok(());
    }

    let origin = match headers.get(header::ORIGIN) {
        Some(value) => value
            .to_str()
            .map_err(|_| ApiErrorResponse::forbidden("Invalid Origin header"))?,
        None => return Ok(()), // Mobile clients often don't send Origin.
    };

    if origin == "null" {
        return Ok(());
    }

    if state.config.environment == "development" {
        return Ok(());
    }

    if state.config.cors_allowed_origins.is_empty() {
        return Err(ApiErrorResponse::forbidden("Origin is not allowed"));
    }

    let allowed = state
        .config
        .cors_allowed_origins
        .split(',')
        .map(|s| s.trim())
        .any(|s| !s.is_empty() && s == origin);

    if !allowed {
        return Err(ApiErrorResponse::forbidden("Origin is not allowed"));
    }

    Ok(())
}

async fn load_user_videos_snapshot(
    state: &AppState,
    user_id: Uuid,
) -> ApiResult<Vec<UserVideoDto>> {
    let items = videos::Entity::find()
        .filter(videos::Column::UserId.eq(user_id))
        .filter(videos::Column::DeletedAt.is_null())
        .order_by_desc(videos::Column::CreatedAt)
        .all(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    let video_ids: Vec<Uuid> = items.iter().map(|v| v.id).collect();
    let liked_video_ids = load_liked_video_id_set(&state.db, user_id, &video_ids).await?;

    Ok(items
        .into_iter()
        .map(|v| {
            let is_liked = liked_video_ids.contains(&v.id);
            video_model_to_user_dto(v, &state.config, is_liked)
        })
        .collect())
}

async fn handle_ws_connection(
    state: AppState,
    socket: WebSocket,
    user_id: Uuid,
    conn_id: Uuid,
    mut rx: tokio::sync::mpsc::Receiver<String>,
    jti: Uuid,
    exp: usize,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Send initial snapshot.
    match load_user_videos_snapshot(&state, user_id).await {
        Ok(videos) => {
            let snapshot = RealtimeSignalMessage::snapshot(videos);
            match serde_json::to_string(&snapshot) {
                Ok(text) => {
                    if ws_sender.send(Message::Text(text)).await.is_err() {
                        state.realtime_hub.remove_connection(user_id, conn_id).await;
                        return;
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to serialize snapshot message: {:?}", e);
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to build realtime snapshot for user {}: {:?}",
                user_id,
                e
            );
        }
    }

    let pending = state
        .realtime_hub
        .mark_bootstrapped_and_take_pending(user_id, conn_id)
        .await;
    for msg in pending {
        if ws_sender.send(Message::Text(msg)).await.is_err() {
            state.realtime_hub.remove_connection(user_id, conn_id).await;
            return;
        }
    }

    let heartbeat_secs = state.config.video_ws_heartbeat_secs.max(5);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(heartbeat_secs));
    let mut last_pong = Instant::now();
    let timeout = Duration::from_secs(heartbeat_secs * 3);

    loop {
        tokio::select! {
            maybe_msg = rx.recv() => {
                match maybe_msg {
                    Some(msg) => {
                        if ws_sender.send(Message::Text(msg)).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            inbound = ws_receiver.next() => {
                match inbound {
                    Some(Ok(Message::Pong(_))) => {
                        last_pong = Instant::now();
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        last_pong = Instant::now();
                        if ws_sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                    None => break,
                }
            }
            _ = heartbeat.tick() => {
                let now_secs = chrono::Utc::now().timestamp() as usize;
                if now_secs >= exp {
                    break;
                }
                if state.token_revocation.is_revoked(jti).await {
                    break;
                }
                if Instant::now().duration_since(last_pong) > timeout {
                    break;
                }
                if ws_sender.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
        }
    }

    state.realtime_hub.remove_connection(user_id, conn_id).await;
}
