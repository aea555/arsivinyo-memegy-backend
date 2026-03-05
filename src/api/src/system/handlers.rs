use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use shared::config::{TERMS_LANGUAGE_EN, TERMS_LANGUAGE_TR};

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
    pub language: String,
    pub version: String,
    pub url: Option<String>,
    pub content_type: Option<String>,
    pub content_sha256: Option<String>,
    pub content: Option<String>,
    pub effective_at: Option<String>,
    pub jurisdictions: Vec<String>,
    pub legal_contact_email: Option<String>,
    pub abuse_contact_email: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TermsBundleResponse {
    pub version: String,
    pub default_language: String,
    pub available_languages: Vec<String>,
    pub documents: TermsBundleDocuments,
}

#[derive(Debug, Serialize)]
pub struct TermsBundleDocuments {
    pub en: TermsResponse,
    pub tr: TermsResponse,
}

fn if_none_match_matches(headers: &HeaderMap, expected_etag: &str) -> bool {
    let Some(raw) = headers.get(axum::http::header::IF_NONE_MATCH) else {
        return false;
    };
    let Ok(value) = raw.to_str() else {
        return false;
    };
    value
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate == expected_etag)
}

fn cacheable_json_response<T: Serialize>(
    body: &T,
    etag: &str,
    request_headers: &HeaderMap,
) -> ApiResult<axum::response::Response> {
    let cache_control = HeaderValue::from_static("public, max-age=300, stale-while-revalidate=300");
    if if_none_match_matches(request_headers, etag) {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (
                    axum::http::header::ETAG,
                    HeaderValue::from_str(etag)
                        .map_err(|_| ApiErrorResponse::internal_error("Invalid ETag"))?,
                ),
                (axum::http::header::CACHE_CONTROL, cache_control),
            ],
        )
            .into_response());
    }

    Ok((
        StatusCode::OK,
        [
            (
                axum::http::header::ETAG,
                HeaderValue::from_str(etag)
                    .map_err(|_| ApiErrorResponse::internal_error("Invalid ETag"))?,
            ),
            (axum::http::header::CACHE_CONTROL, cache_control),
        ],
        Json(body),
    )
        .into_response())
}

pub(crate) fn build_terms_response_for_language(
    state: &AppState,
    language: &str,
) -> ApiResult<TermsResponse> {
    let document = state
        .config
        .get_terms_document(language)
        .ok_or_else(|| ApiErrorResponse::not_found("Terms language not found"))?;
    Ok(TermsResponse {
        language: document.language.clone(),
        version: document.version.clone(),
        url: document.url.clone(),
        content_type: document.content_type.clone(),
        content_sha256: document.content_sha256.clone(),
        content: document.content.clone(),
        effective_at: document.effective_at.clone(),
        jurisdictions: document.jurisdictions.clone(),
        legal_contact_email: state.config.terms_legal_contact_email.clone(),
        abuse_contact_email: state.config.terms_abuse_contact_email.clone(),
    })
}

pub async fn get_terms(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    let response = TermsBundleResponse {
        version: state.config.terms_current_version.clone(),
        default_language: state.config.terms_default_language.clone(),
        available_languages: vec![TERMS_LANGUAGE_EN.to_string(), TERMS_LANGUAGE_TR.to_string()],
        documents: TermsBundleDocuments {
            en: build_terms_response_for_language(&state, TERMS_LANGUAGE_EN)?,
            tr: build_terms_response_for_language(&state, TERMS_LANGUAGE_TR)?,
        },
    };
    let etag = format!("\"{}\"", state.config.terms_bundle_content_sha256);
    cacheable_json_response(&response, &etag, &headers)
}

async fn get_terms_for_language(
    state: AppState,
    language: &str,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    let response = build_terms_response_for_language(&state, language)?;
    let etag_seed = if let Some(hash) = response.content_sha256.clone() {
        hash
    } else {
        let serialized = serde_json::to_string(&response)
            .map_err(|_| ApiErrorResponse::internal_error("Failed to serialize terms response"))?;
        shared::config::Config::sha256_hex(&serialized)
    };
    let etag = format!("\"{}\"", etag_seed);
    cacheable_json_response(&response, &etag, &headers)
}

pub async fn get_terms_en(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    get_terms_for_language(state, TERMS_LANGUAGE_EN, headers).await
}

pub async fn get_terms_tr(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    get_terms_for_language(state, TERMS_LANGUAGE_TR, headers).await
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
