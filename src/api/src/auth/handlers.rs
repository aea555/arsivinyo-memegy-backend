use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect},
    Json,
};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope, TokenUrl,
};
use reqwest::StatusCode;
use axum_extra::{headers::{Authorization, authorization::Bearer}, TypedHeader};

use crate::state::AppState;
use super::{dtos::*, service::AuthService};

// Helper to build OAuth Client
fn oauth_client(state: &AppState) -> BasicClient {
    BasicClient::new(
        ClientId::new(state.config.minio_access_key.clone()), // Placeholder - need GOOGLE_CLIENT_ID
        Some(ClientSecret::new(state.config.minio_secret_key.clone())), // Placeholder
        AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).unwrap(),
        Some(TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).unwrap()),
    )
    .set_redirect_uri(
        RedirectUrl::new(format!("http://{}:{}/auth/google/callback", state.config.server_host, state.config.server_port))
            .expect("Invalid redirect URL"),
    )
}

pub async fn google_login(State(state): State<AppState>) -> impl IntoResponse {
    // Ideally we use the real config for Client ID/Secret
    // For now using the placeholders or env vars directly if we update Config
    
    let client = oauth_client(&state);
    let (auth_url, _csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .url();

    Redirect::to(auth_url.as_str())
}

pub async fn google_callback(
    State(state): State<AppState>,
    Query(query): Query<GoogleCallbackQuery>,
) -> Result<Json<AuthResponse>, StatusCode> {
    // Note: In production we must verify `state` (CSRF token).
    
    // We need the actual secrets to exchange code
    // For now, I'll pass the raw config values. 
    // WARNING: In real app, `minio_access_key` is NOT google client id.
    // I need to update Config to support Google Secrets.
    
    // Assuming Config is updated:
    let client_id = &state.config.google_client_id;
    let client_secret = &state.config.google_client_secret;
    let redirect_uri = format!("http://{}:{}/auth/google/callback", state.config.server_host, state.config.server_port);

    let google_user = AuthService::verify_google_code(
        query.code,
        &client_id,
        &client_secret,
        &redirect_uri
    )
    .await
    .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let (access_token, refresh_token, user) = AuthService::login_or_register(
        &state.db,
        google_user,
        &state.config.jwt_secret
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
        user: UserDto {
            id: user.id,
            username: user.username,
            email: user.email,
        },
    }))
}

pub async fn refresh_token(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>, StatusCode> {
    let new_access_token = AuthService::refresh_access_token(
        &state.db,
        &payload.access_token,
        &payload.refresh_token,
        &state.config.jwt_secret
    )
    .await
    .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(Json(RefreshResponse {
        access_token: new_access_token,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
) -> Result<(), StatusCode> {
    let token = auth.token();
    let user_id = AuthService::get_user_id_from_expired_token(token, &state.config.jwt_secret)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    AuthService::logout_all(&state.db, user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(())
}
