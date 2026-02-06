use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use chrono::Utc;
use sea_orm::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

use super::dtos::*;
use crate::auth::service::AuthService;
use crate::{
    auth::extractors::AuthUser,
    error::{ApiErrorResponse, ApiResult},
    state::AppState,
};
use redis::AsyncCommands;
use shared::{
    entities::{likes, videos},
    queue::VideoProcessJob,
};

#[derive(Deserialize)]
pub struct FeedQuery {
    pub sort: Option<String>, // "latest", "popular", "random"
    pub page: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct VideoFeedItem {
    pub id: Uuid,
    pub title: Option<String>,
    pub url: String, // Public URL
    pub like_count: i64,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploader: Option<UploaderInfo>, // None if video is anonymous
    pub is_liked: bool,
}

#[derive(Serialize, Deserialize)]
pub struct UploaderInfo {
    pub id: Uuid,
    pub username: String,
}

pub async fn get_feed(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Json<Vec<VideoFeedItem>>> {
    // Rate limiting for feed access
    let feed_rate_key = crate::services::rate_limiter::RateLimiter::feed_rpm_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(
            &feed_rate_key,
            state.config.limit_feed_rpm,
            60, // 1 minute window for RPM
        )
        .await
    {
        Ok(Ok(_)) => {
            // Within limit, continue
        }
        Ok(Err(count)) => {
            return Err(ApiErrorResponse::too_many_requests(format!(
                "Feed rate limit exceeded: {} requests/minute",
                count
            )));
        }
        Err(_) => {
            // Redis error - fail open
        }
    }

    let requested_sort = query.sort.as_deref().unwrap_or("random").to_ascii_lowercase();
    let sort = match requested_sort.as_str() {
        "latest" | "newest" => "latest",
        "popular" => "popular",
        _ => "random",
    };
    // Standardize on 1-based pagination for API
    let page = query.page.unwrap_or(1);
    let page = if page > 0 { page - 1 } else { 0 };
    let page_size = state.config.feed_page_size;

    // Try cache first (skip for random to keep it truly random)
    if sort != "random" {
        if let Ok(Some(cached)) = state.feed_cache.get_feed(sort, None, page).await {
            let base_url = format!(
                "{}/{}",
                state.config.minio_public_endpoint, state.config.minio_bucket_videos
            );

            let items: Vec<VideoFeedItem> = cached
                .into_iter()
                .map(|c| VideoFeedItem {
                    id: c.id,
                    title: c.title,
                    url: format!("{}/{}", base_url, c.url.split('/').last().unwrap_or(&c.url)),
                    like_count: c.like_count,
                    created_at: c.created_at,
                    uploader: if c.is_anonymous {
                        None
                    } else {
                        c.uploader.map(|u| UploaderInfo {
                            id: u.id,
                            username: u.username,
                        })
                    },
                    is_liked: false, // Will be populated shortly
                })
                .collect();

            // Batch check for likes
            let video_ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
            let liked_video_ids: Vec<Uuid> = likes::Entity::find()
                .select_only()
                .column(likes::Column::VideoId)
                .filter(
                    Condition::all()
                        .add(likes::Column::UserId.eq(user_id))
                        .add(likes::Column::VideoId.is_in(video_ids)),
                )
                .into_tuple()
                .all(&state.db)
                .await
                .map_err(|e| ApiErrorResponse::internal_error(e.to_string()))?;

            let liked_set: std::collections::HashSet<Uuid> = liked_video_ids.into_iter().collect();

            let items = items
                .into_iter()
                .map(|mut item| {
                    item.is_liked = liked_set.contains(&item.id);
                    item
                })
                .collect();

            return Ok(Json(items));
        }
    }

    // Cache miss or random - query DB with user join for non-anonymous videos
    use shared::entities::users;

    let mut select = videos::Entity::find()
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .filter(videos::Column::DeletedAt.is_null()) // Filter soft-deleted videos
        .find_also_related(users::Entity); // Left join users table

    match sort {
        "latest" => {
            select = select
                .order_by_desc(videos::Column::CreatedAt)
                .order_by_desc(videos::Column::Id);
        }
        "popular" => {
            select = select
                .order_by_desc(videos::Column::LikeCount)
                .order_by_desc(videos::Column::CreatedAt)
                .order_by_desc(videos::Column::Id);
        }
        _ => {
            select = select.order_by(
                sea_orm::sea_query::Expr::cust("RANDOM()"),
                sea_orm::Order::Asc,
            );
        }
    }

    let video_with_users: Vec<(videos::Model, Option<users::Model>)> = select
        .paginate(&state.db, page_size)
        .fetch_page(page)
        .await?;

    let base_url = format!(
        "{}/{}",
        state.config.minio_public_endpoint, state.config.minio_bucket_videos
    );

    let mut items: Vec<VideoFeedItem> = video_with_users
        .iter()
        .map(|(v, u)| VideoFeedItem {
            id: v.id,
            title: v.title.clone(),
            url: format!("{}/{}", base_url, v.s3_key),
            like_count: v.like_count,
            created_at: v.created_at,
            // Only include uploader for non-anonymous videos
            uploader: if v.is_anonymous {
                None
            } else {
                u.as_ref().map(|user| UploaderInfo {
                    id: user.id,
                    username: user.username.clone(),
                })
            },
            is_liked: false,
        })
        .collect();

    if !items.is_empty() {
        let video_ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
        let liked_video_ids: Vec<Uuid> = likes::Entity::find()
            .select_only()
            .column(likes::Column::VideoId)
            .filter(
                Condition::all()
                    .add(likes::Column::UserId.eq(user_id))
                    .add(likes::Column::VideoId.is_in(video_ids)),
            )
            .into_tuple()
            .all(&state.db)
            .await
            .map_err(|e| ApiErrorResponse::internal_error(e.to_string()))?;

        let liked_set: std::collections::HashSet<Uuid> = liked_video_ids.into_iter().collect();
        for item in &mut items {
            item.is_liked = liked_set.contains(&item.id);
        }
    }

    // Populate cache with full metadata
    if sort != "random" && !video_with_users.is_empty() {
        let cached_items: Vec<_> = video_with_users
            .iter()
            .map(|(v, u)| crate::cache::feed_cache::CachedVideoFeedItem {
                id: v.id,
                title: v.title.clone(),
                url: v.s3_key.clone(),
                thumbnail_url: None,
                like_count: v.like_count,
                created_at: v.created_at,
                deleted_at: v.deleted_at,
                is_anonymous: v.is_anonymous,
                uploader: if v.is_anonymous {
                    None
                } else {
                    u.as_ref()
                        .map(|user| crate::cache::feed_cache::CachedUploaderInfo {
                            id: user.id,
                            username: user.username.clone(),
                        })
                },
            })
            .collect();

        // Cache with 5-minute TTL (300 seconds)
        let _ = state
            .feed_cache
            .set_feed(sort, None, page, &cached_items, 300)
            .await;
    }

    Ok(Json(items))
}

pub async fn init_upload(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(payload): Json<InitUploadRequest>,
) -> ApiResult<Json<InitUploadResponse>> {
    // 1. Validate file size
    if payload.size_bytes > state.config.max_file_size_bytes {
        return Err(ApiErrorResponse::bad_request(format!(
            "File too large. Max size: {} bytes",
            state.config.max_file_size_bytes
        )));
    }

    // 2. Rate Limiting Check (check only - increment on confirm)
    let rate_key = crate::services::rate_limiter::RateLimiter::upload_bytes_key(&user_id);
    let current_usage = state.rate_limiter.get_count(&rate_key).await.unwrap_or(0) as i64;

    if current_usage + payload.size_bytes > state.config.limit_upload_bytes_hourly {
        return Err(ApiErrorResponse::too_many_requests(
            "Upload rate limit exceeded. Please try again later.",
        ));
    }

    // 3. Create DB Record (DRAFT)
    let video_id = Uuid::new_v4();
    let ext = std::path::Path::new(&payload.filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");

    let s3_key = format!("{}/{}.{}", user_id, video_id, ext);

    let new_video = videos::ActiveModel {
        id: Set(video_id),
        user_id: Set(user_id),
        title: Set(None),
        description: Set(None),
        s3_bucket: Set(state.config.minio_bucket_raw.clone()),
        s3_key: Set(s3_key.clone()),
        status: Set("DRAFT".to_string()),
        size_bytes: Set(payload.size_bytes),
        like_count: Set(0),
        is_anonymous: Set(false), // Regular upload, not anonymous
        deleted_at: Set(None),    // Not deleted
        created_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    };

    new_video.insert(&state.db).await?;

    // 4. Generate Presigned URL with configurable expiry
    let upload_url = state
        .storage
        .generate_presigned_put(
            &state.config.minio_bucket_raw,
            &s3_key,
            Duration::from_secs(state.config.presigned_url_expiry_secs),
        )
        .await
        .map_err(|e| {
            ApiErrorResponse::internal_error(format!("Failed to generate upload URL: {}", e))
        })?;

    // Ensure /users/me/videos reflects new DRAFT immediately.
    invalidate_user_video_cache(&state, user_id).await;

    Ok(Json(InitUploadResponse {
        video_id,
        upload_url,
    }))
}

/// Initialize anonymous video upload (hidden identity in public feed)
pub async fn init_anonymous_upload(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(payload): Json<InitUploadRequest>,
) -> ApiResult<Json<InitUploadResponse>> {
    // 1. Validate file size
    if payload.size_bytes > state.config.max_file_size_bytes {
        return Err(ApiErrorResponse::bad_request(format!(
            "File too large. Max size: {} bytes",
            state.config.max_file_size_bytes
        )));
    }

    // 2. Stricter Rate Limiting for Anonymous Uploads
    // Anonymous uploads have 50% lower quota to prevent abuse
    let rate_key = crate::services::rate_limiter::RateLimiter::upload_bytes_key(&user_id);
    let current_usage = state.rate_limiter.get_count(&rate_key).await.unwrap_or(0) as i64;

    let anonymous_limit = state.config.limit_upload_bytes_hourly / 2; // 128 MB instead of 256 MB
    if current_usage + payload.size_bytes > anonymous_limit {
        return Err(ApiErrorResponse::too_many_requests(
            "Anonymous upload rate limit exceeded. Please try again later.",
        ));
    }

    // 3. Create DB Record (DRAFT, ANONYMOUS)
    let video_id = Uuid::new_v4();
    let ext = std::path::Path::new(&payload.filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");

    let s3_key = format!("{}/{}.{}", user_id, video_id, ext);

    let new_video = videos::ActiveModel {
        id: Set(video_id),
        user_id: Set(user_id),
        title: Set(None),
        description: Set(None),
        s3_bucket: Set(state.config.minio_bucket_raw.clone()),
        s3_key: Set(s3_key.clone()),
        status: Set("DRAFT".to_string()),
        size_bytes: Set(payload.size_bytes),
        like_count: Set(0),
        is_anonymous: Set(true), // ANONYMOUS upload
        deleted_at: Set(None),   // Not deleted
        created_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    };

    new_video.insert(&state.db).await?;

    // 4. Generate Presigned URL with configurable expiry
    let upload_url = state
        .storage
        .generate_presigned_put(
            &state.config.minio_bucket_raw,
            &s3_key,
            Duration::from_secs(state.config.presigned_url_expiry_secs),
        )
        .await
        .map_err(|e| {
            ApiErrorResponse::internal_error(format!("Failed to generate upload URL: {}", e))
        })?;

    // Ensure /users/me/videos reflects new DRAFT immediately.
    invalidate_user_video_cache(&state, user_id).await;

    Ok(Json(InitUploadResponse {
        video_id,
        upload_url,
    }))
}

pub async fn confirm_upload(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let txn = state.db.begin().await?;

    // 1. SELECT FOR UPDATE for idempotency
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::UserId.eq(user_id))
        .lock_exclusive()
        .one(&txn)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not owned by you"))?;

    // 2. Idempot: If already processing/published, return success
    if video.status != "DRAFT" {
        txn.commit().await?;
        return Ok(StatusCode::ACCEPTED);
    }

    // 3. Verify file exists and check actual size
    let actual_size = state
        .storage
        .get_file_size(&state.config.minio_bucket_raw, &video.s3_key)
        .await
        .map_err(|_| ApiErrorResponse::bad_request("File not uploaded or not accessible"))?;

    // SECURITY: Enforce strict size check to prevent quota bypass
    // If actual size is significantly larger than claimed size (allowing small buffer for differences), reject it.
    // For strictness, we reject any size larger than claimed.
    if actual_size > video.size_bytes as u64 {
        txn.rollback().await?;
        return Err(ApiErrorResponse::bad_request(format!(
            "File larger than declared. Declared: {} bytes, Actual: {} bytes",
            video.size_bytes, actual_size
        )));
    }

    // Double check global max limit (redundant but safe)
    if actual_size > state.config.max_file_size_bytes as u64 {
        txn.rollback().await?;
        return Err(ApiErrorResponse::bad_request(
            "File exceeds global size limit",
        ));
    }

    // 4. Increment rate limit (only after verified upload)
    let rate_key = crate::services::rate_limiter::RateLimiter::upload_bytes_key(&user_id);
    let _ = state
        .rate_limiter
        .increment(&rate_key, state.config.rate_limit_window_secs)
        .await;

    // 5. Update Status and Actual Size
    let mut active_video: videos::ActiveModel = video.clone().into();
    active_video.status = Set("PROCESSING".to_string());

    // Update DB with actual size if different (e.g. user declared 10MB but uploaded 9MB)
    if actual_size != video.size_bytes as u64 {
        active_video.size_bytes = Set(actual_size as i64);
    }

    active_video.updated_at = Set(Utc::now().into());
    active_video.update(&txn).await?;

    // 6. Queue Job
    let job = VideoProcessJob {
        video_id,
        user_id,
        raw_bucket: video.s3_bucket,
        raw_key: video.s3_key,
    };

    state.queue.push_video_job(job).await.map_err(|e| {
        // Job queue failure - don't commit transaction
        ApiErrorResponse::internal_error(format!("Failed to queue job: {}", e))
    })?;

    // Commit transaction
    txn.commit().await?;

    // 7. Invalidate User's Video List Cache
    // This ensures the new video immediately appears in their list
    invalidate_user_video_cache(&state, user_id).await;

    Ok(StatusCode::ACCEPTED)
}

async fn invalidate_user_video_cache(state: &AppState, user_id: Uuid) {
    if let Ok(mut conn) = state.queue.get_conn().await {
        let pattern = format!("user:{}:videos:*", user_id);
        let keys: Vec<String> = conn.keys(&pattern).await.unwrap_or_default();
        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(&keys).await;
        }
    }
}

async fn invalidate_like_related_caches(state: &AppState, owner_user_id: Uuid, actor_user_id: Uuid) {
    let _ = state.feed_cache.invalidate_all().await;
    if let Ok(mut conn) = state.queue.get_conn().await {
        let mut keys: Vec<String> = Vec::new();

        let owner_videos_pattern = format!("user:{}:videos:*", owner_user_id);
        let actor_search_pattern = format!("search:cache:{}:*", actor_user_id);

        keys.extend(
            conn.keys::<_, Vec<String>>(&owner_videos_pattern)
                .await
                .unwrap_or_default(),
        );
        keys.extend(
            conn.keys::<_, Vec<String>>(&actor_search_pattern)
                .await
                .unwrap_or_default(),
        );

        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(keys).await;
        }
    }
}

async fn get_published_video_or_404(
    db: &DatabaseConnection,
    video_id: Uuid,
) -> ApiResult<videos::Model> {
    videos::Entity::find_by_id(video_id)
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .one(db)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not published"))
}

async fn get_video_like_count(txn: &DatabaseTransaction, video_id: Uuid) -> ApiResult<i64> {
    let video = videos::Entity::find_by_id(video_id)
        .one(txn)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not published"))?;
    Ok(video.like_count)
}

async fn enforce_like_rate_limit(state: &AppState, user_id: Uuid) -> ApiResult<()> {
    let rate_key = crate::services::rate_limiter::RateLimiter::like_actions_rpm_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(
            &rate_key,
            state.config.like_actions_rpm_limit,
            state.config.like_actions_window_secs,
        )
        .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(count)) => Err(ApiErrorResponse::too_many_requests(format!(
            "Like rate limit exceeded: {} actions per {} seconds (current: {})",
            state.config.like_actions_rpm_limit, state.config.like_actions_window_secs, count
        ))),
        Err(_) => Ok(()), // Fail open if Redis is unavailable
    }
}

