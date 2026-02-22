use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::cookie::Cookie;
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};

use crate::{
    audit::logger::{AuditEvent, log_audit_event},
    auth::extractors::AuthUser,
    cache::otc_cache::{OtcAuthTokenData, OtcSignupRequiredData, OtcTokenData},
    error::{ApiErrorResponse, ApiResult},
    metrics::*,
    services::{ban_service::BanEnforcementError, client_ip::ClientIp},
    state::AppState,
    users::username::{
        USERNAME_MAX_LEN, USERNAME_MIN_LEN, USERNAME_PATTERN, suggest_username_from_google_name,
        validate_username,
    },
};
use axum::response::Redirect;
use axum_extra::extract::cookie::{CookieJar, SameSite};
use chrono::{Duration, Utc};
use oauth2::{
    AuthType, AuthUrl, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenUrl, basic::BasicClient,
};
use redis::AsyncCommands;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use shared::entities::{extension_sessions, refresh_tokens, users};
use shared::security::{
    compute_code_challenge, create_access_token, create_extension_access_token, generate_otc,
    generate_refresh_token, hash_token, validate_code_verifier,
};
use tokio::time::{Duration as TokioDuration, sleep};
use uuid::Uuid;

use super::constants::*;
use super::dtos::*;
use super::service::{
    AuthService, RefreshAccessTokenError, RegisterUserError, RegisterWithUsernameInput,
};

#[derive(Debug)]
struct OAuthState {
    source: String,
    code_challenge: Option<String>,
}

