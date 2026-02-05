use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use axum_extra::extract::cookie::Cookie;

use crate::{
    audit::logger::{AuditEvent, log_audit_event},
    auth::extractors::AuthUser,
    error::{ApiErrorResponse, ApiResult},
    metrics::*,
    state::AppState,
};
use axum::response::Redirect;
use axum_extra::extract::cookie::{CookieJar, SameSite};
use chrono::{Duration, Utc};
use oauth2::{
    AuthType, AuthUrl, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope, TokenUrl,
    basic::BasicClient,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use shared::entities::{refresh_tokens, users};
use shared::security::{create_access_token, generate_refresh_token, hash_token};
use uuid::Uuid;

use super::constants::*;
use super::dtos::*;
use super::service::AuthService;

#[derive(Debug)]
struct OAuthState {
    source: String,
    code_challenge: Option<String>,
}

fn parse_oauth_state(state: &str) -> Result<OAuthState, &'static str> {
    let parts: Vec<&str> = state.split(':').collect();

    // Format: csrf:source[:code_challenge]
    // We validate the full state string before parsing, so we only extract what we need
    if parts.len() < 2 {
        return Err("Invalid state format");
    }

    Ok(OAuthState {
        source: parts[1].to_string(),
        code_challenge: parts.get(2).map(|s| s.to_string()),
    })
}

fn oauth_client(state: &AppState) -> BasicClient {
    let google_client_id = ClientId::new(state.config.google_client_id.clone());
    let google_client_secret = ClientSecret::new(state.config.google_client_secret.clone());
    let auth_url = AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())
        .expect("Invalid authorization endpoint URL");
    let token_url = TokenUrl::new("https://oauth2.googleapis.com/token".to_string())
        .expect("Invalid token endpoint URL");

    let redirect_url = format!(
        "{}/auth/google/callback",
        state.config.oauth_redirect_base_url
    );

    BasicClient::new(
        google_client_id,
        Some(google_client_secret),
        auth_url,
        Some(token_url),
    )
    .set_auth_type(AuthType::RequestBody)
    .set_redirect_uri(RedirectUrl::new(redirect_url).expect("Invalid redirect URL"))
}