async fn insert_like_if_missing(
    txn: &DatabaseTransaction,
    user_id: Uuid,
    video_id: Uuid,
) -> ApiResult<bool> {
    let result = txn
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO likes (user_id, video_id)
            VALUES ($1, $2)
            ON CONFLICT (user_id, video_id) DO NOTHING
            "#,
            vec![user_id.into(), video_id.into()],
        ))
        .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn set_like_video(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<Json<LikeVideoResponse>> {
    enforce_like_rate_limit(&state, user_id).await?;
    let video = get_published_video_or_404(&state.db, video_id).await?;

    let txn = state.db.begin().await?;
    if insert_like_if_missing(&txn, user_id, video_id).await? {
        videos::Entity::update_many()
            .col_expr(
                videos::Column::LikeCount,
                sea_orm::sea_query::Expr::col(videos::Column::LikeCount).add(1),
            )
            .filter(videos::Column::Id.eq(video_id))
            .exec(&txn)
            .await?;
    }
    let like_count = get_video_like_count(&txn, video_id).await?;

    txn.commit().await?;

    invalidate_like_related_caches(&state, video.user_id, user_id).await;

    Ok(Json(LikeVideoResponse {
        is_liked: true,
        like_count,
    }))
}

pub async fn unset_like_video(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<Json<LikeVideoResponse>> {
    enforce_like_rate_limit(&state, user_id).await?;
    let video = get_published_video_or_404(&state.db, video_id).await?;

    let txn = state.db.begin().await?;
    let delete_result = likes::Entity::delete_by_id((user_id, video_id))
        .exec(&txn)
        .await?;

    if delete_result.rows_affected > 0 {
        videos::Entity::update_many()
            .col_expr(
                videos::Column::LikeCount,
                sea_orm::sea_query::Expr::cust(
                    "CASE WHEN like_count > 0 THEN like_count - 1 ELSE 0 END",
                ),
            )
            .filter(videos::Column::Id.eq(video_id))
            .exec(&txn)
            .await?;
    }

    let like_count = get_video_like_count(&txn, video_id).await?;

    txn.commit().await?;

    invalidate_like_related_caches(&state, video.user_id, user_id).await;

    Ok(Json(LikeVideoResponse {
        is_liked: false,
        like_count,
    }))
}

/// Delete video (soft delete with audit trail)
pub async fn delete_video(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    // Rate limiting for deletion (10/hour per user)
    let delete_rate_key = format!("delete:{}:hourly", user_id);
    match state
        .rate_limiter
        .check_and_increment(&delete_rate_key, 10, 3600) // 10 deletions per hour
        .await
    {
        Ok(Ok(_)) => {
            // Within limit, continue
        }
        Ok(Err(count)) => {
            return Err(ApiErrorResponse::too_many_requests(format!(
                "Deletion rate limit exceeded: {} deletions/hour. Contact support for bulk deletion.",
                count
            )));
        }
        Err(_) => {
            // Redis error - fail open
        }
    }

    let txn = state.db.begin().await?;

    // 1. Fetch video with ownership verification
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::UserId.eq(user_id))
        .one(&txn)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not owned by you"))?;

    // 2. Prevent double-deletion (idempotency)
    if video.deleted_at.is_some() {
        txn.commit().await?;
        return Ok(StatusCode::NO_CONTENT); // Already deleted
    }

    // 3. Check if video is published (needs cache invalidation)
    let was_published = video.status == "PUBLISHED";

    // 4. Soft delete
    let mut active: videos::ActiveModel = video.into();
    active.deleted_at = Set(Some(Utc::now().into()));
    active.update(&txn).await?;

    txn.commit().await?;

    // 5. Invalidate feed cache ONLY if video was published
    if was_published {
        if let Err(e) = invalidate_feed_cache(&state).await {
            tracing::warn!("Failed to invalidate feed cache after deletion: {:?}", e);
            // Don't fail the request - cache will expire naturally
        }
    }

    // 6. Invalidate User's Video List Cache (Always, as list includes drafts)
    if let Ok(mut conn) = state.queue.get_conn().await {
        let pattern = format!("user:{}:videos:*", user_id);
        let keys: Vec<String> = conn.keys(&pattern).await.unwrap_or_default();
        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(&keys).await;
            tracing::info!(
                "Invalidated {} video list cache keys for user {} (delete)",
                keys.len(),
                user_id
            );
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Update video metadata (title, description, anonymity)
pub async fn update_video_metadata(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
    Json(payload): Json<UpdateVideoRequest>,
) -> ApiResult<StatusCode> {
    // Rate limiting for updates (30/hour per user)
    let update_rate_key = format!("update:{}:hourly", user_id);
    match state
        .rate_limiter
        .check_and_increment(&update_rate_key, 30, 3600) // 30 updates per hour
        .await
    {
        Ok(Ok(_)) => {
            // Within limit, continue
        }
        Ok(Err(count)) => {
            return Err(ApiErrorResponse::too_many_requests(format!(
                "Update rate limit exceeded: {} updates/hour",
                count
            )));
        }
        Err(_) => {
            // Redis error - fail open
        }
    }

    // 1. Validate payload
    if let Some(ref title) = payload.title {
        if title.len() > 200 {
            return Err(ApiErrorResponse::bad_request(
                "Title too long. Maximum 200 characters.",
            ));
        }
    }

    if let Some(ref description) = payload.description {
        if description.len() > 2000 {
            return Err(ApiErrorResponse::bad_request(
                "Description too long. Maximum 2000 characters.",
            ));
        }
    }

    // 2. Fetch video with ownership verification
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::UserId.eq(user_id))
        .filter(videos::Column::DeletedAt.is_null()) // Cannot update deleted videos
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not owned by you"))?;

    // 3. Check if video is published (needs cache invalidation)
    let was_published = video.status == "PUBLISHED";

    // 4. Apply updates
    let mut active: videos::ActiveModel = video.into();
    let mut has_changes = false;

    if let Some(title) = payload.title {
        active.title = Set(Some(title));
        has_changes = true;
    }

    if let Some(description) = payload.description {
        active.description = Set(Some(description));
        has_changes = true;
    }

    if let Some(is_anonymous) = payload.is_anonymous {
        active.is_anonymous = Set(is_anonymous);
        has_changes = true;
    }

    // Early return if no changes
    if !has_changes {
        return Ok(StatusCode::OK);
    }

    active.updated_at = Set(Utc::now().into());
    active.update(&state.db).await?;

    // 5. Invalidate feed cache ONLY if video was published
    if was_published {
        if let Err(e) = invalidate_feed_cache(&state).await {
            tracing::warn!("Failed to invalidate feed cache after update: {:?}", e);
            // Don't fail the request - cache will expire naturally
        }
    }

    // 6. Invalidate User's Video List Cache
    if let Ok(mut conn) = state.queue.get_conn().await {
        let pattern = format!("user:{}:videos:*", user_id);
        let keys: Vec<String> = conn.keys(&pattern).await.unwrap_or_default();
        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(&keys).await;
        }
    }

    Ok(StatusCode::OK)
}

/// Helper function to invalidate all feed cache keys
async fn invalidate_feed_cache(state: &AppState) -> anyhow::Result<()> {
    use redis::AsyncCommands;

    let mut conn = state.queue.get_conn().await?;
    let keys: Vec<String> = conn.keys("feed:*").await?;

    if !keys.is_empty() {
        let _: () = conn.del(&keys).await?; // Explicit type for never type fallback
        tracing::info!("Invalidated {} feed cache keys", keys.len());
    }

    Ok(())
}

// ============================================================================
// DOWNLOAD ENDPOINTS
// ============================================================================

/// Download a single video
/// Returns 302 redirect to S3 presigned GET URL (60s TTL)
pub async fn download_video(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<axum::response::Response> {
    use axum::response::{IntoResponse, Redirect};

    // 1. Check rate limits FIRST (before DB query)
    // Request-based limit: 30/min
    let request_key = format!("download:{}:rpm", user_id);
    match state.rate_limiter.check_rate(&request_key, 30).await {
        Ok(Ok(_remaining)) => {} // Under limit, proceed
        Ok(Err(current)) => {
            return Err(ApiErrorResponse::too_many_requests(format!(
                "Download rate limit exceeded (30 downloads/minute). Current: {}",
                current
            )));
        }
        Err(e) => {
            tracing::error!("Rate limit check failed: {:?}", e);
            return Err(ApiErrorResponse::internal_error("Rate limit check failed"));
        }
    }

    // 2. Fetch video with soft-delete and status checks
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::DeletedAt.is_null())
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error fetching video: {:?}", e);
            ApiErrorResponse::internal_error("Failed to fetch video")
        })?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or has been deleted"))?;

    // 3. Check bandwidth limit: 500MB/hour
    let bandwidth_key = format!("download:{}:bandwidth", user_id);
    let mut conn = state.queue.get_conn().await.map_err(|e| {
        tracing::warn!("Redis connection failed for bandwidth check: {:?}", e);
        // Allow download if Redis is down (fail-open for bandwidth, not security)
        ApiErrorResponse::internal_error("Bandwidth tracking unavailable")
    })?;

    let current_bandwidth: i64 = redis::cmd("GET")
        .arg(&bandwidth_key)
        .query_async(&mut conn)
        .await
        .unwrap_or(Some(0i64))
        .unwrap_or(0);

    let bandwidth_limit_mb = 500;
    let bandwidth_limit_bytes = bandwidth_limit_mb * 1024 * 1024;

    if current_bandwidth + video.size_bytes > bandwidth_limit_bytes {
        return Err(ApiErrorResponse::too_many_requests(format!(
            "Bandwidth limit exceeded ({} MB/hour). Current usage: {} MB",
            bandwidth_limit_mb,
            current_bandwidth / (1024 * 1024)
        )));
    }

    // 4. Generate presigned GET URL (60 second TTL)
    let presigned_url = state
        .storage
        .generate_presigned_get(
            &state.config.minio_bucket_videos,
            &video.s3_key,
            Duration::from_secs(60),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to generate presigned URL: {:?}", e);
            ApiErrorResponse::internal_error("Failed to generate download URL")
        })?;

    // 5. Increment bandwidth counter (3600s = 1 hour TTL)
    let new_total: i64 = redis::cmd("INCRBY")
        .arg(&bandwidth_key)
        .arg(video.size_bytes)
        .query_async(&mut conn)
        .await
        .unwrap_or(video.size_bytes);

    // Set expiry if this is the first increment
    if new_total == video.size_bytes {
        let _: Result<(), redis::RedisError> = redis::cmd("EXPIRE")
            .arg(&bandwidth_key)
            .arg(3600)
            .query_async(&mut conn)
            .await;
    }

    // 6. Increment request counter (60s = 1 minute TTL)
    let _: Result<u64, redis::RedisError> = redis::cmd("INCR")
        .arg(&request_key)
        .query_async(&mut conn)
        .await;

    let _: Result<i64, redis::RedisError> = redis::cmd("EXPIRE")
        .arg(&request_key)
        .arg(60)
        .query_async(&mut conn)
        .await;

    // 7. Log download event (analytics)
    tracing::info!(
        "Download: user={}, video={}, size={} bytes",
        user_id,
        video_id,
        video.size_bytes
    );

    // 8. Return 302 redirect to presigned URL
    Ok(Redirect::temporary(&presigned_url).into_response())
}

