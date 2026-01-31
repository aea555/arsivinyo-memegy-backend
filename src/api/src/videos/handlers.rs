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
        Ok(Ok(_remaining)) => {} // Within limit
        Ok(Err(count)) => {
            return Err(ApiErrorResponse::too_many_requests(format!(
                "Feed rate limit exceeded ({} requests/minute)",
                count
            )));
        }
        Err(_) => {} // Redis error, fail open
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
                    uploader: None, // TODO: Cache doesn't store user info yet, will be added in Phase 5
                })
                .collect();

            return Ok(Json(items));
        }
    }

    // Cache miss or random - query DB with user join for non-anonymous videos
    use shared::entities::users;

    let mut select = videos::Entity::find()
        .filter(videos::Column::Status.eq("PUBLISHED"))
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

    // Update cache (skip random)
    if sort != "random" && !video_with_users.is_empty() {
        let cached_items: Vec<_> = video_with_users
            .into_iter()
            .map(|(v, _)| crate::cache::feed_cache::CachedVideoFeedItem {
                id: v.id,
                title: v.title,
                url: v.s3_key,
                thumbnail_url: None,
                like_count: v.like_count,
                created_at: v.created_at,
            })
            .collect();

        let _ = state
            .feed_cache
            .set_feed(
                sort,
                None,
                page,
                &cached_items,
                state.config.feed_cache_ttl_secs,
            )
            .await;
    }

    Ok(Json(items))
}

pub async fn like_video(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(video_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    // Transaction to ensure consistency
    let txn = state.db.begin().await?;

    // Check if already liked
    let existing = likes::Entity::find()
        .filter(likes::Column::UserId.eq(user_id))
        .filter(likes::Column::VideoId.eq(video_id))
        .one(&txn)
        .await?;

    if let Some(like) = existing {
        // Unlike
        like.delete(&txn).await?;

        // Decrement count atomically
        if let Some(_v) = videos::Entity::find_by_id(video_id).one(&txn).await? {
            txn.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE "videos" SET "like_count" = "like_count" - 1 WHERE "id" = $1"#,
                [video_id.into()],
            ))
            .await?;
        }
    } else {
        // Like
        let new_like = likes::ActiveModel {
            user_id: Set(user_id),
            video_id: Set(video_id),
            created_at: Set(Utc::now().into()),
        };
        new_like.insert(&txn).await?;

        txn.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE "videos" SET "like_count" = "like_count" + 1 WHERE "id" = $1"#,
            [video_id.into()],
        ))
        .await?;
    }

    txn.commit().await?;

    Ok(StatusCode::OK)
}

pub async fn init_upload(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(payload): Json<InitUploadRequest>,
) -> ApiResult<Json<InitUploadResponse>> {
    // 1. Validate file size
    if payload.size_bytes > state.config.max_file_size_bytes {
        return Err(ApiErrorResponse::bad_request(format!(
            "File size exceeds maximum allowed size of {} bytes",
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
    // Use transaction with SELECT FOR UPDATE for idempotency
    let txn = state.db.begin().await?;

    // 1. Fetch Video with lock to prevent concurrent confirmations
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::UserId.eq(user_id))
        .lock_exclusive() // SELECT FOR UPDATE
        .one(&txn)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found"))?;

    // 2. Idempotency check - if already processed, return success
    if video.status != "DRAFT" {
        txn.commit().await?;
        // If already PROCESSING or PUBLISHED, return 202 (idempotent)
        // If FAILED, return error
        return if video.status == "FAILED" {
            Err(ApiErrorResponse::bad_request(
                "Video processing failed previously",
            ))
        } else {
            Ok(StatusCode::ACCEPTED)
        };
    }

    // 3. Check S3
    let exists = state
        .storage
        .file_exists(&video.s3_bucket, &video.s3_key)
        .await
        .map_err(|e| ApiErrorResponse::internal_error(format!("Failed to check file: {}", e)))?;

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
