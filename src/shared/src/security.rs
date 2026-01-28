use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,  // User ID
    pub jti: Uuid,  // Unique Token ID (for revocation)
    pub exp: usize, // Expiration
    pub iat: usize, // Issued At
}

pub fn hash_token(token: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(token.as_bytes(), &salt)
        .map_err(|e| anyhow!(e.to_string()))?;
    Ok(password_hash.to_string())
}

pub fn verify_token_hash(token: &str, hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(token.as_bytes(), &parsed_hash)
        .is_ok()
}

pub fn create_access_token(user_id: Uuid, secret: &str) -> Result<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as usize;

    // 15 Minutes TTL
    let exp = now + (15 * 60);

    let claims = Claims {
        sub: user_id,
        jti: Uuid::new_v4(),
        exp,
        iat: now,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| anyhow!(e.to_string()))
}

pub fn verify_jwt(token: &str, secret: &str) -> Result<Claims> {
    let validation = Validation::default();
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| anyhow!(e.to_string()))?;

    Ok(token_data.claims)
}

pub fn generate_refresh_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng(); // Use thread_rng
    let random_bytes: [u8; 32] = rng.random();
    hex::encode(random_bytes)
}
