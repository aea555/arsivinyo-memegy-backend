use axum::{
    extract::{Path, State, Query},
    Json,
    http::StatusCode,
};
use redis::AsyncCommands;
use sea_orm::*;
use std::time::Duration;
use uuid::Uuid;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{state::AppState, auth::extractors::AuthUser, error::{ApiResult, ApiErrorResponse}};
use shared::{entities::{videos, likes}, queue::VideoProcessJob};
use super::dtos::*;

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
}

pub async fn get_feed(
    State(state): State<AppState>,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Json<Vec<VideoFeedItem>>> {
    let sort = query.sort.as_deref().unwrap_or("random");
    let page = query.page.unwrap_or(0);
    let page_size = 20;

    let mut select = videos::Entity::find()
        .filter(videos::Column::Status.eq("PUBLISHED"));

    match sort {
        "latest" => {
            select = select.order_by_desc(videos::Column::CreatedAt);
        }
        "popular" => {
            select = select.order_by_desc(videos::Column::LikeCount);
        }
        _ => {
            select = select.order_by(sea_orm::sea_query::Expr::cust("RANDOM()"), sea_orm::Order::Asc);
        }
    }

    let videos = select
        .paginate(&state.db, page_size)
        .fetch_page(page)
        .await?;

    let base_url = format!("{}/{}", state.config.minio_endpoint, state.config.minio_bucket_videos);

    let items = videos.into_iter().map(|v| VideoFeedItem {
        id: v.id,
        title: v.title,
        url: format!("{}/{}", base_url, v.s3_key),
        like_count: v.like_count,
        created_at: v.created_at,
    }).collect();

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
                 [video_id.into()]
             )).await?;
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
             [video_id.into()]
         )).await?;
    }

    txn.commit().await?;

    Ok(StatusCode::OK)
}

pub async fn init_upload(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(payload): Json<InitUploadRequest>,
) -> ApiResult<Json<InitUploadResponse>> {
    // 1. Rate Limiting Check
    let key = format!("rate_limit:upload:{}", user_id);
    let limit = state.config.limit_upload_bytes_hourly as i64;
    
    let mut conn = state.queue.get_conn().await
        .map_err(|e| ApiErrorResponse::internal_error(format!("Redis connection failed: {}", e)))?;
    
    // Check current usage
    let current_usage: i64 = conn.get(&key).await.unwrap_or(0);
    
    if current_usage + payload.size_bytes > limit {
        return Err(ApiErrorResponse::too_many_requests("Upload rate limit exceeded. Please try again later."));
    }

    // 2. Create DB Record (DRAFT)
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
        created_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    };

    new_video.insert(&state.db).await?;

    // 3. Generate Presigned URL
    let upload_url = state.storage
        .generate_presigned_put(&state.config.minio_bucket_raw, &s3_key, Duration::from_secs(3600))
        .await
        .map_err(|e| ApiErrorResponse::internal_error(format!("Failed to generate upload URL: {}", e)))?;

    // 4. Update Rate Limit
    let _: () = conn.incr(&key, payload.size_bytes).await.unwrap_or(());
    let ttl: i64 = conn.ttl(&key).await.unwrap_or(-1);
    if ttl == -1 {
         let _: () = conn.expire(&key, 3600).await.unwrap_or(());
    }

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
    // 1. Fetch Video
    let video = videos::Entity::find_by_id(video_id)
        .filter(videos::Column::UserId.eq(user_id))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiErrorResponse::not_found("Video not found"))?;

    if video.status != "DRAFT" {
        return Err(ApiErrorResponse::bad_request("Video is not in DRAFT status"));
    }

    // 2. Check S3
    let exists = state.storage
        .file_exists(&video.s3_bucket, &video.s3_key)
        .await
        .map_err(|e| ApiErrorResponse::internal_error(format!("Failed to check file: {}", e)))?;

    if !exists {
        return Err(ApiErrorResponse::bad_request("File not uploaded yet"));
    }

    // 3. Update Status
    let mut active_video: videos::ActiveModel = video.clone().into();
    active_video.status = Set("PROCESSING".to_string());
    active_video.updated_at = Set(Utc::now().into());
    active_video.update(&state.db).await?;

    // 4. Queue Job
    let job = VideoProcessJob {
        video_id,
        user_id,
        raw_bucket: video.s3_bucket,
        raw_key: video.s3_key,
    };

    state.queue.push_video_job(job).await
        .map_err(|e| ApiErrorResponse::internal_error(format!("Failed to queue job: {}", e)))?;

    Ok(StatusCode::ACCEPTED)
}