const SIGNUP_TICKET_TTL_SECS: usize = 600;
const SIGNUP_RESULT_TTL_SECS: usize = 600;
const SIGNUP_LOCK_TTL_SECS: usize = 15;
const SIGNUP_LOCK_POLL_RETRIES: usize = 8;

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
struct SignupTicketData {
    google_id: String,
    email: String,
    avatar_url: String,
    source: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn signup_ticket_key(ticket: &str) -> String {
    format!("signup:ticket:{}", ticket)
}

fn signup_result_key(ticket: &str) -> String {
    format!("signup:result:{}", ticket)
}

fn signup_lock_key(ticket: &str) -> String {
    format!("signup:lock:{}", ticket)
}

async fn read_signup_result(state: &AppState, signup_ticket: &str) -> Option<AuthResponse> {
    let key = signup_result_key(signup_ticket);
    let mut conn = state.queue.get_conn().await.ok()?;
    let cached: Option<String> = conn.get(key).await.ok()?;
    cached.and_then(|json| serde_json::from_str::<AuthResponse>(&json).ok())
}

async fn write_signup_result(
    state: &AppState,
    signup_ticket: &str,
    response: &AuthResponse,
) -> anyhow::Result<()> {
    let key = signup_result_key(signup_ticket);
    let json = serde_json::to_string(response)?;
    let mut conn = state.queue.get_conn().await?;
    let _: () = redis::cmd("SET")
        .arg(&key)
        .arg(&json)
        .arg("EX")
        .arg(SIGNUP_RESULT_TTL_SECS)
        .query_async(&mut conn)
        .await?;
    Ok(())
}

async fn read_signup_ticket(state: &AppState, signup_ticket: &str) -> Option<SignupTicketData> {
    let key = signup_ticket_key(signup_ticket);
    let mut conn = state.queue.get_conn().await.ok()?;
    let raw: Option<String> = conn.get(key).await.ok()?;
    raw.and_then(|json| serde_json::from_str::<SignupTicketData>(&json).ok())
}

async fn write_signup_ticket(
    state: &AppState,
    signup_ticket: &str,
    data: &SignupTicketData,
) -> anyhow::Result<()> {
    let key = signup_ticket_key(signup_ticket);
    let json = serde_json::to_string(data)?;
    let mut conn = state.queue.get_conn().await?;
    let _: () = redis::cmd("SET")
        .arg(&key)
        .arg(&json)
        .arg("EX")
        .arg(SIGNUP_TICKET_TTL_SECS)
        .query_async(&mut conn)
        .await?;
    Ok(())
}

async fn delete_signup_ticket(state: &AppState, signup_ticket: &str) {
    let key = signup_ticket_key(signup_ticket);
    if let Ok(mut conn) = state.queue.get_conn().await {
        let _: Result<(), _> = conn.del(key).await;
    }
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

fn normalize_extension_scopes(
    requested_scopes: Option<Vec<String>>,
) -> Result<Vec<String>, ApiErrorResponse> {
    let allowed = ["keyboard.search", "keyboard.send"];
    let mut scopes = requested_scopes
        .unwrap_or_else(|| vec!["keyboard.search".to_string(), "keyboard.send".to_string()]);

    scopes.sort();
    scopes.dedup();

    if scopes.is_empty() {
        return Err(ApiErrorResponse::bad_request(
            "requested_scopes cannot be empty",
        ));
    }

    for scope in &scopes {
        if !allowed.contains(&scope.as_str()) {
            return Err(ApiErrorResponse::bad_request(format!(
                "Invalid scope requested: {}",
                scope
            )));
        }
    }

    Ok(scopes)
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
    ClientIp(client_ip): ClientIp,
    jar: CookieJar,
    Query(query): Query<GoogleLoginQuery>,
) -> Result<(CookieJar, impl IntoResponse), ApiErrorResponse> {
    OAUTH_REQUESTS_TOTAL.inc();

    let client = oauth_client(&state);
    let source = query.source.as_deref().unwrap_or(SOURCE_WEB);

    // PKCE Validation
    let mut code_challenge: Option<String> = None;
    let mut code_verifier: Option<String> = None;

    if query.code_challenge.is_some() || query.code_verifier.is_some() {
        match query.code_challenge_method.as_deref() {
            Some(method) if method == PKCE_METHOD_S256 => {}
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
    }

    if source == SOURCE_MOBILE && query.code_verifier.is_none() {
        return Err(ApiErrorResponse::bad_request(
            "PKCE required for mobile flows: code_verifier and code_challenge_method=S256",
        ));
    }

    if let Some(verifier) = query.code_verifier.clone() {
        if !validate_code_verifier(&verifier) {
            return Err(ApiErrorResponse::bad_request("Invalid code_verifier"));
        }

        let computed_challenge = compute_code_challenge(&verifier);
        code_challenge = Some(computed_challenge.clone());
        code_verifier = Some(verifier);

        if let Some(challenge) = query.code_challenge.clone() {
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

            if challenge != computed_challenge {
                return Err(ApiErrorResponse::bad_request(
                    "code_challenge does not match code_verifier",
                ));
            }
        }
    } else {
        if query.code_challenge.is_some() {
            return Err(ApiErrorResponse::bad_request(
                "code_verifier required when code_challenge is provided",
            ));
        }
        if source == SOURCE_MOBILE {
            return Err(ApiErrorResponse::bad_request(
                "PKCE required for mobile flows: code_verifier and code_challenge_method=S256",
            ));
        }
    }

    // Audit log: OAuth login initiated
    log_audit_event(AuditEvent::OAuthLoginInitiated {
        source: source.to_string(),
        has_pkce: code_verifier.is_some(),
        timestamp: Utc::now(),
    });
    let _ = state
        .security_event_service
        .record(
            "auth.google_login_initiated",
            None,
            &client_ip.to_string(),
            None,
            None,
            serde_json::json!({
                "source": source,
                "has_pkce": code_verifier.is_some(),
            }),
        )
        .await;

    // Build state parameter: "csrf:source[:challenge]"
    let csrf = CsrfToken::new_random();
    let combined_state_str = if let Some(challenge) = code_challenge {
        format!("{}:{}:{}", csrf.secret(), source, challenge)
    } else {
        format!("{}:{}", csrf.secret(), source)
    };

    let mut auth_request = client
        .authorize_url(|| CsrfToken::new(combined_state_str.clone()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()));

    if let Some(ref verifier) = code_verifier {
        let pkce_verifier = PkceCodeVerifier::new(verifier.clone());
        let pkce_challenge = PkceCodeChallenge::from_code_verifier_sha256(&pkce_verifier);
        auth_request = auth_request.set_pkce_challenge(pkce_challenge);
    }

    let (auth_url, _) = auth_request.url();

    let mut cookie = Cookie::new("oauth_state", combined_state_str);
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(time::Duration::minutes(10));

    let mut jar = jar.add(cookie);

    if let Some(verifier) = code_verifier {
        let mut pkce_cookie = Cookie::new("oauth_pkce_verifier", verifier);
        pkce_cookie.set_path("/");
        pkce_cookie.set_http_only(true);
        pkce_cookie.set_same_site(SameSite::Lax);
        pkce_cookie.set_max_age(time::Duration::minutes(10));
        jar = jar.add(pkce_cookie);
    }

    Ok((jar, Redirect::to(auth_url.as_str())))
}

pub async fn google_callback(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
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

    let mut jar = jar.remove(Cookie::from("oauth_state"));
    let pkce_verifier = jar
        .get("oauth_pkce_verifier")
        .map(|cookie| cookie.value().to_string());
    jar = jar.remove(Cookie::from("oauth_pkce_verifier"));

    if stored_state_str != query.state {
        tracing::error!("CSRF mismatch");
        log_audit_event(AuditEvent::OAuthCallbackFailure {
            error: "CSRF mismatch".to_string(),
            source: None,
            timestamp: Utc::now(),
        });
        let _ = state
            .security_event_service
            .record(
                "auth.google_callback_failure",
                None,
                &client_ip.to_string(),
                None,
                None,
                serde_json::json!({ "reason": "csrf_mismatch" }),
            )
            .await;
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

        let verifier = pkce_verifier.clone().ok_or_else(|| {
            tracing::error!("Missing code_verifier for PKCE flow");
            log_audit_event(AuditEvent::PkceValidationFailed {
                source: oauth_state.source.clone(),
                timestamp: Utc::now(),
            });
            PKCE_FAILURES_TOTAL.inc();
            ApiErrorResponse::bad_request("Invalid OAuth state")
        })?;

        use shared::security::verify_pkce_challenge;

        if !verify_pkce_challenge(&verifier, expected_challenge) {
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
        pkce_verifier.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::error!("Google OAuth verification failed: {:?}", e);
        ApiErrorResponse::unauthorized("Failed to verify Google authentication")
    })?;

    // Capture pkce_validated flag before oauth_state is consumed
    let pkce_validated = oauth_state.code_challenge.is_some();
    let token_data = if let Some((access_token, refresh_token, user)) =
        AuthService::login_existing_user(
            &state.db,
            google_user.clone(),
            &state.config.jwt_secret,
            state.config.access_token_ttl_secs,
            state.config.refresh_token_ttl_days,
        )
        .await
        .map_err(|e| {
            tracing::error!("Existing login failed: {:?}", e);
            ApiErrorResponse::internal_error("Failed to complete authentication")
        })? {
        match state.ban_service.ensure_user_not_banned(user.id).await {
            Ok(()) => {}
            Err(BanEnforcementError::Banned) => {
                return Err(ApiErrorResponse::forbidden("user_banned"));
            }
            Err(BanEnforcementError::Unavailable) => {
                return Err(ApiErrorResponse::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ban_check_unavailable",
                ));
            }
        }

        log_audit_event(AuditEvent::OAuthCallbackSuccess {
            user_id: user.id,
            source: oauth_state.source.clone(),
            pkce_validated,
            timestamp: Utc::now(),
        });
        let _ = state
            .security_event_service
            .record(
                "auth.google_callback_success",
                Some(user.id),
                &client_ip.to_string(),
                None,
                None,
                serde_json::json!({
                    "source": oauth_state.source.clone(),
                    "pkce_validated": pkce_validated,
                }),
            )
            .await;

        OtcTokenData::AuthSuccess(OtcAuthTokenData {
            access_token,
            refresh_token,
            user_id: user.id,
            username: user.username,
            email: user.email,
            avatar_url: user.avatar_url,
            age_confirmed: user.age_confirmed_at.is_some(),
            terms_accepted: user.terms_accepted_at.is_some()
                && user.terms_accepted_version.as_deref()
                    == Some(state.config.terms_current_version.as_str()),
            required_terms_version: state.config.terms_current_version.clone(),
            accepted_terms_version: user.terms_accepted_version,
        })
    } else {
        let signup_ticket = generate_otc();
        let suggested_username = suggest_username_from_google_name(&google_user.name);
        let ticket_data = SignupTicketData {
            google_id: google_user.id,
            email: google_user.email,
            avatar_url: google_user.picture,
            source: oauth_state.source.clone(),
            created_at: Utc::now(),
        };

        write_signup_ticket(&state, &signup_ticket, &ticket_data)
            .await
            .map_err(|e| {
                tracing::error!("Failed to persist signup ticket: {:?}", e);
                ApiErrorResponse::internal_error("Failed to complete authentication")
            })?;

        USERNAME_SIGNUP_REQUIRED_TOTAL.inc();
        log_audit_event(AuditEvent::UsernameSignupRequired {
            source: oauth_state.source.clone(),
            timestamp: Utc::now(),
        });

        OtcTokenData::SignupRequired(OtcSignupRequiredData {
            signup_ticket,
            suggested_username,
            required_terms_version: state.config.terms_current_version.clone(),
            terms_url: Some("/system/terms".to_string()),
            requires_age_confirmation: true,
        })
    };

    let otc = generate_otc();

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
    ClientIp(client_ip): ClientIp,
    Json(payload): Json<RefreshRequest>,
) -> ApiResult<Json<RefreshResponse>> {
    let claims =
        AuthService::get_claims_ignoring_expiry(&payload.access_token, &state.config.jwt_secret)
            .map_err(|_| ApiErrorResponse::unauthorized("Invalid or expired token"))?;
    match state.ban_service.ensure_user_not_banned(claims.sub).await {
        Ok(()) => {}
        Err(BanEnforcementError::Banned) => {
            return Err(ApiErrorResponse::forbidden("user_banned"));
        }
        Err(BanEnforcementError::Unavailable) => {
            return Err(ApiErrorResponse::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ban_check_unavailable",
            ));
        }
    }

    let (new_access_token, new_refresh_token) = match AuthService::refresh_access_token(
        &state.db,
        &state.token_revocation,
        &payload.access_token,
        &payload.refresh_token,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
        state.config.refresh_token_ttl_days,
    )
    .await
    {
        Ok(tokens) => tokens,
        Err(RefreshAccessTokenError::InvalidAccessToken)
        | Err(RefreshAccessTokenError::InvalidOrExpiredRefreshToken) => {
            return Err(ApiErrorResponse::unauthorized("Invalid or expired token"));
        }
        Err(RefreshAccessTokenError::ReuseDetected) => {
            return Err(ApiErrorResponse::unauthorized(
                "Session revoked due to refresh token reuse",
            ));
        }
        Err(RefreshAccessTokenError::Internal(e)) => {
            tracing::error!("Token refresh failed: {:?}", e);
            if cfg!(debug_assertions) {
                return Err(ApiErrorResponse::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to refresh token",
                    format!("{:?}", e),
                ));
            }
            return Err(ApiErrorResponse::internal_error("Failed to refresh token"));
        }
    };

