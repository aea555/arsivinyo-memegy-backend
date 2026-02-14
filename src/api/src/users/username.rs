use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use shared::config::Config;
use std::collections::HashSet;
use uuid::Uuid;

pub const USERNAME_MIN_LEN: usize = 3;
pub const USERNAME_MAX_LEN: usize = 20;
pub const USERNAME_PATTERN: &str = "^[a-z0-9_]{3,20}$";

#[derive(Debug, Clone)]
pub struct ValidatedUsername {
    pub original: String,
    pub normalized: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsernameValidationError {
    InvalidWhitespace,
    InvalidLength,
    InvalidCharacters,
    Reserved,
}

impl UsernameValidationError {
    pub fn message(self) -> &'static str {
        match self {
            UsernameValidationError::InvalidWhitespace => {
                "Username must not contain leading or trailing whitespace"
            }
            UsernameValidationError::InvalidLength => "Username length must be between 3 and 20",
            UsernameValidationError::InvalidCharacters => {
                "Username may only contain lowercase letters, numbers, and underscore"
            }
            UsernameValidationError::Reserved => "Username is reserved",
        }
    }
}

fn default_reserved_usernames() -> HashSet<&'static str> {
    HashSet::from([
        "admin",
        "support",
        "api",
        "root",
        "system",
        "me",
        "users",
        "auth",
        "feed",
        "www",
        "null",
        "undefined",
        "help",
        "contact",
        "info",
        "security",
        "abuse",
        "postmaster",
        "webmaster",
        "host",
        "superuser",
        "staff",
        "moderator",
        "manager",
        "editor",
        "superadmin",
        "sysadmin",
        "sys",
        "administrator",
        "operator",
        "dev",
        "developer",
        "qa",
        "test",
        "testing",
    ])
}

fn collect_reserved_usernames(config: &Config) -> HashSet<String> {
    let mut names: HashSet<String> = default_reserved_usernames()
        .into_iter()
        .map(String::from)
        .collect();

    for value in config.username_reserved_values.split(',') {
        let trimmed = value.trim().to_ascii_lowercase();
        if !trimmed.is_empty() {
            names.insert(trimmed);
        }
    }

    names
}

fn normalize_for_reserved_match(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_lowercase())
        .collect::<String>()
}

fn is_reserved_username_variant(normalized_username: &str, reserved: &HashSet<String>) -> bool {
    if reserved.contains(normalized_username) {
        return true;
    }

    let folded_username = normalize_for_reserved_match(normalized_username);
    if folded_username.is_empty() {
        return false;
    }

    reserved.iter().any(|value| {
        let folded_reserved = normalize_for_reserved_match(value);
        !folded_reserved.is_empty() && folded_username.contains(&folded_reserved)
    })
}

pub fn validate_username(
    input: &str,
    config: &Config,
) -> Result<ValidatedUsername, UsernameValidationError> {
    if input.trim() != input {
        return Err(UsernameValidationError::InvalidWhitespace);
    }

    if input.len() < USERNAME_MIN_LEN || input.len() > USERNAME_MAX_LEN {
        return Err(UsernameValidationError::InvalidLength);
    }

    if !input
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(UsernameValidationError::InvalidCharacters);
    }

    let normalized = input.to_ascii_lowercase();
    let reserved = collect_reserved_usernames(config);
    if is_reserved_username_variant(&normalized, &reserved) {
        return Err(UsernameValidationError::Reserved);
    }

    Ok(ValidatedUsername {
        original: input.to_string(),
        normalized,
    })
}

pub fn suggest_username_from_google_name(name: &str) -> String {
    let mut out = String::with_capacity(USERNAME_MAX_LEN);
    let mut previous_was_separator = false;

    for c in name.chars() {
        if out.len() >= USERNAME_MAX_LEN {
            break;
        }

        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !out.is_empty() {
            out.push('_');
            previous_was_separator = true;
        }
    }

    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "user".to_string()
    } else if out.len() < USERNAME_MIN_LEN {
        format!("{out}_user")
    } else {
        out
    }
}

pub async fn username_exists_case_insensitive(
    db: &DatabaseConnection,
    normalized_username: &str,
    exclude_user_id: Option<Uuid>,
) -> Result<bool, DbErr> {
    let row = if let Some(user_id) = exclude_user_id {
        db.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 FROM users WHERE LOWER(username) = $1 AND id <> $2 LIMIT 1",
            vec![normalized_username.into(), user_id.into()],
        ))
        .await?
    } else {
        db.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 FROM users WHERE LOWER(username) = $1 LIMIT 1",
            vec![normalized_username.into()],
        ))
        .await?
    };

    Ok(row.is_some())
}