/// Refresh an expired download URL
/// Returns new presigned URL for same video (higher rate limit)
pub async fn refresh_download_url(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<Json<RefreshDownloadResponse>> {
    // Rate limit: 10/min (higher than regular downloads)
    let refresh_key = format!("download:{}:refresh:rpm", user_id);
    match state.rate_limiter.check_rate(&refresh_key, 10).await {
        Ok(Ok(_remaining)) => {} // Under limit, proceed
        Ok(Err(current)) => {
            return Err(ApiErrorResponse::too_many_requests(format!(
                "URL refresh rate limit exceeded (10 refreshes/minute). Current: {}",
                current
            )));
        }
        Err(e) => {
            tracing::error!("Rate limit check failed: {:?}", e);
            return Err(ApiErrorResponse::internal_error("Rate limit check failed"));
        }
    }

    // Fetch video (same validations as download)
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::DeletedAt.is_null())
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error fetching video: {:?}", e);
            ApiErrorResponse::internal_error("Failed to fetch video")
        })?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or has been deleted"))?;

    // Generate new presigned URL (60s TTL)
    let presigned_url = state
        .storage
        .generate_presigned_get(
            &state.config.minio_bucket_videos,
            &video.s3_key,
            Duration::from_secs(60),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to generate presigned URL: {:?}", e);
            ApiErrorResponse::internal_error("Failed to generate download URL")
        })?;

    // Increment counter for rate limiting
    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<u64, redis::RedisError> = redis::cmd("INCR")
            .arg(&refresh_key)
            .query_async(&mut conn)
            .await;

        let _: Result<i64, redis::RedisError> = redis::cmd("EXPIRE")
            .arg(&refresh_key)
            .arg(60)
            .query_async(&mut conn)
            .await;
    }

    tracing::info!(
        "Download URL refreshed: user={}, video={}",
        user_id,
        video_id
    );

    Ok(Json(RefreshDownloadResponse {
        download_url: presigned_url,
        expires_in_seconds: 60,
    }))
}