    let _ = state
        .security_event_service
        .record(
            "auth.refresh",
            Some(claims.sub),
            &client_ip.to_string(),
            None,
            None,
            serde_json::json!({}),
        )
        .await;

    Ok(Json(RefreshResponse {
        access_token: new_access_token,
        refresh_token: new_refresh_token,
    }))
}

pub async fn signup_complete(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Json(payload): Json<SignupCompleteRequest>,
) -> ApiResult<Json<AuthResponse>> {
    if !payload.age_confirmed {
        return Err(ApiErrorResponse::bad_request("age_confirmed must be true"));
    }
    if payload.terms_version != state.config.terms_current_version {
        return Err(ApiErrorResponse::bad_request(format!(
            "terms_version must match current version: {}",
            state.config.terms_current_version
        )));
    }

    let validated = validate_username(&payload.username, &state.config).map_err(|e| {
        log_audit_event(AuditEvent::UsernameSignupFailed {
            reason: format!("invalid_username:{}", e.message()),
            timestamp: Utc::now(),
        });
        ApiErrorResponse::bad_request(e.message())
    })?;

    let client_ip = client_ip.to_string();
    let ip_rate_key =
        crate::services::rate_limiter::RateLimiter::username_signup_ip_key(&client_ip);
    match state
        .rate_limiter
        .check_and_increment(&ip_rate_key, state.config.username_signup_rpm_per_ip, 60)
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            USERNAME_RATE_LIMIT_EXCEEDED_TOTAL.inc();
            return Err(ApiErrorResponse::too_many_requests(
                "Too many username setup attempts from this IP",
            ));
        }
        Err(e) => {
            tracing::warn!("Signup IP rate limit check failed (fail-open): {:?}", e);
        }
    }

    let ticket_rate_key = crate::services::rate_limiter::RateLimiter::username_signup_ticket_key(
        &payload.signup_ticket,
    );
    match state
        .rate_limiter
        .check_and_increment(
            &ticket_rate_key,
            state.config.username_signup_attempts_per_ticket,
            SIGNUP_TICKET_TTL_SECS,
        )
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            USERNAME_RATE_LIMIT_EXCEEDED_TOTAL.inc();
            return Err(ApiErrorResponse::too_many_requests(
                "Too many username setup attempts for this signup session",
            ));
        }
        Err(e) => {
            tracing::warn!("Signup ticket rate limit check failed (fail-open): {:?}", e);
        }
    }

    if let Some(existing) = read_signup_result(&state, &payload.signup_ticket).await {
        return Ok(Json(existing));
    }

    let lock_key = signup_lock_key(&payload.signup_ticket);
    let lock_value = Uuid::new_v4().to_string();
    let mut lock_enabled = false;
    let mut lock_acquired = false;

    if let Ok(mut conn) = state.queue.get_conn().await {
        lock_enabled = true;
        let set_result: Option<String> = redis::cmd("SET")
            .arg(&lock_key)
            .arg(&lock_value)
            .arg("NX")
            .arg("EX")
            .arg(SIGNUP_LOCK_TTL_SECS)
            .query_async(&mut conn)
            .await
            .ok();
        lock_acquired = set_result.is_some();
    } else {
        tracing::warn!("Redis unavailable for signup lock (continuing without lock)");
    }

    if lock_enabled && !lock_acquired {
        for _ in 0..SIGNUP_LOCK_POLL_RETRIES {
            sleep(TokioDuration::from_millis(125)).await;
            if let Some(existing) = read_signup_result(&state, &payload.signup_ticket).await {
                return Ok(Json(existing));
            }
        }
        return Err(ApiErrorResponse::too_many_requests(
            "Signup completion already in progress, retry shortly",
        ));
    }

    let result = async {
        if let Some(existing) = read_signup_result(&state, &payload.signup_ticket).await {
            return Ok(Json(existing));
        }

        let signup_ticket_data = read_signup_ticket(&state, &payload.signup_ticket)
            .await
            .ok_or_else(|| {
                log_audit_event(AuditEvent::UsernameSignupFailed {
                    reason: "invalid_or_expired_ticket".to_string(),
                    timestamp: Utc::now(),
                });
                ApiErrorResponse::unauthorized("Invalid or expired signup ticket")
            })?;

        let google_user = super::service::GoogleUserResult {
            id: signup_ticket_data.google_id,
            email: signup_ticket_data.email,
            verified_email: true,
            name: String::new(),
            picture: signup_ticket_data.avatar_url,
        };

        let (access_token, refresh_token, user) = match AuthService::register_with_username(
            &state.db,
            RegisterWithUsernameInput {
                google_user,
                username: validated.original,
                username_normalized: validated.normalized,
                terms_version: payload.terms_version.clone(),
                jwt_secret: &state.config.jwt_secret,
                access_token_ttl_secs: state.config.access_token_ttl_secs,
                refresh_token_ttl_days: state.config.refresh_token_ttl_days,
            },
        )
        .await
        {
            Ok(v) => v,
            Err(RegisterUserError::UsernameTaken) => {
                USERNAME_SIGNUP_CONFLICT_TOTAL.inc();
                log_audit_event(AuditEvent::UsernameSignupFailed {
                    reason: "username_taken".to_string(),
                    timestamp: Utc::now(),
                });
                return Err(ApiErrorResponse::conflict("Username already taken"));
            }
            Err(RegisterUserError::Other(e)) => {
                tracing::error!("Signup completion failed: {:?}", e);
                log_audit_event(AuditEvent::UsernameSignupFailed {
                    reason: "internal_error".to_string(),
                    timestamp: Utc::now(),
                });
                return Err(ApiErrorResponse::internal_error(
                    "Failed to complete signup",
                ));
            }
        };

        let response = AuthResponse {
            access_token,
            refresh_token,
            user: UserDto::from_user_model(&user, &state.config.terms_current_version),
        };

        if let Err(e) = write_signup_result(&state, &payload.signup_ticket, &response).await {
            tracing::warn!("Failed to cache signup idempotency result: {:?}", e);
        }
        delete_signup_ticket(&state, &payload.signup_ticket).await;

        USERNAME_SIGNUP_COMPLETE_TOTAL.inc();
        log_audit_event(AuditEvent::UsernameSignupCompleted {
            user_id: response.user.id,
            timestamp: Utc::now(),
        });
        let _ = state
            .security_event_service
            .record(
                "auth.signup_complete",
                Some(response.user.id),
                &client_ip,
                None,
                None,
                serde_json::json!({ "signup_ticket": payload.signup_ticket }),
            )
            .await;

        Ok(Json(response))
    }
    .await;

    if lock_enabled
        && lock_acquired
        && let Ok(mut conn) = state.queue.get_conn().await
    {
        let script = redis::Script::new(
            "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end",
        );
        let _: Result<i32, _> = script
            .key(&lock_key)
            .arg(&lock_value)
            .invoke_async(&mut conn)
            .await;
    }

    result
}

