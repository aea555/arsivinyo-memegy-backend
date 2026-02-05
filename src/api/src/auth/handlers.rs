use axum::{
    Json,
    extract::{Query, State},
    response::{IntoResponse, Redirect},
};
use axum_extra::{
    TypedHeader,
    extract::cookie::{Cookie, CookieJar, SameSite},
    headers::{Authorization, authorization::Bearer},
};
use oauth2::{
    AuthUrl, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope, TokenUrl, basic::BasicClient,
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
    Query(query): Query<GoogleLoginQuery>,
) -> (CookieJar, impl IntoResponse) {
    let client = oauth_client(&state);
    // Encode source into state: "csrf_token:source"
    let source = query.source.unwrap_or_else(|| "web".to_string());

    // Create CSRF token with embedded source
    let csrf_token = CsrfToken::new_random();
    let combined_state_str = format!("{}:{}", csrf_token.secret(), source);

    let (auth_url, _) = client
        .authorize_url(|| CsrfToken::new(combined_state_str.clone())) // Use closure that returns our custom token
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .url();

    let mut cookie = Cookie::new("oauth_state", combined_state_str);
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(time::Duration::minutes(10));

    (jar.add(cookie), Redirect::to(auth_url.as_str()))
}

pub async fn google_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<GoogleCallbackQuery>,
) -> ApiResult<impl IntoResponse> {
    // 1. Verify CSRF Token
    let stored_state = jar.get("oauth_state").map(|c| c.value().to_string());
    let jar = jar.remove(Cookie::from("oauth_state"));

    match stored_state {
        Some(ref stored) if stored == &query.state => {
            // State matches
        }
        _ => {
            tracing::error!("CSRF state mismatch or missing");
            return Err(ApiErrorResponse::bad_request(
                "Invalid authentication state",
            ));
        }
    }

    // Decode source from state
    let parts: Vec<&str> = query.state.split(':').collect();
    let source = if parts.len() >= 2 { parts[1] } else { "web" };

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

    let (access_token, refresh_token, _user) = AuthService::login_or_register(
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

    // Handle Redirect based on Source
    match source {
        "mobile" => {
            // Deep Link Redirect
            // e.g. memegy://auth/callback?access_token=...
            let redirect_url = format!(
                "{}auth/callback?access_token={}&refresh_token={}",
                state.config.mobile_app_scheme, access_token, refresh_token
            );
            Ok((
                jar,
                axum::response::Response::builder()
                    .status(302)
                    .header("Location", redirect_url)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
                .into_response())
        }
        "popup" => {
            // HTML PostMessage
            let html = format!(
                r#"
                <!DOCTYPE html>
                <html>
                <head><title>Authenticating...</title></head>
                <body>
                <script>
                    window.opener.postMessage({{
                        type: 'GOOGLE_AUTH_SUCCESS',
                        accessToken: '{}',
                        refreshToken: '{}'
                    }}, '*');
                    window.close();
                </script>
                </body>
                </html>
                "#,
                access_token, refresh_token
            );
            Ok((jar, axum::response::Html(html)).into_response())
        }
        _ => {
            // Default Web Redirect
            let redirect_url = format!(
                "{}/auth/callback?access_token={}&refresh_token={}",
                state.config.frontend_app_url, access_token, refresh_token
            );
            Ok((jar, Redirect::to(&redirect_url)).into_response())
        }
    }
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
    // Validate token (must be valid to logout, though lenient impls might allow invalid)
    let claims = AuthService::validate_token(token, &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    // 1. Revoke Access Token
    let now_secs = chrono::Utc::now().timestamp() as usize;
    if claims.exp > now_secs {
        state
            .token_revocation
            .revoke_token(claims.jti, claims.exp - now_secs)
            .await
            .ok(); // ignore errors
    }

    // 2. Revoke Refresh Tokens
    AuthService::logout_all(&state.db, claims.sub).await?;

    Ok(())
}

/// Development-only endpoint for testing without Google OAuth
/// Only works when RUST_LOG contains "debug"
pub async fn dev_login(
    State(state): State<AppState>,
    Json(payload): Json<DevLoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    // Only allow in development mode or test environment
    let log_level = std::env::var("RUST_LOG").unwrap_or_default();
    if !log_level.contains("debug")
        && !log_level.contains("trace")
        && state.config.environment != "test"
    {
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
            avatar_url: user.avatar_url,
        },
    }))
}