// ============================================================================
// BULK DOWNLOAD ENDPOINTS
// ============================================================================

/// Create a bulk download job
pub async fn create_bulk_download(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(req): Json<CreateBulkDownloadRequest>,
) -> ApiResult<Json<BulkDownloadJobResponse>> {
    use shared::entities::{download_jobs, videos};

    // 1. Validate video count (max 10)
    if req.video_ids.is_empty() {
        return Err(ApiErrorResponse::bad_request("No video IDs provided"));
    }
    if req.video_ids.len() > 10 {
        return Err(ApiErrorResponse::bad_request(
            "Maximum 10 videos per bulk download",
        ));
    }

    // 2. Generate or use provided idempotency key
    let idempotency_key = req
        .idempotency_key
        .unwrap_or_else(|| format!("bulk:{}:{}", user_id, Uuid::new_v4()));

    // 3. Check idempotency (24h cache)
    let idempotency_cache_key = format!("bulk_download:idempotency:{}", idempotency_key);
    if let Ok(mut conn) = state.queue.get_conn().await {
        let existing_job_id: Option<String> = redis::cmd("GET")
            .arg(&idempotency_cache_key)
            .query_async(&mut conn)
            .await
            .ok()
            .flatten();

        if let Some(job_id_str) = existing_job_id {
            if let Ok(job_id) = Uuid::parse_str(&job_id_str) {
                // Return existing job
                if let Ok(Some(existing_job)) = download_jobs::Entity::find_by_id(job_id)
                    .one(&state.db)
                    .await
                {
                    tracing::info!(
                        "Returning existing bulk download job: {} (idempotency)",
                        job_id
                    );
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
        }
    }

    // 4. Check active jobs limit (3 concurrent per user)
    let active_count = download_jobs::Entity::find()
        .filter(download_jobs::Column::UserId.eq(user_id))
        .filter(download_jobs::Column::Status.is_in(["PENDING", "PROCESSING"]))
        .count(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to count active jobs: {:?}", e);
            ApiErrorResponse::internal_error("Failed to check job limits")
        })?;

    if active_count >= 3 {
        return Err(ApiErrorResponse::too_many_requests(
            "Maximum 3 active bulk download jobs. Please wait for existing jobs to complete.",
        ));
    }

    // 5. Check daily limit (10 jobs/day)
    let today_start = Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .fixed_offset();

    let daily_count = download_jobs::Entity::find()
        .filter(download_jobs::Column::UserId.eq(user_id))
        .filter(download_jobs::Column::CreatedAt.gte(today_start))
        .count(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to count daily jobs: {:?}", e);
            ApiErrorResponse::internal_error("Failed to check job limits")
        })?;

    if daily_count >= 10 {
        return Err(ApiErrorResponse::too_many_requests(
            "Daily limit of 10 bulk downloads reached. Resets at midnight UTC.",
        ));
    }

    // 6. Validate all videos exist, are published, and not deleted (CHECKPOINT 1)
    let videos_found = videos::Entity::find()
        .filter(videos::Column::Id.is_in(req.video_ids.clone()))
        .filter(videos::Column::DeletedAt.is_null())
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .all(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch videos: {:?}", e);
            ApiErrorResponse::internal_error("Failed to validate videos")
        })?;

    if videos_found.len() != req.video_ids.len() {
        let found_ids: std::collections::HashSet<_> = videos_found.iter().map(|v| v.id).collect();
        let missing: Vec<_> = req
            .video_ids
            .iter()
            .filter(|id| !found_ids.contains(id))
            .collect();

        return Err(ApiErrorResponse::bad_request(format!(
            "Some videos are not available: {:?}",
            missing
        )));
    }

    // 7. Create job record
    let job_id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    let expires_at = now + chrono::Duration::days(7);

    let new_job = download_jobs::ActiveModel {
        id: Set(job_id),
        user_id: Set(user_id),
        video_ids: Set(req.video_ids.clone()),
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

    let job = new_job.insert(&state.db).await.map_err(|e| {
        tracing::error!("Failed to create bulk download job: {:?}", e);
        ApiErrorResponse::internal_error("Failed to create download job")
    })?;

    // 8. Cache idempotency key (24h TTL)
    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), redis::RedisError> = redis::cmd("SETEX")
            .arg(&idempotency_cache_key)
            .arg(86400) // 24 hours
            .arg(job_id.to_string())
            .query_async(&mut conn)
            .await;
    }

    // 9. Enqueue job to worker (Redis queue)
    if let Ok(mut conn) = state.queue.get_conn().await {
        let job_payload = serde_json::json!({
            "job_id": job_id,
            "user_id": user_id,
            "video_ids": req.video_ids,
        });

        let _: Result<(), redis::RedisError> = redis::cmd("LPUSH")
            .arg("bulk_download_queue")
            .arg(job_payload.to_string())
            .query_async(&mut conn)
            .await;

        tracing::info!(
            "Enqueued bulk download job: {} ({} videos)",
            job_id,
            req.video_ids.len()
        );
    }

    // 10. Return job response
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

