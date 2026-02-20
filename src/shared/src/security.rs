use anyhow::{Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExtensionClaims {
    pub sub: Uuid, // User ID
    pub jti: Uuid, // Session JTI
    pub exp: usize,
    pub iat: usize,
    pub scope: Vec<String>,
    pub platform: String,
    pub device_id_hash: String,
    pub token_use: String,
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

pub fn create_access_token(user_id: Uuid, secret: &str, ttl_secs: usize) -> Result<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as usize;

    let exp = now + ttl_secs;

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

pub fn create_extension_access_token(
    user_id: Uuid,
    session_jti: Uuid,
    platform: &str,
    device_id_hash: &str,
    scope: &[String],
    secret: &str,
    ttl_secs: usize,
) -> Result<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as usize;
    let exp = now + ttl_secs;

    let claims = ExtensionClaims {
        sub: user_id,
        jti: session_jti,
        exp,
        iat: now,
        scope: scope.to_vec(),
        platform: platform.to_string(),
        device_id_hash: device_id_hash.to_string(),
        token_use: "extension".to_string(),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| anyhow!(e.to_string()))
}

pub fn verify_extension_jwt(token: &str, secret: &str) -> Result<ExtensionClaims> {
    let validation = Validation::default();
    let token_data = decode::<ExtensionClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| anyhow!(e.to_string()))?;

    if token_data.claims.token_use != "extension" {
        return Err(anyhow!("Invalid token_use"));
    }

    Ok(token_data.claims)
}

pub fn generate_refresh_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng(); // Use thread_rng
    let random_bytes: [u8; 32] = rng.random();
    hex::encode(random_bytes)
}

/// Generate a cryptographically secure one-time code for OAuth token exchange
pub fn generate_otc() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let random_bytes: [u8; 32] = rng.random();
    hex::encode(random_bytes)
}

/// Validates PKCE code_verifier format per RFC 7636
/// Must be 43-128 chars of [A-Z a-z 0-9 - . _ ~]
pub fn validate_code_verifier(verifier: &str) -> bool {
    let len = verifier.len();
    if !(43..=128).contains(&len) {
        return false;
    }

    verifier
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
}

/// Computes SHA256 code_challenge from code_verifier
/// Returns base64url-encoded (no padding) challenge per RFC 7636
pub fn compute_code_challenge(verifier: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

/// Validates that code_verifier produces the expected code_challenge
/// Used for PKCE validation in OAuth callback
pub fn verify_pkce_challenge(verifier: &str, expected_challenge: &str) -> bool {
    if !validate_code_verifier(verifier) {
        return false;
    }

    let computed = compute_code_challenge(verifier);
    computed == expected_challenge
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_verifier_validation() {
        // Valid
        assert!(validate_code_verifier(&"a".repeat(43)));
        assert!(validate_code_verifier(&"a".repeat(128)));
        assert!(validate_code_verifier(&"aZ0-._~".repeat(7)));

        // Invalid length
        assert!(!validate_code_verifier(&"a".repeat(42)));
        assert!(!validate_code_verifier(&"a".repeat(129)));

        // Invalid chars
        assert!(!validate_code_verifier(&format!("{}+", "a".repeat(43))));
        assert!(!validate_code_verifier(&format!("{}/", "a".repeat(43))));
    }

    #[test]
    fn test_pkce_challenge_computation() {
        // RFC 7636 Appendix B test vector
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

        let computed = compute_code_challenge(verifier);
        assert_eq!(computed, expected);
    }

    #[test]
    fn test_verify_pkce_challenge() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

        assert!(verify_pkce_challenge(verifier, challenge));
        assert!(!verify_pkce_challenge(verifier, "wrong_challenge"));
        assert!(!verify_pkce_challenge("short", challenge));
    }

    #[test]
    fn test_extension_jwt_roundtrip() {
        let user_id = Uuid::new_v4();
        let session_jti = Uuid::new_v4();
        let secret = "test_secret";
        let scope = vec!["keyboard.search".to_string(), "keyboard.send".to_string()];

        let token = create_extension_access_token(
            user_id,
            session_jti,
            "android",
            "device_hash_abc",
            &scope,
            secret,
            60,
        )
        .unwrap();

        let claims = verify_extension_jwt(&token, secret).unwrap();
        assert_eq!(claims.sub, user_id);
        assert_eq!(claims.jti, session_jti);
        assert_eq!(claims.scope, scope);
        assert_eq!(claims.platform, "android");
    }
}
