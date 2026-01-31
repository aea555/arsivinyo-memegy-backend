use axum::{
    extract::{Query, State},
    Json,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use redis::AsyncCommands;
use sea_orm::*;
use serde::Deserialize;
use shared::entities::{users, videos};

use crate::{
    auth::{dtos::UserDto, service::AuthService},
    error::{ApiErrorResponse, ApiResult},
    state::AppState,
};

use super::dtos::UserVideoDto;

#[derive(Deserialize)]
pub struct PaginationQuery {
    pub page: Option<u64>,
    pub per_page: Option<u64>,
}

/// GET /users/me
/// Returns the authenticated user's profile.
/// Cached for 24 hours.
pub async fn get_me(
    State(state): State<AppState>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
) -> ApiResult<Json<UserDto>> {
    let token = auth.token();
    let claims = AuthService::get_claims_from_token(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    let user_id = claims.sub;
    let cache_key = format!("user:{}:profile", user_id);

    // Try cache first
    if let Ok(mut conn) = state.queue.get_conn().await {
        if let Ok(json) = conn.get::<_, String>(&cache_key).await {
            if let Ok(cached) = serde_json::from_str::<UserDto>(&json) {
                return Ok(Json(cached));
            }
        }
    }

    // Fetch from DB
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

    // Cache result
    if let Ok(mut conn) = state.queue.get_conn().await {
        if let Ok(json) = serde_json::to_string(&dto) {
            let _: Result<(), _> = conn.set_ex(&cache_key, json, 24 * 3600).await;
        }
    }

    Ok(Json(dto))
}

/// DELETE /users/me
/// Soft-deletes the user account and revokes all tokens.
pub async fn delete_account(
    State(state): State<AppState>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
) -> ApiResult<()> {
    let token = auth.token();
    let claims = AuthService::get_claims_from_token(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    let user_id = claims.sub;

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

    // 3. Invalidate Cache
    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), _> = conn.del(&format!("user:{}:profile", user_id)).await;
    }

    Ok(())
}

/// GET /users/me/videos
/// Returns a paginated list of videos uploaded by the user (excluding soft-deleted ones).
pub async fn get_my_videos(
    State(state): State<AppState>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<Vec<UserVideoDto>>> {
    let token = auth.token();
    let claims = AuthService::get_claims_from_token(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    let user_id = claims.sub;
    let page = pagination.page.unwrap_or(1).max(1);
    let per_page = pagination.per_page.unwrap_or(20).clamp(1, 100);

    // Cache key specific to user and pagination
    let cache_key = format!("user:{}:videos:{}:{}", user_id, page, per_page);

    // Try cache
    if let Ok(mut conn) = state.queue.get_conn().await {
        if let Ok(json) = conn.get::<_, String>(&cache_key).await {
            if let Ok(cached) = serde_json::from_str::<Vec<UserVideoDto>>(&json) {
                return Ok(Json(cached));
            }
        }
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

    let dtos: Vec<UserVideoDto> = items.into_iter().map(|v: videos::Model| v.into()).collect();

    // Cache result (short TTL, e.g., 5 mins, invalidated on upload/delete)
    if let Ok(mut conn) = state.queue.get_conn().await {
        if let Ok(json) = serde_json::to_string(&dtos) {
            let _: Result<(), _> = conn.set_ex(&cache_key, json, 300).await;
        }
    }

    Ok(Json(dtos))
}