/// Get bulk download job status  
pub async fn get_bulk_download_status(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(job_id): Path<Uuid>,
) -> ApiResult<Json<BulkDownloadStatus>> {
    use shared::entities::download_jobs;

    // Fetch job
    let job = download_jobs::Entity::find_by_id(job_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch job: {:?}", e);
            ApiErrorResponse::internal_error("Failed to fetch job status")
        })?
        .ok_or_else(|| ApiErrorResponse::not_found("Download job not found"))?;

    // Verify ownership
    if job.user_id != user_id {
        return Err(ApiErrorResponse::not_found("Download job not found"));
    }

    // Return status
    Ok(Json(BulkDownloadStatus {
        job_id: job.id,
        status: job.status,
        video_ids: job.video_ids,
        created_at: job.created_at,
        download_url: job.zip_url,
        zip_size_bytes: job.zip_size_bytes,
        error_message: job.error_message,
        partial_manifest: job.partial_manifest,
        completed_at: job.completed_at,
        expires_at: job.expires_at,
        retry_count: job.retry_count,
    }))
}

// ============================================================================
// SEARCH ENDPOINT - PRODUCTION HARDENED
// ============================================================================

/// Search configuration parameters for validation
#[derive(Clone, Copy)]
pub struct SearchConfig {
    pub max_tokens: usize,
    pub max_token_length: usize,
    pub max_query_chars: usize,
    pub timeout_secs: u64,
    pub cache_ttl_secs: usize,
    pub rpm_limit: u64,
}

