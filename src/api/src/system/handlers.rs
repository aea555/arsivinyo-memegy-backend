use axum::{Json, extract::State};
use serde::Serialize;

use crate::{
    auth::extractors::SessionUser,
    error::{ApiErrorResponse, ApiResult},
    services::rate_limiter::RateLimiter,
    state::AppState,
};

#[derive(Debug, Serialize)]
pub struct ModeStatusResponse {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct TermsResponse {
    pub version: String,
    pub url: Option<String>,
    pub content_type: Option<String>,
    pub content_sha256: Option<String>,
    pub content: Option<String>,
}

pub async fn get_terms(State(state): State<AppState>) -> ApiResult<Json<TermsResponse>> {
    Ok(Json(TermsResponse {
        version: state.config.terms_current_version.clone(),
        url: state.config.terms_url.clone(),
        content_type: state.config.terms_content_type.clone(),
        content_sha256: state.config.terms_content_sha256.clone(),
        content: state.config.terms_content.clone(),
    }))
}

pub async fn get_read_only_status(
    State(state): State<AppState>,
    SessionUser(user_id): SessionUser,
) -> ApiResult<Json<ModeStatusResponse>> {
    let key = RateLimiter::read_only_status_user_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(&key, state.config.read_only_status_rpm_per_user, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            return Err(ApiErrorResponse::too_many_requests(
                "Too many read-only status requests",
            ));
        }
        Err(_) => {
            return Err(ApiErrorResponse::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Read-only status temporarily unavailable",
            ));
        }
    }

    Ok(Json(ModeStatusResponse {
        enabled: state.config.read_only_mode_enabled,
    }))
}

pub async fn get_maintenance_status(
    State(state): State<AppState>,
    SessionUser(user_id): SessionUser,
) -> ApiResult<Json<ModeStatusResponse>> {
    let key = RateLimiter::maintenance_status_user_key(&user_id);
    match state
        .rate_limiter
        .check_and_increment(&key, state.config.maintenance_status_rpm_per_user, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            return Err(ApiErrorResponse::too_many_requests(
                "Too many maintenance status requests",
            ));
        }
        Err(_) => {
            return Err(ApiErrorResponse::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Maintenance status temporarily unavailable",
            ));
        }
    }

    Ok(Json(ModeStatusResponse {
        enabled: state.config.maintenance_mode_enabled,
    }))
}
