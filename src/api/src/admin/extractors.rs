use std::net::IpAddr;

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};

use crate::{error::ApiErrorResponse, state::AppState};

use super::auth::verify_admin_jwt;
use crate::services::client_ip::extract_client_ip_from_headers_and_extensions;

#[derive(Debug, Clone)]
pub struct AdminPrincipal {
    pub sub: String,
    pub jti: String,
    pub exp: usize,
    pub client_ip: IpAddr,
}

#[async_trait]
impl FromRequestParts<AppState> for AdminPrincipal {
    type Rejection = ApiErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if !state.config.admin_api_enabled {
            return Err(ApiErrorResponse::new(
                StatusCode::NOT_FOUND,
                "Admin API is disabled",
            ));
        }

        let TypedHeader(Authorization(bearer)) =
            TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
                .await
                .map_err(|_| ApiErrorResponse::unauthorized("Missing admin bearer token"))?;

        let client_ip = extract_client_ip_from_headers_and_extensions(
            &parts.headers,
            &parts.extensions,
            &state.config.environment,
            state.config.require_cloudflare_headers,
        )?;
        let claims = verify_admin_jwt(state, bearer.token(), client_ip).await?;

        Ok(Self {
            sub: claims.sub,
            jti: claims.jti,
            exp: claims.exp,
            client_ip,
        })
    }
}
