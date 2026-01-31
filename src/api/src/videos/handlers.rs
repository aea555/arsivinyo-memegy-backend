use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use sea_orm::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

use super::dtos::*;
use crate::{
    auth::extractors::AuthUser,
    error::{ApiErrorResponse, ApiResult},
    state::AppState,
};
use shared::{
    entities::{likes, videos},
    queue::VideoProcessJob,
};

#[derive(Deserialize)]
pub struct FeedQuery {
    pub sort: Option<String>, // "latest", "popular", "random"
    pub page: Option<u64>,
}

#[derive(Serialize)]
pub struct VideoFeedItem {
    pub id: Uuid,
    pub title: Option<String>,
    pub url: String, // Public URL
    pub like_count: i64,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploader: Option<UploaderInfo>, // None if video is anonymous
}

#[derive(Serialize)]
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

    let sort = query.sort.as_deref().unwrap_or("random");
    let page = query.page.unwrap_or(0);
    let page_size = state.config.feed_page_size;

    // Try cache first (skip for random to keep it truly random)
    if sort != "random" {
        if let Ok(Some(cached)) = state.feed_cache.get_feed(sort, None, page).await {
            let base_url = format!(
                "{}/{}",
                state.config.minio_endpoint, state.config.minio_bucket_videos
            );

            let items: Vec<VideoFeedItem> = cached
                .into_iter()
                .map(|c| VideoFeedItem {
                    id: c.id,
                    title: c.title,
                    url: format!("{}/{}", base_url, c.url.split('/').last().unwrap_or(&c.url)),
                    like_count: c.like_count,
                    created_at: c.created_at,
                    // Phase 5: Map uploader from cache (respecting anonymity)
                    uploader: if c.is_anonymous {
                        None
                    } else {
                        c.uploader.map(|u| UploaderInfo {
                            id: u.id,
                            username: u.username,
                        })
                    },
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
            select = select.order_by_desc(videos::Column::CreatedAt);
        }
        "popular" => {
            select = select.order_by_desc(videos::Column::LikeCount);
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
        state.config.minio_endpoint, state.config.minio_bucket_videos
    );

    let items: Vec<VideoFeedItem> = video_with_users
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
        })
        .collect();

    // Update cache is skipped - will be implemented in Phase 5 with user info support

    // Phase 5: Populate cache with full metadata
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

    // 3. Verify file exists in S3
    let exists = state
        .storage
        .file_exists(&state.config.minio_bucket_raw, &video.s3_key)
        .await
        .unwrap_or(false);

    if !exists {
        txn.rollback().await?;
        return Err(ApiErrorResponse::bad_request("File not uploaded yet"));
    }

    // 4. Increment rate limit (only after verified upload)
    let rate_key = crate::services::rate_limiter::RateLimiter::upload_bytes_key(&user_id);
    let _ = state
        .rate_limiter
        .increment(&rate_key, state.config.rate_limit_window_secs)
        .await;

    // 5. Update Status
    let mut active_video: videos::ActiveModel = video.clone().into();
    active_video.status = Set("PROCESSING".to_string());
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

    Ok(StatusCode::ACCEPTED)
}

pub async fn like_video(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    // Check if video exists and is published
    let _video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::Status.eq("PUBLISHED"))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found or not published"))?;

    // Try to insert like
    let like = likes::ActiveModel {
        user_id: Set(user_id),
        video_id: Set(video_id),
        created_at: Set(Utc::now().into()),
    };

    match like.insert(&state.db).await {
        Ok(_) => {
            // Increment like count
            videos::Entity::update_many()
                .col_expr(
                    videos::Column::LikeCount,
                    sea_orm::sea_query::Expr::col(videos::Column::LikeCount).add(1),
                )
                .filter(videos::Column::Id.eq(video_id))
                .exec(&state.db)
                .await?;

            Ok(StatusCode::CREATED)
        }
        Err(DbErr::RecordNotInserted) | Err(DbErr::Exec(_)) => {
            // Already liked (duplicate key violation)
            Ok(StatusCode::OK)
        }
        Err(e) => Err(e.into()),
    }
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
