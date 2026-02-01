use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
