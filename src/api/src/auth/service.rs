use crate::auth::revocation::TokenRevocationService;
use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use sea_orm::*;
use serde::Deserialize;
use shared::entities::{refresh_tokens, users};
use shared::security::{
    create_access_token, generate_refresh_token, hash_token, verify_token_hash, Claims,
};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Deserialize, Debug)]
pub struct GoogleUserResult {
    pub id: String,
    pub email: String,
    pub verified_email: bool,
    pub name: String,
    pub picture: String,
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

        tracing::debug!(
            "Verifying Google Code. ClientID: {}, Redirect: {}",
            client_id,
            redirect_uri
        );

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
            tracing::error!(
                "Google Token Exchange Failed. Status: {}, Response: {}",
                status,
                error_text
            );
            return Err(anyhow!(
                "Failed to exchange code for token: {:?}",
                error_text
            ));
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
        access_token_ttl_secs: usize,
        refresh_token_ttl_days: u16,
    ) -> Result<(String, String, users::Model)> {
        if !google_user.verified_email {
            return Err(anyhow!("Email not verified by Google"));
        }

        let user = users::Entity::find()
            .filter(users::Column::GoogleId.eq(&google_user.id))
            .one(db)
            .await?;

        let user = match user {
            Some(u) => {
                let mut active_model: users::ActiveModel = u.into();

                // Account Recovery Logic: If deleted, reactivate
                if active_model.deleted_at.as_ref().is_some() {
                    tracing::info!(
                        "Recovering soft-deleted account: {}",
                        active_model.id.as_ref()
                    );
                    active_model.deleted_at = Set(None);
                }

                // Update immutable fields from Google (e.g. avatar might change)
                active_model.avatar_url = Set(Some(google_user.picture));

                active_model.update(db).await?
            }
            None => {
                let new_user = users::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    google_id: Set(google_user.id),
                    username: Set(google_user.name),
                    email: Set(google_user.email),
                    avatar_url: Set(Some(google_user.picture)),
                    ..Default::default()
                };
                new_user.insert(db).await?
            }
        };

        let access_token = create_access_token(user.id, jwt_secret, access_token_ttl_secs)?;
        let refresh_token = generate_refresh_token();
        let refresh_token_hash = hash_token(&refresh_token)?;

        // Store Refresh Token
        let expires_at = Utc::now() + Duration::days(refresh_token_ttl_days as i64);

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

    /// Refresh access token with proper token rotation and revocation.
    /// Returns (new_access_token, new_refresh_token).
    pub async fn refresh_access_token(
        db: &DatabaseConnection,
        revocation: &TokenRevocationService,
        old_access_token: &str,
        refresh_token_raw: &str,
        jwt_secret: &str,
        access_token_ttl_secs: usize,
        refresh_token_ttl_days: u16,
    ) -> Result<(String, String)> {
        // 1. Get claims from old token (even if expired)
        let old_claims = Self::get_claims_ignoring_expiry(old_access_token, jwt_secret)?;
        let user_id = old_claims.sub;

        // 2. Find and validate the refresh token (with lock to prevent concurrent refresh)
        let txn = db.begin().await?;

        let user_tokens = refresh_tokens::Entity::find()
            .filter(refresh_tokens::Column::UserId.eq(user_id))
            .all(&txn)
            .await?;

        // 3. Match the hash and find valid token
        let mut valid_token_model = None;
        for token_model in user_tokens {
            if verify_token_hash(refresh_token_raw, &token_model.token_hash) {
                // Check expiry
                if token_model.expires_at < Utc::now() {
                    // Clean up expired token
                    let _ = refresh_tokens::Entity::delete_by_id(token_model.id)
                        .exec(&txn)
                        .await;
                    continue;
                }
                valid_token_model = Some(token_model);
                break;
            }
        }

        let old_refresh =
            valid_token_model.ok_or_else(|| anyhow!("Invalid or expired refresh token"))?;

        // 4. REVOKE old access token (add to Redis blacklist)
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as usize)
            .unwrap_or(0);

        let remaining_ttl = old_claims.exp.saturating_sub(now_secs);
        if remaining_ttl > 0 {
            // Only revoke if token hasn't already expired
            if let Err(e) = revocation.revoke_token(old_claims.jti, remaining_ttl).await {
                tracing::warn!("Failed to revoke old token {}: {:?}", old_claims.jti, e);
                // Continue anyway - the old token will expire naturally
            }
            tracing::debug!(
                "Revoked old access token {} with {}s remaining TTL",
                old_claims.jti,
                remaining_ttl
            );
        }

        // 5. DELETE old refresh token (rotation - prevents reuse)
        refresh_tokens::Entity::delete_by_id(old_refresh.id)
            .exec(&txn)
            .await?;

        // 6. Issue NEW access and refresh tokens
        let new_access_token = create_access_token(user_id, jwt_secret, access_token_ttl_secs)?;
        let new_refresh_token = generate_refresh_token();
        let new_refresh_hash = hash_token(&new_refresh_token)?;

        // 7. Store new refresh token
        let expires_at = Utc::now() + Duration::days(refresh_token_ttl_days as i64);
        let new_rt_model = refresh_tokens::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            token_hash: Set(new_refresh_hash),
            expires_at: Set(expires_at.into()),
            ..Default::default()
        };
        new_rt_model.insert(&txn).await?;

        txn.commit().await?;

        Ok((new_access_token, new_refresh_token))
    }

    pub async fn logout_all(db: &DatabaseConnection, user_id: Uuid) -> Result<()> {
        refresh_tokens::Entity::delete_many()
            .filter(refresh_tokens::Column::UserId.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }

    /// Validate access token and extract claims (Enforces Expiry)
    pub fn validate_token(token: &str, secret: &str) -> Result<Claims> {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation};
        let validation = Validation::new(Algorithm::HS256);
        // validate_exp is true by default

        let token_data = jsonwebtoken::decode::<Claims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        )?;

        Ok(token_data.claims)
    }

    /// Extract claims from token WITHOUT validating expiry.
    /// Used for token refresh when access token may be expired.
    pub fn get_claims_ignoring_expiry(token: &str, secret: &str) -> Result<Claims> {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation};
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false; // Ignore expiry

        let token_data = jsonwebtoken::decode::<Claims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        )?;

        Ok(token_data.claims)
    }
}