pub async fn create_extension_session(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(payload): Json<ExtensionSessionRequest>,
) -> ApiResult<Json<ExtensionSessionResponse>> {
    let platform = payload.platform.trim().to_ascii_lowercase();
    if platform != "ios" && platform != "android" {
        return Err(ApiErrorResponse::bad_request(
            "platform must be one of: ios, android",
        ));
    }

    let device_id_hash = payload.device_id_hash.trim().to_string();
    if device_id_hash.len() < 16 || device_id_hash.len() > 256 {
        return Err(ApiErrorResponse::bad_request(
            "device_id_hash length must be between 16 and 256",
        ));
    }

    let scope = normalize_extension_scopes(payload.requested_scopes)?;
    let session_jti = Uuid::new_v4();
    let issued_at = Utc::now();
    let expires_at = issued_at + Duration::seconds(state.config.extension_token_ttl_secs as i64);

    let model = extension_sessions::ActiveModel {
        jti: Set(session_jti),
        user_id: Set(user_id),
        device_id_hash: Set(device_id_hash.clone()),
        platform: Set(platform.clone()),
        scope: Set(scope.clone()),
        issued_at: Set(issued_at.into()),
        expires_at: Set(expires_at.into()),
        revoked_at: Set(None),
    };
    model.insert(&state.db).await?;

    let extension_access_token = create_extension_access_token(
        user_id,
        session_jti,
        &platform,
        &device_id_hash,
        &scope,
        &state.config.jwt_secret,
        state.config.extension_token_ttl_secs,
    )?;

    Ok(Json(ExtensionSessionResponse {
        extension_access_token,
        expires_in_seconds: state.config.extension_token_ttl_secs,
        session_jti,
        scope,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
) -> ApiResult<()> {
    let claims = AuthService::validate_token(auth.token(), &state.config.jwt_secret)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid token"))?;

    if state.token_revocation.is_revoked(claims.jti).await {
        return Err(ApiErrorResponse::unauthorized("Token revoked"));
    }
    let user_id = claims.sub;
    match state.ban_service.ensure_user_not_banned(user_id).await {
        Ok(()) => {}
        Err(BanEnforcementError::Banned) => {
            return Err(ApiErrorResponse::forbidden("user_banned"));
        }
        Err(BanEnforcementError::Unavailable) => {
            return Err(ApiErrorResponse::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ban_check_unavailable",
            ));
        }
    }

    AuthService::logout_all(&state.db, user_id).await?;
    AuthService::revoke_extension_sessions(&state.db, user_id).await?;

    let now_secs = Utc::now().timestamp() as usize;
    if claims.exp > now_secs {
        let _ = state
            .token_revocation
            .revoke_token(claims.jti, claims.exp - now_secs)
            .await;
    }
    let _ = state
        .security_event_service
        .record(
            "auth.logout",
            Some(user_id),
            &client_ip.to_string(),
            None,
            None,
            serde_json::json!({}),
        )
        .await;
    Ok(())
}

/// Development-only endpoint for testing without Google OAuth
/// Only works when RUST_LOG contains "debug"
pub async fn dev_login(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Json(payload): Json<DevLoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    // Only allow in development mode or test environment
    let log_level = std::env::var("RUST_LOG").unwrap_or_default();
    if !log_level.contains("debug") && state.config.environment != "test" {
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
        Some(u) => {
            match state.ban_service.ensure_user_not_banned(u.id).await {
                Ok(()) => {}
                Err(BanEnforcementError::Banned) => {
                    return Err(ApiErrorResponse::forbidden("user_banned"));
                }
                Err(BanEnforcementError::Unavailable) => {
                    return Err(ApiErrorResponse::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "ban_check_unavailable",
                    ));
                }
            }
            let needs_onboarding = u.age_confirmed_at.is_none()
                || u.terms_accepted_at.is_none()
                || u.terms_accepted_version.as_deref()
                    != Some(state.config.terms_current_version.as_str());
            if needs_onboarding {
                let now = Utc::now().fixed_offset();
                let mut active: users::ActiveModel = u.into();
                active.age_confirmed_at = Set(Some(now));
                active.terms_accepted_at = Set(Some(now));
                active.terms_accepted_version =
                    Set(Some(state.config.terms_current_version.clone()));
                active.update(&state.db).await?
            } else {
                u
            }
        }
        None => {
            let now = Utc::now().fixed_offset();
            let new_user = users::ActiveModel {
                id: Set(Uuid::new_v4()),
                google_id: Set(google_id),
                username: Set(payload.username),
                email: Set(payload.email),
                age_confirmed_at: Set(Some(now)),
                terms_accepted_at: Set(Some(now)),
                terms_accepted_version: Set(Some(state.config.terms_current_version.clone())),
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
        user: UserDto::from_user_model(&user, &state.config.terms_current_version),
    };

    let _ = state
        .security_event_service
        .record(
            "auth.dev_login",
            Some(response.user.id),
            &client_ip.to_string(),
            None,
            None,
            serde_json::json!({}),
        )
        .await;

    Ok(Json(response))
}

/// Exchange one-time code for access/refresh tokens
pub async fn exchange_otc(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Json(payload): Json<ExchangeOtcRequest>,
) -> ApiResult<impl IntoResponse> {
    let client_ip = client_ip.to_string();
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

    OTC_EXCHANGES_TOTAL.inc();
    timer.observe_duration();

    match token_data {
        OtcTokenData::AuthSuccess(token_data) => {
            match state
                .ban_service
                .ensure_user_not_banned(token_data.user_id)
                .await
            {
                Ok(()) => {}
                Err(BanEnforcementError::Banned) => {
                    return Err(ApiErrorResponse::forbidden("user_banned"));
                }
                Err(BanEnforcementError::Unavailable) => {
                    return Err(ApiErrorResponse::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "ban_check_unavailable",
                    ));
                }
            }

            log_audit_event(AuditEvent::OtcExchangeSuccess {
                user_id: token_data.user_id,
                client_ip: client_ip.clone(),
                timestamp: Utc::now(),
            });
            let _ = state
                .security_event_service
                .record(
                    "auth.exchange_otc_success",
                    Some(token_data.user_id),
                    &client_ip,
                    None,
                    None,
                    serde_json::json!({}),
                )
                .await;

            let user_dto = users::Entity::find_by_id(token_data.user_id)
                .one(&state.db)
                .await
                .map_err(ApiErrorResponse::db_error)?
                .map(|user| UserDto::from_user_model(&user, &state.config.terms_current_version))
                .unwrap_or(UserDto {
                    id: token_data.user_id,
                    username: token_data.username,
                    email: token_data.email,
                    avatar_url: token_data.avatar_url,
                    age_confirmed: token_data.age_confirmed,
                    terms_accepted: token_data.terms_accepted,
                    required_terms_version: if token_data.required_terms_version.is_empty() {
                        state.config.terms_current_version.clone()
                    } else {
                        token_data.required_terms_version
                    },
                    accepted_terms_version: token_data.accepted_terms_version,
                });

            let response = ExchangeOtcResponse {
                access_token: token_data.access_token,
                refresh_token: token_data.refresh_token,
                user: user_dto,
            };

            Ok((StatusCode::OK, Json(response)).into_response())
        }
        OtcTokenData::SignupRequired(signup_data) => {
            let _ = state
                .security_event_service
                .record(
                    "auth.exchange_otc_signup_required",
                    None,
                    &client_ip,
                    None,
                    None,
                    serde_json::json!({}),
                )
                .await;
            let response = UsernameRequiredResponse {
                error: "username_required".to_string(),
                signup_ticket: signup_data.signup_ticket,
                suggested_username: signup_data.suggested_username,
                requires_age_confirmation: signup_data.requires_age_confirmation,
                required_terms_version: signup_data.required_terms_version,
                terms_url: signup_data.terms_url,
                rules: UsernameRulesDto {
                    min_length: USERNAME_MIN_LEN,
                    max_length: USERNAME_MAX_LEN,
                    pattern: USERNAME_PATTERN.to_string(),
                },
            };

            Ok((StatusCode::CONFLICT, Json(response)).into_response())
        }
    }
}
