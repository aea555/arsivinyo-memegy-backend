use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use shared::{
    entities::extension_sessions,
    security::{verify_extension_jwt, verify_jwt},
};
use uuid::Uuid;

use crate::state::AppState;

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

#[async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Extract the Authorization header
        let TypedHeader(Authorization(bearer)) =
            TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
                .await
                .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let token = bearer.token();
        let claims =
            verify_jwt(token, &state.config.jwt_secret).map_err(|_| StatusCode::UNAUTHORIZED)?;

        // Check if token has been revoked
        if state.token_revocation.is_revoked(claims.jti).await {
            tracing::debug!("Rejected revoked token: {}", claims.jti);
            return Err(StatusCode::UNAUTHORIZED);
        }

        Ok(AuthUser(claims.sub))
    }
}

#[async_trait]
impl FromRequestParts<AppState> for ExtensionAuth {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(bearer)) =
            TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
                .await
                .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let claims = verify_extension_jwt(bearer.token(), &state.config.jwt_secret)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let session = extension_sessions::Entity::find_by_id(claims.jti)
            .filter(extension_sessions::Column::UserId.eq(claims.sub))
            .filter(extension_sessions::Column::RevokedAt.is_null())
            .one(&state.db)
            .await
            .map_err(|_| StatusCode::UNAUTHORIZED)?
            .ok_or(StatusCode::UNAUTHORIZED)?;

        if session.expires_at <= Utc::now() {
            return Err(StatusCode::UNAUTHORIZED);
        }

        Ok(Self {
            user_id: claims.sub,
            session_jti: claims.jti,
            platform: session.platform,
            device_id_hash: session.device_id_hash,
            scope: session.scope,
        })
    }
}
