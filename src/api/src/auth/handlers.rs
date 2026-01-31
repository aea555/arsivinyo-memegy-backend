use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect},
    Json,
};
use axum_extra::{
    extract::cookie::{Cookie, CookieJar, SameSite},
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope, TokenUrl,
};

use super::{dtos::*, service::AuthService};
use crate::{
    error::{ApiErrorResponse, ApiResult},
    state::AppState,
};

// Helper to build OAuth Client
fn oauth_client(state: &AppState) -> BasicClient {
    BasicClient::new(
        ClientId::new(state.config.google_client_id.clone()),
        Some(ClientSecret::new(state.config.google_client_secret.clone())),
        AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).unwrap(),
        Some(TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).unwrap()),
    )
    .set_redirect_uri(
        RedirectUrl::new(format!(
            "{}/auth/google/callback",
            state.config.oauth_redirect_base_url
        ))
        .expect("Invalid redirect URL"),
    )
}

pub async fn google_login(
    State(state): State<AppState>,
    jar: CookieJar,
) -> (CookieJar, impl IntoResponse) {
    let client = oauth_client(&state);
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .url();

    // Create CSRF cookie
    let mut cookie = Cookie::new("oauth_state", csrf_token.secret().to_string());
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax); // Allow redirect from Google
                                         // Set expiry (e.g., 10 minutes)
    cookie.set_max_age(time::Duration::minutes(10));

    (jar.add(cookie), Redirect::to(auth_url.as_str()))
}

pub async fn google_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<GoogleCallbackQuery>,
) -> ApiResult<(CookieJar, Json<AuthResponse>)> {
    // 1. Verify CSRF Token
    let stored_state = jar.get("oauth_state").map(|c| c.value().to_string());

    // Clear the cookie regardless of outcome
    let jar = jar.remove(Cookie::from("oauth_state"));

    match stored_state {
        Some(ref stored) if stored == &query.state => {
            // State matches, proceed
        }
        Some(_) => {
            tracing::error!("CSRF state mismatch");
            return Err(ApiErrorResponse::bad_request(
                "Invalid authentication state",
            ));
        }
        None => {
            tracing::error!("Missing CSRF state cookie");
            return Err(ApiErrorResponse::bad_request(
                "Authentication session expired",
            ));
        }
    }

    // Use the same redirect URI as in the authorization request
    let redirect_uri = format!(
        "{}/auth/google/callback",
        state.config.oauth_redirect_base_url
    );

    let google_user = AuthService::verify_google_code(
        query.code,
        &state.config.google_client_id,
        &state.config.google_client_secret,
        &redirect_uri,
    )
    .await
    .map_err(|e| {
        tracing::error!("Google OAuth verification failed: {:?}", e);
        ApiErrorResponse::unauthorized("Failed to verify Google authentication")
    })?;

    let (access_token, refresh_token, user) = AuthService::login_or_register(
        &state.db,
        google_user,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
        state.config.refresh_token_ttl_days,
    )
    .await
    .map_err(|e| {
        tracing::error!("Login/register failed: {:?}", e);
        ApiErrorResponse::internal_error("Failed to complete authentication")
    })?;

    Ok((
        jar,
        Json(AuthResponse {
            access_token,
            refresh_token,
            user: UserDto {
                id: user.id,
                username: user.username,
                email: user.email,
            },
        }),
    ))
}

pub async fn refresh_token(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> ApiResult<Json<RefreshResponse>> {
    let (new_access_token, new_refresh_token) = AuthService::refresh_access_token(
        &state.db,
        &state.token_revocation,
        &payload.access_token,
        &payload.refresh_token,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
        state.config.refresh_token_ttl_days,
    )
    .await
    .map_err(|_| ApiErrorResponse::unauthorized("Invalid or expired tokens"))?;

    Ok(Json(RefreshResponse {
        access_token: new_access_token,
        refresh_token: new_refresh_token,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
) -> ApiResult<()> {
    let token = auth.token();
    let claims = AuthService::get_claims_from_token(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    AuthService::logout_all(&state.db, claims.sub).await?;

    Ok(())
}

/// Development-only endpoint for testing without Google OAuth
/// Only works when RUST_LOG contains "debug"
pub async fn dev_login(
    State(state): State<AppState>,
    Json(payload): Json<DevLoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    // Only allow in development mode
    let log_level = std::env::var("RUST_LOG").unwrap_or_default();
    if !log_level.contains("debug") && !log_level.contains("trace") {
        return Err(ApiErrorResponse::not_found("Endpoint not available"));
    }

    use chrono::{Duration, Utc};
    use sea_orm::*;
    use shared::entities::refresh_tokens;
    use shared::entities::users;
    use shared::security::{create_access_token, generate_refresh_token, hash_token};
    use uuid::Uuid;

    // Create or find dev user
    let google_id = format!("dev_{}", payload.email);

    let user = users::Entity::find()
        .filter(users::Column::GoogleId.eq(&google_id))
        .one(&state.db)
        .await?;

    let user = match user {
        Some(u) => u,
        None => {
            let new_user = users::ActiveModel {
                id: Set(Uuid::new_v4()),
                google_id: Set(google_id),
                username: Set(payload.username),
                email: Set(payload.email),
                ..Default::default()
            };
            new_user.insert(&state.db).await?
        }
    };

    let access_token = create_access_token(
        user.id,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
    )?;
    let refresh_token = generate_refresh_token();
    let refresh_token_hash = hash_token(&refresh_token)?;

    let expires_at = Utc::now() + Duration::days(14);

    let rt_model = refresh_tokens::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user.id),
        token_hash: Set(refresh_token_hash),
        expires_at: Set(expires_at.into()),
        ..Default::default()
    };

    rt_model.insert(&state.db).await?;

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