impl SearchConfig {
    /// Create SearchConfig from shared::Config
    pub fn from_config(config: &shared::config::Config) -> Self {
        Self {
            max_tokens: config.search_max_tokens,
            max_token_length: config.search_max_token_length,
            max_query_chars: config.search_max_query_chars,
            timeout_secs: config.search_timeout_secs,
            cache_ttl_secs: config.search_cache_ttl_secs,
            rpm_limit: config.search_rpm_limit,
        }
    }
}

/// Validates search query complexity to prevent ReDoS attacks
fn validate_search_query(query: &str, config: &SearchConfig) -> Result<String, ApiErrorResponse> {
    // 1. Remove control characters and normalize whitespace
    let normalized: String = query
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // 2. Check if empty after normalization
    if normalized.is_empty() {
        return Err(ApiErrorResponse::bad_request("Query cannot be empty"));
    }

    // 3. Character count limit (Unicode-aware)
    if normalized.chars().count() > config.max_query_chars {
        return Err(ApiErrorResponse::bad_request(format!(
            "Query too long (max {} characters)",
            config.max_query_chars
        )));
    }

    // 4. Token count limit (ReDoS protection)
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    if tokens.len() > config.max_tokens {
        return Err(ApiErrorResponse::bad_request(format!(
            "Too many search terms (max {})",
            config.max_tokens
        )));
    }

    // 5. Individual token length limit
    for token in &tokens {
        if token.chars().count() > config.max_token_length {
            return Err(ApiErrorResponse::bad_request(format!(
                "Search term too long (max {} characters per word)",
                config.max_token_length
            )));
        }
    }

    Ok(normalized)
}

