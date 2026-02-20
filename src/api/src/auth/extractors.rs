use axum::{async_trait, extract::FromRequestParts, http::request::Parts};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use shared::{
    entities::{extension_sessions, users},
    security::{verify_extension_jwt, verify_jwt},
};
use uuid::Uuid;

use crate::{error::ApiErrorResponse, services::ban_service::BanEnforcementError, state::AppState};

pub struct SessionUser(pub Uuid);
pub struct AuthUser(pub Uuid);
pub struct ExtensionAuth {
    pub user_id: Uuid,
    pub session_jti: Uuid,
    pub platform: String,
    pub device_id_hash: String,
    pub scope: Vec<String>,
}

impl ExtensionAuth {
    pub fn has_scope(&self, required_scope: &str) -> bool {
        self.scope.iter().any(|s| s == required_scope)
    }
}

pub type AuthorizedUser = AuthUser;
pub type AuthorizedExtension = ExtensionAuth;

async fn extract_session_user_id(
    parts: &mut Parts,
    state: &AppState,
) -> Result<Uuid, ApiErrorResponse> {
    let TypedHeader(Authorization(bearer)) =
        TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiErrorResponse::unauthorized("Missing or invalid token"))?;

    let token = bearer.token();
    let claims = verify_jwt(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    if state.token_revocation.is_revoked(claims.jti).await {
        tracing::debug!("Rejected revoked token: {}", claims.jti);
        return Err(ApiErrorResponse::unauthorized("Token revoked"));
    }

    match state.ban_service.ensure_user_not_banned(claims.sub).await {
        Ok(()) => {}
        Err(BanEnforcementError::Banned) => {
            return Err(ApiErrorResponse::forbidden("user_banned"));
        }
        Err(BanEnforcementError::Unavailable) => {
            return Err(ApiErrorResponse::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ban_check_unavailable",
            ));
        }
    }

    Ok(claims.sub)
}

pub async fn ensure_user_onboarding_complete(
    state: &AppState,
    user_id: Uuid,
) -> Result<(), ApiErrorResponse> {
    match state.ban_service.ensure_user_not_banned(user_id).await {
        Ok(()) => {}
        Err(BanEnforcementError::Banned) => {
            return Err(ApiErrorResponse::forbidden("user_banned"));
        }
        Err(BanEnforcementError::Unavailable) => {
            return Err(ApiErrorResponse::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ban_check_unavailable",
            ));
        }
    }

    let user = users::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(ApiErrorResponse::db_error)?
        .ok_or_else(|| ApiErrorResponse::unauthorized("User not found"))?;

    if user.deleted_at.is_some() {
        return Err(ApiErrorResponse::unauthorized("Invalid token"));
    }

    let age_confirmed = user.age_confirmed_at.is_some();
    let terms_accepted = user.terms_accepted_at.is_some()
        && user.terms_accepted_version.as_deref()
            == Some(state.config.terms_current_version.as_str());

    if age_confirmed && terms_accepted {
        Ok(())
    } else {
        Err(ApiErrorResponse::forbidden("onboarding_required"))
    }
}

#[async_trait]
impl FromRequestParts<AppState> for SessionUser {
    type Rejection = ApiErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(SessionUser(extract_session_user_id(parts, state).await?))
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user_id = extract_session_user_id(parts, state).await?;
        ensure_user_onboarding_complete(state, user_id).await?;
        Ok(AuthUser(user_id))
    }
}

#[async_trait]
impl FromRequestParts<AppState> for ExtensionAuth {
    type Rejection = ApiErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(bearer)) =
            TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
                .await
                .map_err(|_| ApiErrorResponse::unauthorized("Missing or invalid token"))?;

        let claims = verify_extension_jwt(bearer.token(), &state.config.jwt_secret)
            .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

        let session = extension_sessions::Entity::find_by_id(claims.jti)
            .filter(extension_sessions::Column::UserId.eq(claims.sub))
            .filter(extension_sessions::Column::RevokedAt.is_null())
            .one(&state.db)
            .await
            .map_err(ApiErrorResponse::db_error)?
            .ok_or_else(|| ApiErrorResponse::unauthorized("Invalid session"))?;

        if session.expires_at <= Utc::now() {
            return Err(ApiErrorResponse::unauthorized("Session expired"));
        }

        match state.ban_service.ensure_user_not_banned(claims.sub).await {
            Ok(()) => {}
            Err(BanEnforcementError::Banned) => {
                return Err(ApiErrorResponse::forbidden("user_banned"));
            }
            Err(BanEnforcementError::Unavailable) => {
                return Err(ApiErrorResponse::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "ban_check_unavailable",
                ));
            }
        }

        ensure_user_onboarding_complete(state, claims.sub).await?;

        Ok(Self {
            user_id: claims.sub,
            session_jti: claims.jti,
            platform: session.platform,
            device_id_hash: session.device_id_hash,
            scope: session.scope,
        })
    }
}