pub fn is_username_unique_violation(err: &DbErr) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("idx_users_username_normalized_unique")
        || msg.contains("username_normalized")
        || msg.contains("users_username_normalized")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            server_host: "127.0.0.1".to_string(),
            server_port: 3000,
            oauth_redirect_base_url: "http://localhost".to_string(),
            frontend_app_url: "http://localhost".to_string(),
            cors_allowed_origins: String::new(),
            environment: "test".to_string(),
            require_cloudflare_headers: false,
            database_url: "postgres://localhost/test".to_string(),
            valkey_url: "redis://localhost:6379".to_string(),
            jwt_secret: "secret".to_string(),
            access_token_ttl_secs: 900,
            refresh_token_ttl_days: 14,
            auth_require_username_on_google_signup: true,
            username_reserved_values: "owner,moderator".to_string(),
            admin_api_enabled: false,
            admin_jwt_issuer: None,
            admin_jwt_audience: None,
            admin_jwt_public_keys: std::collections::HashMap::new(),
            admin_jwt_max_ttl_secs: 300,
            admin_jwt_clock_skew_secs: 60,
            admin_replay_protection_enabled: true,
            admin_allowed_ip_cidrs: String::new(),
            google_client_id: "id".to_string(),
            google_client_secret: "secret".to_string(),
            mobile_app_scheme: "memegy://".to_string(),
            minio_endpoint: "http://localhost".to_string(),
            minio_access_key: "x".to_string(),
            minio_secret_key: "x".to_string(),
            minio_bucket_videos: "videos".to_string(),
            minio_bucket_raw: "raw".to_string(),
            minio_public_endpoint: "http://localhost".to_string(),
            presigned_url_expiry_secs: 3600,
            max_file_size_bytes: 1,
            limit_upload_bytes_hourly: 1,
            upload_size_tolerance_bytes: 0,
            min_video_size_bytes: 1,
            feed_page_size: 20,
            feed_cache_ttl_secs: 300,
            limit_feed_rpm: 60,
            rate_limit_window_secs: 60,
            ip_rate_limit_rpm: 30,
            like_actions_rpm_limit: 60,
            like_actions_window_secs: 60,
            username_signup_rpm_per_ip: 20,
            username_signup_attempts_per_ticket: 10,
            username_update_rpm_per_user: 5,
            otc_rate_limit_max_attempts: 5,
            otc_rate_limit_window_seconds: 60,
            search_max_tokens: 50,
            search_max_token_length: 50,
            search_max_query_chars: 200,
            search_timeout_secs: 5,
            search_cache_ttl_secs: 10,
            search_rpm_limit: 30,
            extension_token_ttl_secs: 600,
            keyboard_search_rpm: 60,
            keyboard_search_max_limit: 20,
            keyboard_send_ticket_rpm: 30,
            keyboard_daily_send_cap: 100,
            keyboard_nonce_ttl_secs: 300,
            keyboard_ticket_ttl_secs: 45,
            keyboard_media_url_ttl_secs: 30,
            draft_video_cleanup_hours: 24,
            worker_retry_max_attempts: 3,
            worker_retry_backoff_base_secs: 2,
            ffmpeg_transcode_timeout_secs: 180,
            ffmpeg_thumbnail_timeout_secs: 30,
            video_ws_enabled: true,
            video_ws_max_conn_per_user: 3,
            video_ws_max_conn_global: 5000,
            video_ws_connect_rpm_per_ip: 30,
            video_ws_connect_rpm_per_user: 60,
            video_ws_send_buffer: 64,
            video_ws_heartbeat_secs: 20,
        }
    }

    #[test]
    fn validates_basic_format_rules() {
        let cfg = test_config();
        assert!(validate_username("abc_123", &cfg).is_ok());
        assert!(matches!(
            validate_username("Abc", &cfg),
            Err(UsernameValidationError::InvalidCharacters)
        ));
        assert!(matches!(
            validate_username("ab", &cfg),
            Err(UsernameValidationError::InvalidLength)
        ));
        assert!(matches!(
            validate_username("abc ", &cfg),
            Err(UsernameValidationError::InvalidWhitespace)
        ));
    }

    #[test]
    fn validates_reserved_values() {
        let cfg = test_config();
        assert!(matches!(
            validate_username("admin", &cfg),
            Err(UsernameValidationError::Reserved)
        ));
        assert!(matches!(
            validate_username("moderator", &cfg),
            Err(UsernameValidationError::Reserved)
        ));
        assert!(matches!(
            validate_username("admin1", &cfg),
            Err(UsernameValidationError::Reserved)
        ));
        assert!(matches!(
            validate_username("1admin", &cfg),
            Err(UsernameValidationError::Reserved)
        ));
        assert!(matches!(
            validate_username("a_d_m_i_n", &cfg),
            Err(UsernameValidationError::Reserved)
        ));
    }

    #[test]
    fn creates_reasonable_suggestions() {
        assert_eq!(suggest_username_from_google_name("John Doe"), "john_doe");
        assert_eq!(suggest_username_from_google_name("   "), "user");
    }
}