/// Logs suspicious search queries for security monitoring
fn log_suspicious_query(user_id: Uuid, query: &str, reason: &str) {
    tracing::warn!(
        security = "suspicious_search",
        user_id = %user_id,
        query_length = query.len(),
        query_preview = &query[..query.len().min(50)],
        reason = reason,
        "Suspicious search query detected"
    );
}

/// Track search analytics in Redis
async fn track_search_analytics(
    queue: &shared::queue::QueueService,
    user_id: Uuid,
    query: &str,
    result_count: usize,
    cache_hit: bool,
    duration_ms: u64,
) {
    if let Ok(mut conn) = queue.get_conn().await {
        // Track popular queries (global)
        let _: Result<(), redis::RedisError> = redis::cmd("ZINCRBY")
            .arg("search:popular_queries")
            .arg(1)
            .arg(query.to_lowercase())
            .query_async(&mut conn)
            .await;

        // Track per-user query count (for abuse detection)
        let user_key = format!("search:user:{}:count", user_id);
        let _: Result<(), redis::RedisError> = redis::cmd("INCR")
            .arg(&user_key)
            .query_async(&mut conn)
            .await;
        let _: Result<(), redis::RedisError> = redis::cmd("EXPIRE")
            .arg(&user_key)
            .arg(3600) // 1 hour window
            .query_async(&mut conn)
            .await;

        // Log search metrics
        tracing::info!(
            event = "search_executed",
            user_id = %user_id,
            query_len = query.len(),
            result_count = result_count,
            cache_hit = cache_hit,
            duration_ms = duration_ms,
            "Search completed"
        );
    }
}