pub async fn google_login(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<GoogleLoginQuery>,
) -> Result<(CookieJar, impl IntoResponse), ApiErrorResponse> {
    OAUTH_REQUESTS_TOTAL.inc();

    let client = oauth_client(&state);
    let source = query.source.as_deref().unwrap_or(SOURCE_WEB);

    // PKCE Validation
    let code_challenge = if let Some(challenge) = query.code_challenge {
        // Validate method is S256
        match query.code_challenge_method.as_deref() {
            Some(method) if method == PKCE_METHOD_S256 => {
                // RFC 7636: base64url(SHA256) = 43 characters
                if challenge.is_empty() || challenge.len() != 43 {
                    return Err(ApiErrorResponse::bad_request("Invalid code_challenge"));
                }

                // Validate base64url characters only (A-Z, a-z, 0-9, -, _)
                if !challenge
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(ApiErrorResponse::bad_request(
                        "Invalid code_challenge format",
                    ));
                }

                // Ensure challenge is base64url (no padding, no +/)
                if challenge.contains('+') || challenge.contains('/') || challenge.contains('=') {
                    return Err(ApiErrorResponse::bad_request(
                        "code_challenge must be base64url encoded",
                    ));
                }

                Some(challenge)
            }
            Some(method) if method == PKCE_METHOD_PLAIN => {
                return Err(ApiErrorResponse::bad_request(
                    "PKCE plain method not allowed, use S256",
                ));
            }
            _ => {
                return Err(ApiErrorResponse::bad_request(
                    "code_challenge_method must be S256",
                ));
            }
        }
    } else {
        // Require PKCE for mobile
        if source == SOURCE_MOBILE {
            return Err(ApiErrorResponse::bad_request(
                "PKCE required for mobile flows: code_challenge and code_challenge_method=S256",
            ));
        }
        None
    };

    // Audit log: OAuth login initiated
    log_audit_event(AuditEvent::OAuthLoginInitiated {
        source: source.to_string(),
        has_pkce: code_challenge.is_some(),
        timestamp: Utc::now(),
    });

    // Build state parameter: "csrf:source[:challenge]"
    let csrf = CsrfToken::new_random();
    let combined_state_str = if let Some(challenge) = code_challenge {
        format!("{}:{}:{}", csrf.secret(), source, challenge)
    } else {
        format!("{}:{}", csrf.secret(), source)
    };

    let (auth_url, _) = client
        .authorize_url(|| CsrfToken::new(combined_state_str.clone()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .url();

    let mut cookie = Cookie::new("oauth_state", combined_state_str);
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(time::Duration::minutes(10));

    Ok((jar.add(cookie), Redirect::to(auth_url.as_str())))
}

pub async fn google_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<GoogleCallbackQuery>,
) -> ApiResult<impl IntoResponse> {
    OAUTH_CALLBACKS_TOTAL.inc();

    // 1. Validate CSRF
    let stored_state_str = jar
        .get("oauth_state")
        .ok_or_else(|| ApiErrorResponse::bad_request("Missing state cookie"))?
        .value()
        .to_string();

    let jar = jar.remove(Cookie::from("oauth_state"));

    if stored_state_str != query.state {
        tracing::error!("CSRF mismatch");
        log_audit_event(AuditEvent::OAuthCallbackFailure {
            error: "CSRF mismatch".to_string(),
            source: None,
            timestamp: Utc::now(),
        });
        return Err(ApiErrorResponse::bad_request("Invalid state"));
    }

    // 2. Parse state
    let oauth_state = parse_oauth_state(&stored_state_str).map_err(|e| {
        tracing::error!("State parse error: {}", e);
        log_audit_event(AuditEvent::OAuthCallbackFailure {
            error: format!("State parse error: {}", e),
            source: None,
            timestamp: Utc::now(),
        });
        ApiErrorResponse::bad_request("Malformed state")
    })?;

    // 3. PKCE Validation
    if let Some(ref expected_challenge) = oauth_state.code_challenge {
        PKCE_VALIDATIONS_TOTAL.inc();

        let verifier = query.code_verifier.ok_or_else(|| {
            tracing::error!("Missing code_verifier for PKCE flow");
            log_audit_event(AuditEvent::PkceValidationFailed {
                source: oauth_state.source.clone(),
                timestamp: Utc::now(),
            });
            PKCE_FAILURES_TOTAL.inc();
            ApiErrorResponse::bad_request("Invalid OAuth state")
        })?;

        use shared::security::verify_pkce_challenge;

        if !verify_pkce_challenge(&verifier, &expected_challenge) {
            tracing::error!("PKCE validation failed for user flow. Challenge mismatch.");
            log_audit_event(AuditEvent::PkceValidationFailed {
                source: oauth_state.source.clone(),
                timestamp: Utc::now(),
            });
            PKCE_FAILURES_TOTAL.inc();
            return Err(ApiErrorResponse::unauthorized("Authentication failed"));
        }

        tracing::info!("PKCE validation successful");
    }

    // 4. Use the same redirect URI as in the authorization request
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

    // Capture pkce_validated flag before oauth_state is consumed
    let pkce_validated = oauth_state.code_challenge.is_some();

    // Audit log: OAuth callback success
    log_audit_event(AuditEvent::OAuthCallbackSuccess {
        user_id: user.id,
        source: oauth_state.source.clone(),
        pkce_validated,
        timestamp: Utc::now(),
    });

    // Generate OTC and store tokens
    use crate::cache::otc_cache::OtcTokenData;
    use shared::security::generate_otc;

    let otc = generate_otc();
    let token_data = OtcTokenData {
        access_token,
        refresh_token,
        user_id: user.id,
        username: user.username.clone(),
        email: user.email.clone(),
        avatar_url: user.avatar_url.clone(),
    };

    state
        .otc_cache
        .store_otc(&otc, &token_data)
        .await
        .map_err(|e| {
            tracing::error!("Failed to store OTC: {:?}", e);
            ApiErrorResponse::internal_error("Failed to complete authentication")
        })?;

    // Handle Redirect based on Source
    match oauth_state.source.as_str() {
        s if s == SOURCE_MOBILE => {
            // Deep Link Redirect with OTC
            let redirect_url = format!(
                "{}auth/callback?code={}",
                state.config.mobile_app_scheme, otc
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
        s if s == SOURCE_POPUP => {
            // HTML PostMessage with specific origin
            let html = format!(
                r#"
                <!DOCTYPE html>
                <html>
                <head><title>Authenticating...</title></head>
                <body>
                <script>
                    window.opener.postMessage({{
                        type: 'GOOGLE_AUTH_SUCCESS',
                        code: '{}'
                    }}, '{}');
                    window.close();
                </script>
                </body>
                </html>
                "#,
                otc, state.config.frontend_app_url
            );
            Ok((jar, axum::response::Html(html)).into_response())
        }
        _ => {
            // Default Web Redirect with OTC
            let redirect_url = format!(
                "{}/auth/callback?code={}",
                state.config.frontend_app_url, otc
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
    .await?;

    Ok(Json(RefreshResponse {
        access_token: new_access_token,
        refresh_token: new_refresh_token,
    }))
}

pub async fn logout(State(state): State<AppState>, AuthUser(user_id): AuthUser) -> ApiResult<()> {
    AuthService::logout_all(&state.db, user_id).await?;
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
    if !log_level.contains("debug") && std::env::var("APP_ENV").unwrap_or_default() != "test" {
        return Err(ApiErrorResponse::forbidden(
            "Dev login is only available in debug mode",
        ));
    }

    // Generate a fake Google ID for dev user
    let google_id = format!("dev_{}", uuid::Uuid::new_v4());

    // Find or create user
    let user = users::Entity::find()
        .filter(users::Column::Email.eq(&payload.email))
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

    // Store refresh token in database
    let expires_at = Utc::now() + Duration::days(state.config.refresh_token_ttl_days as i64);
    let rt_model = refresh_tokens::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user.id),
        token_hash: Set(refresh_token_hash),
        expires_at: Set(expires_at.into()),
        ..Default::default()
    };
    rt_model.insert(&state.db).await?;

    let response = AuthResponse {
        access_token,
        refresh_token,
        user: UserDto {
            id: user.id,
            username: user.username,
            email: user.email,
            avatar_url: user.avatar_url,
        },
    };

    Ok(Json(response))
}

/// Exchange one-time code for access/refresh tokens
pub async fn exchange_otc(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    Json(payload): Json<ExchangeOtcRequest>,
) -> ApiResult<Json<ExchangeOtcResponse>> {
    let client_ip = addr.ip().to_string();
    let timer = OTC_EXCHANGE_DURATION.start_timer();

    // Rate limiting
    state
        .otc_rate_limiter
        .check_rate_limit(&client_ip)
        .await
        .map_err(|_| {
            tracing::warn!("Rate limit exceeded for IP: {}", client_ip);
            log_audit_event(AuditEvent::RateLimitExceeded {
                client_ip: client_ip.clone(),
                endpoint: "/auth/exchange-otc".to_string(),
                timestamp: Utc::now(),
            });
            RATE_LIMIT_EXCEEDED_TOTAL.inc();
            ApiErrorResponse::too_many_requests("Too many attempts, please try again later")
        })?;

    // Consume OTC (get and delete atomically)
    let token_data = state
        .otc_cache
        .consume_otc(&payload.code)
        .await
        .map_err(|e| {
            tracing::error!("Failed to retrieve OTC: {:?}", e);
            log_audit_event(AuditEvent::OtcExchangeFailure {
                client_ip: client_ip.clone(),
                error: "Failed to retrieve OTC".to_string(),
                timestamp: Utc::now(),
            });
            ApiErrorResponse::internal_error("Failed to exchange code")
        })?
        .ok_or_else(|| {
            tracing::warn!("Invalid or expired OTC: {}", payload.code);
            log_audit_event(AuditEvent::OtcExchangeFailure {
                client_ip: client_ip.clone(),
                error: "Invalid or expired OTC".to_string(),
                timestamp: Utc::now(),
            });
            ApiErrorResponse::bad_request("Invalid or expired code")
        })?;

    // Audit log: OTC exchange success
    log_audit_event(AuditEvent::OtcExchangeSuccess {
        user_id: token_data.user_id,
        client_ip,
        timestamp: Utc::now(),
    });

    OTC_EXCHANGES_TOTAL.inc();
    timer.observe_duration();

    let response = ExchangeOtcResponse {
        access_token: token_data.access_token,
        refresh_token: token_data.refresh_token,
        user: UserDto {
            id: token_data.user_id,
            username: token_data.username,
            email: token_data.email,
            avatar_url: token_data.avatar_url,
        },
    };

    Ok(Json(response))
}
