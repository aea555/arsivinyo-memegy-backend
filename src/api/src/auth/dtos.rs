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

#[derive(Serialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: UserDto,
}

#[derive(Serialize, Deserialize)]
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