/// Search videos using PostgreSQL full-text search
///
/// Security features:
/// - Query complexity validation (ReDoS protection)
/// - Input normalization (control chars removed)
/// - User-scoped cache (cache poisoning prevention)
/// - Query timeout (resource exhaustion protection)
/// - Rate limiting before cache (bypass prevention)
/// - Suspicious query logging (abuse detection)
/// - Analytics tracking (pattern analysis)
pub async fn search_videos(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Query(params): Query<SearchVideosQuery>,
) -> ApiResult<Json<Vec<VideoFeedItem>>> {
    let start_time = std::time::Instant::now();
    let search_config = SearchConfig::from_config(&state.config);

    // 1. RATE LIMITING (BEFORE cache - prevents bypass attacks)
    let search_key = format!("search:{}:rpm", user_id);
    match state
        .rate_limiter
        .check_rate(&search_key, search_config.rpm_limit)
        .await
    {
        Ok(Ok(_remaining)) => {}
        Ok(Err(current)) => {
            log_suspicious_query(
                user_id,
                &params.q,
                &format!("rate_limit_exceeded:{}", current),
            );
            return Err(ApiErrorResponse::too_many_requests(format!(
                "Search rate limit exceeded ({} searches/minute). Current: {}",
                search_config.rpm_limit, current
            )));
        }
        Err(e) => {
            tracing::error!("Rate limit check failed: {:?}", e);
            return Err(ApiErrorResponse::internal_error("Rate limit check failed"));
        }
    }

    // 2. VALIDATE & NORMALIZE QUERY (ReDoS protection)
    let query = validate_search_query(&params.q, &search_config)?;

    // 3. Security logging for suspicious patterns
    let token_count = query.split_whitespace().count();
    if query.len() > 150 || token_count > 30 {
        log_suspicious_query(
            user_id,
            &query,
            &format!("high_complexity:len={},tokens={}", query.len(), token_count),
        );
    }

    // 4. CACHE CHECK with user_id (prevents cache poisoning)
    let limit = params.limit.min(100);
    let offset = params.offset;
    let cache_key = format!(
        "search:cache:{}:{}:{}:{}:{}",
        user_id, // CRITICAL: User-scoped cache
        query.to_lowercase(),
        params.sort,
        limit,
        offset
    );

    // Try cache AFTER rate limiting (prevents rate limit bypass)
    if let Ok(mut redis_conn) = state.queue.get_conn().await {
        if let Ok(cached_json) = redis::cmd("GET")
            .arg(&cache_key)
            .query_async::<String>(&mut redis_conn)
            .await
        {
            if let Ok(results) = serde_json::from_str::<Vec<VideoFeedItem>>(&cached_json) {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                track_search_analytics(
                    &state.queue,
                    user_id,
                    &query,
                    results.len(),
                    true, // cache hit
                    duration_ms,
                )
                .await;
                return Ok(Json(results));
            }
        }
    }

    // 5. BUILD SQL with parameterized queries
    let sql = match params.sort.as_str() {
        "recent" => {
            r#"
            SELECT id, user_id, title, description, s3_bucket, s3_key, status, size_bytes, like_count, is_anonymous, created_at, updated_at
            FROM videos
            WHERE search_vector @@ plainto_tsquery('english', $1)
            AND deleted_at IS NULL
            AND status = 'PUBLISHED'
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#
        }
        "popular" => {
            r#"
            SELECT id, user_id, title, description, s3_bucket, s3_key, status, size_bytes, like_count, is_anonymous, created_at, updated_at
            FROM videos
            WHERE search_vector @@ plainto_tsquery('english', $1)
            AND deleted_at IS NULL
            AND status = 'PUBLISHED'
            ORDER BY like_count DESC, created_at DESC
            LIMIT $2 OFFSET $3
            "#
        }
        _ => {
            r#"
            SELECT id, user_id, title, description, s3_bucket, s3_key, status, size_bytes, like_count, is_anonymous, created_at, updated_at,
                   ts_rank(search_vector, plainto_tsquery('english', $1)) as rank
            FROM videos
            WHERE search_vector @@ plainto_tsquery('english', $1)
            AND deleted_at IS NULL
            AND status = 'PUBLISHED'
            ORDER BY rank DESC, like_count DESC
            LIMIT $2 OFFSET $3
            "#
        }
    };

    // 6. EXECUTE WITH TIMEOUT (resource exhaustion protection)
    let db_query = state.db.query_all(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        vec![
            query.clone().into(),
            (limit as i64).into(),
            (offset as i64).into(),
        ],
    ));

    let results = tokio::time::timeout(
        std::time::Duration::from_secs(search_config.timeout_secs),
        db_query,
    )
    .await
    .map_err(|_| {
        log_suspicious_query(user_id, &query, "query_timeout");
        ApiErrorResponse::internal_error("Search query timed out")
    })?
    .map_err(|e| {
        tracing::error!("Search query failed: {:?}", e);
        ApiErrorResponse::internal_error("Search failed")
    })?;

    // 7. CONVERT TO RESPONSE
    let mut feed = vec![];
    for row in results {
        let video_id: Uuid = row.try_get("", "id").map_err(|e| {
            tracing::error!("Failed to parse video ID: {:?}", e);
            ApiErrorResponse::internal_error("Failed to parse results")
        })?;

        let uploader_id: Uuid = row.try_get("", "user_id")?;
        let is_anonymous: bool = row.try_get("", "is_anonymous")?;

        // Fetch uploader username if not anonymous
        let uploader = if is_anonymous {
            None
        } else {
            shared::entities::users::Entity::find_by_id(uploader_id)
                .one(&state.db)
                .await
                .ok()
                .flatten()
                .map(|u| UploaderInfo {
                    id: u.id,
                    username: u.username,
                })
        };

        feed.push(VideoFeedItem {
            id: video_id,
            title: row.try_get("", "title")?,
            url: format!(
                "http://{}/videos-public/{}",
                state.config.minio_endpoint,
                row.try_get::<String>("", "s3_key")?
            ),
            like_count: row.try_get("", "like_count")?,
            created_at: row.try_get("", "created_at")?,
            uploader,
            is_liked: false,
        });
    }

    if !feed.is_empty() {
        let video_ids: Vec<Uuid> = feed.iter().map(|i| i.id).collect();
        let liked_video_ids: Vec<Uuid> = likes::Entity::find()
            .select_only()
            .column(likes::Column::VideoId)
            .filter(
                Condition::all()
                    .add(likes::Column::UserId.eq(user_id))
                    .add(likes::Column::VideoId.is_in(video_ids)),
            )
            .into_tuple()
            .all(&state.db)
            .await
            .map_err(|e| ApiErrorResponse::internal_error(e.to_string()))?;

        let liked_set: std::collections::HashSet<Uuid> = liked_video_ids.into_iter().collect();
        for item in &mut feed {
            item.is_liked = liked_set.contains(&item.id);
        }
    }

    // 8. CACHE RESULTS (user-scoped)
    if let Ok(mut conn) = state.queue.get_conn().await {
        if let Ok(json) = serde_json::to_string(&feed) {
            let _: Result<(), redis::RedisError> = redis::cmd("SETEX")
                .arg(&cache_key)
                .arg(search_config.cache_ttl_secs)
                .arg(json)
                .query_async(&mut conn)
                .await;
        }
    }

    // 9. ANALYTICS TRACKING
    let duration_ms = start_time.elapsed().as_millis() as u64;
    track_search_analytics(
        &state.queue,
        user_id,
        &query,
        feed.len(),
        false, // cache miss
        duration_ms,
    )
    .await;

    Ok(Json(feed))
}
/// POST /videos/bulk-delete
/// Bulk soft-delete videos owned by the authenticated user.
pub async fn bulk_delete_videos(
    State(state): State<AppState>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
    Json(payload): Json<BulkDeleteRequest>,
) -> ApiResult<()> {
    let token = auth.token();
    let claims = AuthService::validate_token(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    if state.token_revocation.is_revoked(claims.jti).await {
        return Err(ApiErrorResponse::unauthorized("Token revoked"));
    }
    let user_id = claims.sub;

    if payload.video_ids.is_empty() {
        return Ok(());
    }

    // Update videos: SET deleted_at = NOW() WHERE user_id = ? AND id IN (?)
    // SeaORM doesn't natively support bulk updates with WHERE IN efficiently in one struct call without filter.
    // We can use UpdateMany.

    videos::Entity::update_many()
        .col_expr(
            videos::Column::DeletedAt,
            sea_orm::sea_query::Expr::value(chrono::Utc::now().fixed_offset()),
        )
        .filter(videos::Column::UserId.eq(user_id))
        .filter(videos::Column::Id.is_in(payload.video_ids))
        .exec(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?;

    if let Ok(mut conn) = state.queue.get_conn().await {
        let pattern = format!("user:{}:videos:*", user_id);
        let keys: Vec<String> = conn.keys(&pattern).await.unwrap_or_default();
        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(keys).await;
        }
    }

    Ok(())
}
