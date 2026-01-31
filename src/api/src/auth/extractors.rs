use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use shared::security::verify_jwt;
use uuid::Uuid;

use crate::state::AppState;

pub struct AuthUser(pub Uuid);

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
