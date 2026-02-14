use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct GoogleLoginQuery {
    pub source: Option<String>,                // "web", "popup", "mobile"
    pub code_challenge: Option<String>,        // For PKCE
    pub code_challenge_method: Option<String>, // "S256"
    pub code_verifier: Option<String>,         // PKCE code_verifier
}

#[derive(Deserialize)]
pub struct GoogleCallbackQuery {
    pub code: String,
    pub state: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: UserDto,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct UserDto {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub avatar_url: Option<String>,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Deserialize)]
pub struct DevLoginRequest {
    pub username: String,
    pub email: String,
}

#[derive(Deserialize)]
pub struct ExchangeOtcRequest {
    pub code: String,
}

#[derive(Serialize)]
pub struct ExchangeOtcResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: UserDto,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct UsernameRulesDto {
    pub min_length: usize,
    pub max_length: usize,
    pub pattern: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct UsernameRequiredResponse {
    pub error: String,
    pub signup_ticket: String,
    pub suggested_username: String,
    pub rules: UsernameRulesDto,
}

#[derive(Deserialize)]
pub struct SignupCompleteRequest {
    pub signup_ticket: String,
    pub username: String,
}

#[derive(Deserialize)]
pub struct ExtensionSessionRequest {
    pub platform: String,
    pub device_id_hash: String,
    #[serde(default)]
    pub requested_scopes: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct ExtensionSessionResponse {
    pub extension_access_token: String,
    pub expires_in_seconds: usize,
    pub session_jti: Uuid,
    pub scope: Vec<String>,
}
