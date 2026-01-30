use anyhow::{Result, anyhow, Context};
use chrono::{Duration, Utc};
use sea_orm::*; // Import everything
use serde::Deserialize;
use shared::entities::{refresh_tokens, users};
use shared::security::{create_access_token, generate_refresh_token, hash_token, verify_token_hash};
use uuid::Uuid;
#[derive(Deserialize, Debug)]
pub struct GoogleUserResult {
    pub id: String,
    pub email: String,
    pub verified_email: bool,
    pub name: String,
    #[serde(rename = "picture")]
    pub _picture: String,
}

pub struct AuthService;

impl AuthService {
    pub async fn verify_google_code(
        code: String,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
    ) -> Result<GoogleUserResult> {
        // Trim credentials to avoid common copy-paste errors
        let client_id = client_id.trim();
        let client_secret = client_secret.trim();
        let redirect_uri = redirect_uri.trim();

        tracing::debug!("Verifying Google Code. ClientID: {}, Redirect: {}", client_id, redirect_uri);

        // Use standard reqwest client (supports HTTP/2 by default)
        let client = reqwest::Client::new();
        
        // Exchange code for token
        let params = [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", &code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ];

        tracing::debug!("Sending Token Request to https://oauth2.googleapis.com/token");

        let token_res = client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await?;

        if !token_res.status().is_success() {
             let status = token_res.status();
             let error_text = token_res.text().await?;
             tracing::error!("Google Token Exchange Failed. Status: {}, Response: {}", status, error_text);
             return Err(anyhow!("Failed to exchange code for token: {:?}", error_text));
        }

        #[derive(Deserialize)]
        struct GoogleTokenResponse {
            access_token: String,
        }

        let token_data: GoogleTokenResponse = token_res.json().await?;

        // Get User Info
        let user_res = client
            .get("https://www.googleapis.com/oauth2/v2/userinfo")
            .bearer_auth(token_data.access_token)
            .send()
            .await?;

        if !user_res.status().is_success() {
             let error_text = user_res.text().await?;
             tracing::error!("Google User Info Failed. Response: {}", error_text);
             return Err(anyhow!("Failed to fetch user info: {}", error_text));
        }

        let user_data: GoogleUserResult = user_res.json().await?;
        Ok(user_data)
    }

    pub async fn login_or_register(
        db: &DatabaseConnection,
        google_user: GoogleUserResult,
        jwt_secret: &str,
    ) -> Result<(String, String, users::Model)> {
        if !google_user.verified_email {
            return Err(anyhow!("Email not verified by Google"));
        }

        let user = users::Entity::find()
            .filter(users::Column::GoogleId.eq(&google_user.id))
            .one(db)
            .await?;

        let user = match user {
            Some(u) => u,
            None => {
                let new_user = users::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    google_id: Set(google_user.id),
                    username: Set(google_user.name), // Basic username mapping
                    email: Set(google_user.email),
                    ..Default::default()
                };
                new_user.insert(db).await?
            }
        };

        let access_token = create_access_token(user.id, jwt_secret)?;
        let refresh_token = generate_refresh_token();
        let refresh_token_hash = hash_token(&refresh_token)?;

        // Store Refresh Token
        // Expiry: 14 Days
        let expires_at = Utc::now() + Duration::days(14);
        
        let rt_model = refresh_tokens::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user.id),
            token_hash: Set(refresh_token_hash),
            expires_at: Set(expires_at.into()),
            ..Default::default()
        };

        rt_model.insert(db).await?;

        Ok((access_token, refresh_token, user))
    }

    pub async fn refresh_access_token(
        db: &DatabaseConnection,
        expired_access_token: &str,
        refresh_token_raw: &str,
        jwt_secret: &str,
    ) -> Result<String> {
        // 1. Get User ID from expired token
        let user_id = Self::get_user_id_from_expired_token(expired_access_token, jwt_secret)?;

        // 2. Find all active refresh tokens for this user
        let user_tokens = refresh_tokens::Entity::find()
            .filter(refresh_tokens::Column::UserId.eq(user_id))
            .all(db)
            .await?;

        // 3. Match the hash
        let mut valid_token_model = None;
        for token_model in user_tokens {
            if verify_token_hash(refresh_token_raw, &token_model.token_hash) {
                // Check expiry
                if token_model.expires_at < Utc::now() {
                    // Clean up expired token
                    let _ = refresh_tokens::Entity::delete_by_id(token_model.id).exec(db).await;
                    continue;
                }
                valid_token_model = Some(token_model);
                break;
            }
        }

        match valid_token_model {
            Some(_) => {
                // Issue new access token
                let new_access_token = create_access_token(user_id, jwt_secret)?;
                Ok(new_access_token)
            }
            None => Err(anyhow!("Invalid or expired refresh token")),
        }
    }

    pub async fn logout_all(
        db: &DatabaseConnection,
        user_id: Uuid,
    ) -> Result<()> {
        refresh_tokens::Entity::delete_many()
            .filter(refresh_tokens::Column::UserId.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }
    
    // Helper to extract user_id from expired token
    pub fn get_user_id_from_expired_token(token: &str, secret: &str) -> Result<Uuid> {
        use jsonwebtoken::{Validation, DecodingKey, Algorithm};
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false; // Ignore expiry
        
        let token_data = jsonwebtoken::decode::<shared::security::Claims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &validation
        )?;
        
        Ok(token_data.claims.sub)
    }
}
