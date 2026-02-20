use anyhow::{Result, anyhow};
use dotenvy::dotenv;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;

#[derive(Debug, Default, Deserialize)]
struct TermsFrontmatter {
    version: Option<String>,
    effective_at: Option<String>,
    jurisdictions: Option<Vec<String>>,
    #[allow(dead_code)]
    last_updated_at: Option<String>,
}

fn parse_terms_frontmatter(raw: &str) -> Result<(Option<TermsFrontmatter>, String)> {
    const OPEN: &str = "---\n";
    const CLOSE: &str = "\n---\n";

    if !raw.starts_with(OPEN) {
        return Ok((None, raw.to_string()));
    }

    let Some(close_pos) = raw[OPEN.len()..].find(CLOSE) else {
        return Err(anyhow!(
            "TERMS_CONTENT has opening frontmatter marker but missing closing marker"
        ));
    };

    let yaml_start = OPEN.len();
    let yaml_end = OPEN.len() + close_pos;
    let body_start = yaml_end + CLOSE.len();

    let frontmatter_yaml = &raw[yaml_start..yaml_end];
    let frontmatter: TermsFrontmatter = serde_yaml::from_str(frontmatter_yaml)
        .map_err(|e| anyhow!("Failed to parse terms frontmatter YAML: {}", e))?;
    let body = raw[body_start..].trim_start_matches('\n').to_string();

    Ok((Some(frontmatter), body))
}

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Clone, Debug)]
pub struct Config {
    // Server
    pub server_host: String,
    pub server_port: u16,
    pub oauth_redirect_base_url: String,
    pub frontend_app_url: String,
    pub cors_allowed_origins: String,

    // Environment
    pub environment: String, // "production", "staging", "development"
    pub require_cloudflare_headers: bool, // Force CF header validation

    // Database & Redis
    pub database_url: String,
    pub valkey_url: String,

    // Security & Auth
    pub jwt_secret: String,
    pub access_token_ttl_secs: usize,
    pub refresh_token_ttl_days: u16,
    pub auth_require_username_on_google_signup: bool,
    pub terms_current_version: String,
    pub terms_url: Option<String>,
    pub terms_content: Option<String>,
    pub terms_content_type: Option<String>,
    pub terms_content_sha256: Option<String>,
    pub terms_effective_at: Option<String>,
    pub terms_jurisdictions: Vec<String>,
    pub terms_require_version_match: bool,
    pub terms_legal_contact_email: Option<String>,
    pub terms_abuse_contact_email: Option<String>,
    pub read_only_mode_enabled: bool,
    pub maintenance_mode_enabled: bool,
    pub username_reserved_values: String,
    pub admin_api_enabled: bool,
    pub admin_jwt_issuer: Option<String>,
    pub admin_jwt_audience: Option<String>,
    pub admin_jwt_public_keys: HashMap<String, String>,
    pub admin_jwt_max_ttl_secs: usize,
    pub admin_jwt_clock_skew_secs: usize,
    pub admin_replay_protection_enabled: bool,
    pub admin_allowed_ip_cidrs: String,

    // OAuth Provider
    pub google_client_id: String,
    pub google_client_secret: String,
    pub mobile_app_scheme: String,

    // Object Storage
    pub minio_endpoint: String,
    pub minio_access_key: String,
    pub minio_secret_key: String,
    pub minio_bucket_videos: String,
    pub minio_bucket_raw: String,
    pub minio_public_endpoint: String,
    pub presigned_url_expiry_secs: u64,

    // File Upload Limits
    pub max_file_size_bytes: i64,
    pub limit_upload_bytes_hourly: i64,
    pub upload_size_tolerance_bytes: u64,
    pub min_video_size_bytes: u64,

    // Feed Configuration
    pub feed_page_size: u64,
    pub feed_cache_ttl_secs: usize,
    pub limit_feed_rpm: u64,

    // Rate Limiting
    pub rate_limit_window_secs: usize,
    pub ip_rate_limit_rpm: u64,
    pub like_actions_rpm_limit: u64,
    pub like_actions_window_secs: usize,
    pub username_signup_rpm_per_ip: u64,
    pub username_signup_attempts_per_ticket: u64,
    pub username_update_rpm_per_user: u64,
    pub read_only_status_rpm_per_user: u64,
    pub maintenance_status_rpm_per_user: u64,
    pub onboarding_status_rpm_per_user: u64,
    pub onboarding_complete_rpm_per_user: u64,
    pub report_create_rpm_per_user: u64,
    pub report_create_rpm_per_ip: u64,
    pub report_details_max_chars: usize,
    pub report_reason_max_count: usize,
    pub auto_quarantine_enabled: bool,
    pub auto_quarantine_window_secs: u64,
    pub auto_quarantine_severe_distinct_reporters: u64,
    pub security_events_retention_days: i64,
    pub security_events_purge_interval_secs: u64,
    pub abuse_report_retention_days: i64,
    pub abuse_report_purge_interval_secs: u64,
    pub ban_user_cache_negative_ttl_secs: usize,
    pub ban_user_cache_permanent_ttl_secs: usize,
    pub ban_ip_cache_negative_ttl_secs: usize,
    pub ban_ip_cache_permanent_ttl_secs: usize,
    pub ban_ip_verdict_ttl_secs: usize,

    // OTC Rate Limiting
    pub otc_rate_limit_max_attempts: u32,
    pub otc_rate_limit_window_seconds: u64,

    // Search Configuration
    pub search_max_tokens: usize,
    pub search_max_token_length: usize,
    pub search_max_query_chars: usize,
    pub search_timeout_secs: u64,
    pub search_cache_ttl_secs: usize,
    pub search_rpm_limit: u64,

    // Keyboard Extension
    pub extension_token_ttl_secs: usize,
    pub keyboard_search_rpm: u64,
    pub keyboard_search_max_limit: u64,
    pub keyboard_send_ticket_rpm: u64,
    pub keyboard_daily_send_cap: u64,
    pub keyboard_nonce_ttl_secs: usize,
    pub keyboard_ticket_ttl_secs: u64,
    pub keyboard_media_url_ttl_secs: u64,

    // Worker
    pub draft_video_cleanup_hours: u16,
    pub worker_retry_max_attempts: u8,
    pub worker_retry_backoff_base_secs: u64,
    pub ffmpeg_transcode_timeout_secs: u64,
    pub ffmpeg_thumbnail_timeout_secs: u64,

    // Realtime WebSocket
    pub video_ws_enabled: bool,
    pub video_ws_max_conn_per_user: usize,
    pub video_ws_max_conn_global: usize,
    pub video_ws_connect_rpm_per_ip: u64,
    pub video_ws_connect_rpm_per_user: u64,
    pub video_ws_send_buffer: usize,
    pub video_ws_heartbeat_secs: u64,
}

impl Config {
    pub fn sha256_hex(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn from_env() -> Result<Self> {
        dotenv().ok();

        let admin_api_enabled = env::var("ADMIN_API_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);
        let terms_current_version = env::var("TERMS_CURRENT_VERSION")
            .unwrap_or_else(|_| "v1".to_string())
            .trim()
            .to_string();
        if terms_current_version.is_empty() {
            return Err(anyhow!("TERMS_CURRENT_VERSION must be non-empty"));
        }
        let terms_url = env::var("TERMS_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let terms_require_version_match = env::var("TERMS_REQUIRE_VERSION_MATCH")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true);
        let terms_legal_contact_email = env::var("TERMS_LEGAL_CONTACT_EMAIL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let terms_abuse_contact_email = env::var("TERMS_ABUSE_CONTACT_EMAIL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let terms_jurisdictions_env = {
            let parsed = parse_csv_list(
                &env::var("TERMS_JURISDICTIONS").unwrap_or_else(|_| "US,TR,GLOBAL".to_string()),
            );
            if parsed.is_empty() {
                vec!["US".to_string(), "TR".to_string(), "GLOBAL".to_string()]
            } else {
                parsed
            }
        };
        let terms_content_inline = env::var("TERMS_CONTENT")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let terms_content_file_path = env::var("TERMS_CONTENT_FILE_PATH")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        if terms_content_inline.is_some() && terms_content_file_path.is_some() {
            return Err(anyhow!(
                "Set either TERMS_CONTENT or TERMS_CONTENT_FILE_PATH, not both"
            ));
        }
        let terms_content_raw = match (terms_content_inline, terms_content_file_path) {
            (Some(content), None) => Some(content),
            (None, Some(path)) => Some(std::fs::read_to_string(&path).map_err(|e| {
                anyhow!("Failed to read TERMS_CONTENT_FILE_PATH '{}': {}", path, e)
            })?),
            (None, None) => None,
            (Some(_), Some(_)) => unreachable!(),
        };
        if terms_url.is_none() && terms_content_raw.is_none() {
            return Err(anyhow!(
                "Either TERMS_URL or embedded terms content (TERMS_CONTENT/TERMS_CONTENT_FILE_PATH) must be configured"
            ));
        }
        let (terms_frontmatter, terms_content) = if let Some(content) = terms_content_raw {
            let (frontmatter, body) = parse_terms_frontmatter(&content)?;
            (frontmatter, Some(body))
        } else {
            (None, None)
        };
        if terms_require_version_match
            && let Some(ref frontmatter) = terms_frontmatter
            && let Some(ref frontmatter_version) = frontmatter.version
            && frontmatter_version.trim() != terms_current_version
        {
            return Err(anyhow!(
                "Terms frontmatter version '{}' does not match TERMS_CURRENT_VERSION '{}'",
                frontmatter_version,
                terms_current_version
            ));
        }
        let terms_effective_at = terms_frontmatter
            .as_ref()
            .and_then(|fm| fm.effective_at.as_ref().map(|v| v.trim().to_string()))
            .filter(|v| !v.is_empty());
        let terms_jurisdictions = terms_frontmatter
            .as_ref()
            .and_then(|fm| fm.jurisdictions.clone())
            .filter(|items| !items.is_empty())
            .unwrap_or(terms_jurisdictions_env);
        let terms_content_type = if terms_content.is_some() {
            Some(
                env::var("TERMS_CONTENT_TYPE")
                    .unwrap_or_else(|_| "text/markdown".to_string())
                    .trim()
                    .to_string(),
            )
        } else {
            None
        };
        if let Some(ref content_type) = terms_content_type
            && content_type.is_empty()
        {
            return Err(anyhow!(
                "TERMS_CONTENT_TYPE must be non-empty when embedded terms content is configured"
            ));
        }
        let terms_content_sha256 = terms_content
            .as_ref()
            .map(|content| Self::sha256_hex(content));
        let admin_jwt_issuer = env::var("ADMIN_JWT_ISSUER")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let admin_jwt_audience = env::var("ADMIN_JWT_AUDIENCE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let admin_jwt_public_keys_raw = env::var("ADMIN_JWT_PUBLIC_KEYS_JSON").unwrap_or_default();
        let admin_jwt_public_keys: HashMap<String, String> =
            if admin_jwt_public_keys_raw.trim().is_empty() {
                HashMap::new()
            } else {
                serde_json::from_str(&admin_jwt_public_keys_raw).map_err(|e| {
                    anyhow!("ADMIN_JWT_PUBLIC_KEYS_JSON must be valid JSON map: {}", e)
                })?
            };
        let admin_jwt_max_ttl_secs: usize = env::var("ADMIN_JWT_MAX_TTL_SECS")
            .unwrap_or_else(|_| "300".to_string())
            .parse()?;
        let admin_jwt_clock_skew_secs: usize = env::var("ADMIN_JWT_CLOCK_SKEW_SECS")
            .unwrap_or_else(|_| "60".to_string())
            .parse()?;
        let admin_replay_protection_enabled = env::var("ADMIN_REPLAY_PROTECTION_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true);
        let admin_allowed_ip_cidrs = env::var("ADMIN_ALLOWED_IP_CIDRS").unwrap_or_default();

        if admin_api_enabled {
            if admin_jwt_issuer.is_none() {
                return Err(anyhow!(
                    "ADMIN_JWT_ISSUER must be set when ADMIN_API_ENABLED=true"
                ));
            }
            if admin_jwt_audience.is_none() {
                return Err(anyhow!(
                    "ADMIN_JWT_AUDIENCE must be set when ADMIN_API_ENABLED=true"
                ));
            }
            if admin_jwt_public_keys.is_empty() {
                return Err(anyhow!(
                    "ADMIN_JWT_PUBLIC_KEYS_JSON must define at least one key when ADMIN_API_ENABLED=true"
                ));
            }
        }

        Ok(Self {
            // Server
            server_host: env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            server_port: env::var("SERVER_PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .expect("SERVER_PORT must be a number"),
            oauth_redirect_base_url: env::var("OAUTH_REDIRECT_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:80".to_string())
                .trim_end_matches('/')
                .to_string(),
            frontend_app_url: env::var("FRONTEND_APP_URL")
                .unwrap_or_else(|_| "http://localhost:3000".to_string())
                .trim_end_matches('/')
                .to_string(),
            cors_allowed_origins: env::var("CORS_ALLOWED_ORIGINS").unwrap_or_default(),

            // Environment
            environment: env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string()),
            require_cloudflare_headers: env::var("REQUIRE_CLOUDFLARE_HEADERS")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),

            // Database & Redis
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            valkey_url: env::var("VALKEY_URL").expect("VALKEY_URL must be set"),

            // Security & Auth
            jwt_secret: env::var("JWT_SECRET").expect("JWT_SECRET must be set"),
            access_token_ttl_secs: env::var("ACCESS_TOKEN_TTL_SECS")
                .unwrap_or_else(|_| "900".to_string()) // 15 minutes
                .parse()?,
            refresh_token_ttl_days: env::var("REFRESH_TOKEN_TTL_DAYS")
                .unwrap_or_else(|_| "14".to_string())
                .parse()?,
            auth_require_username_on_google_signup: env::var(
                "AUTH_REQUIRE_USERNAME_ON_GOOGLE_SIGNUP",
            )
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true),
            terms_current_version,
            terms_url,
            terms_content,
            terms_content_type,
            terms_content_sha256,
            terms_effective_at,
            terms_jurisdictions,
            terms_require_version_match,
            terms_legal_contact_email,
            terms_abuse_contact_email,
            read_only_mode_enabled: env::var("READ_ONLY_MODE_ENABLED")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            maintenance_mode_enabled: env::var("MAINTENANCE_MODE_ENABLED")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            username_reserved_values: env::var("USERNAME_RESERVED_VALUES").unwrap_or_default(),
            admin_api_enabled,
            admin_jwt_issuer,
            admin_jwt_audience,
            admin_jwt_public_keys,
            admin_jwt_max_ttl_secs,
            admin_jwt_clock_skew_secs,
            admin_replay_protection_enabled,
            admin_allowed_ip_cidrs,

            // OAuth Provider
            google_client_id: env::var("GOOGLE_CLIENT_ID").expect("GOOGLE_CLIENT_ID must be set"),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET")
                .expect("GOOGLE_CLIENT_SECRET must be set"),
            mobile_app_scheme: env::var("MOBILE_APP_SCHEME")
                .unwrap_or_else(|_| "memegy://".to_string()),

            // Object Storage
            minio_endpoint: env::var("MINIO_ENDPOINT").expect("MINIO_ENDPOINT must be set"),
            minio_access_key: env::var("MINIO_ROOT_USER").expect("MINIO_ROOT_USER must be set"),
            minio_secret_key: env::var("MINIO_ROOT_PASSWORD")
                .expect("MINIO_ROOT_PASSWORD must be set"),
            minio_bucket_videos: env::var("MINIO_BUCKET_VIDEOS")
                .expect("MINIO_BUCKET_VIDEOS must be set"),
            minio_bucket_raw: env::var("MINIO_BUCKET_RAW").expect("MINIO_BUCKET_RAW must be set"),
            minio_public_endpoint: env::var("MINIO_PUBLIC_ENDPOINT").unwrap_or_else(|_| {
                env::var("MINIO_ENDPOINT").expect("MINIO_ENDPOINT must be set")
            }),
            presigned_url_expiry_secs: env::var("PRESIGNED_URL_EXPIRY_SECS")
                .unwrap_or_else(|_| "3600".to_string()) // 1 hour
                .parse()?,

            // File Upload Limits
            max_file_size_bytes: env::var("MAX_FILE_SIZE_BYTES")
                .unwrap_or_else(|_| "33554432".to_string()) // 32 MB
                .parse()?,
            limit_upload_bytes_hourly: env::var("LIMIT_UPLOAD_BYTES_HOURLY")
                .unwrap_or_else(|_| "268435456".to_string()) // 256 MB
                .parse()?,
            upload_size_tolerance_bytes: env::var("UPLOAD_SIZE_TOLERANCE_BYTES")
                .unwrap_or_else(|_| "0".to_string())
                .parse()?,
            min_video_size_bytes: env::var("MIN_VIDEO_SIZE_BYTES")
                .unwrap_or_else(|_| "1024".to_string())
                .parse()?,

            // Feed Configuration
            feed_page_size: env::var("FEED_PAGE_SIZE")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,
            feed_cache_ttl_secs: env::var("FEED_CACHE_TTL_SECS")
                .unwrap_or_else(|_| "300".to_string()) // 5 minutes
                .parse()?,
            limit_feed_rpm: env::var("LIMIT_FEED_RPM")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,

            // Rate Limiting
            rate_limit_window_secs: env::var("RATE_LIMIT_WINDOW_SECS")
                .unwrap_or_else(|_| "3600".to_string()) // 1 hour
                .parse()?,
            ip_rate_limit_rpm: env::var("IP_RATE_LIMIT_RPM")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
            like_actions_rpm_limit: env::var("LIKE_ACTIONS_RPM_LIMIT")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            like_actions_window_secs: env::var("LIKE_ACTIONS_WINDOW_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            username_signup_rpm_per_ip: env::var("USERNAME_SIGNUP_RPM_PER_IP")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,
            username_signup_attempts_per_ticket: env::var("USERNAME_SIGNUP_ATTEMPTS_PER_TICKET")
                .unwrap_or_else(|_| "10".to_string())
                .parse()?,
            username_update_rpm_per_user: env::var("USERNAME_UPDATE_RPM_PER_USER")
                .unwrap_or_else(|_| "5".to_string())
                .parse()?,
            read_only_status_rpm_per_user: env::var("READ_ONLY_STATUS_RPM_PER_USER")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,
            maintenance_status_rpm_per_user: env::var("MAINTENANCE_STATUS_RPM_PER_USER")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,
            onboarding_status_rpm_per_user: env::var("ONBOARDING_STATUS_RPM_PER_USER")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
            onboarding_complete_rpm_per_user: env::var("ONBOARDING_COMPLETE_RPM_PER_USER")
                .unwrap_or_else(|_| "10".to_string())
                .parse()?,
            report_create_rpm_per_user: env::var("REPORT_CREATE_RPM_PER_USER")
                .unwrap_or_else(|_| "10".to_string())
                .parse()?,
            report_create_rpm_per_ip: env::var("REPORT_CREATE_RPM_PER_IP")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,
            report_details_max_chars: env::var("REPORT_DETAILS_MAX_CHARS")
                .unwrap_or_else(|_| "2000".to_string())
                .parse()?,
            report_reason_max_count: env::var("REPORT_REASON_MAX_COUNT")
                .unwrap_or_else(|_| "5".to_string())
                .parse()?,
            auto_quarantine_enabled: env::var("AUTO_QUARANTINE_ENABLED")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
            auto_quarantine_window_secs: env::var("AUTO_QUARANTINE_WINDOW_SECS")
                .unwrap_or_else(|_| "1800".to_string())
                .parse()?,
            auto_quarantine_severe_distinct_reporters: env::var(
                "AUTO_QUARANTINE_SEVERE_DISTINCT_REPORTERS",
            )
            .unwrap_or_else(|_| "2".to_string())
            .parse()?,
            security_events_retention_days: env::var("SECURITY_EVENTS_RETENTION_DAYS")
                .unwrap_or_else(|_| "180".to_string())
                .parse()?,
            security_events_purge_interval_secs: env::var("SECURITY_EVENTS_PURGE_INTERVAL_SECS")
                .unwrap_or_else(|_| "86400".to_string())
                .parse()?,
            abuse_report_retention_days: env::var("ABUSE_REPORT_RETENTION_DAYS")
                .unwrap_or_else(|_| "365".to_string())
                .parse()?,
            abuse_report_purge_interval_secs: env::var("ABUSE_REPORT_PURGE_INTERVAL_SECS")
                .unwrap_or_else(|_| "86400".to_string())
                .parse()?,
            ban_user_cache_negative_ttl_secs: env::var("BAN_USER_CACHE_NEGATIVE_TTL_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            ban_user_cache_permanent_ttl_secs: env::var("BAN_USER_CACHE_PERMANENT_TTL_SECS")
                .unwrap_or_else(|_| "21600".to_string())
                .parse()?,
            ban_ip_cache_negative_ttl_secs: env::var("BAN_IP_CACHE_NEGATIVE_TTL_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            ban_ip_cache_permanent_ttl_secs: env::var("BAN_IP_CACHE_PERMANENT_TTL_SECS")
                .unwrap_or_else(|_| "21600".to_string())
                .parse()?,
            ban_ip_verdict_ttl_secs: env::var("BAN_IP_VERDICT_TTL_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,

            // OTC Rate Limiting
            otc_rate_limit_max_attempts: env::var("OTC_RATE_LIMIT_MAX_ATTEMPTS")
                .unwrap_or_else(|_| "5".to_string())
                .parse()?,
            otc_rate_limit_window_seconds: env::var("OTC_RATE_LIMIT_WINDOW_SECONDS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,

            // Search Configuration
            search_max_tokens: env::var("SEARCH_MAX_TOKENS")
                .unwrap_or_else(|_| "50".to_string())
                .parse()?,
            search_max_token_length: env::var("SEARCH_MAX_TOKEN_LENGTH")
                .unwrap_or_else(|_| "50".to_string())
                .parse()?,
            search_max_query_chars: env::var("SEARCH_MAX_QUERY_CHARS")
                .unwrap_or_else(|_| "200".to_string())
                .parse()?,
            search_timeout_secs: env::var("SEARCH_TIMEOUT_SECS")
                .unwrap_or_else(|_| "5".to_string())
                .parse()?,
            search_cache_ttl_secs: env::var("SEARCH_CACHE_TTL_SECS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()?,
            search_rpm_limit: env::var("SEARCH_RPM_LIMIT")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,

            // Keyboard Extension
            extension_token_ttl_secs: env::var("EXTENSION_TOKEN_TTL_SECS")
                .unwrap_or_else(|_| "600".to_string()) // 10 minutes
                .parse()?,
            keyboard_search_rpm: env::var("KEYBOARD_SEARCH_RPM")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            keyboard_search_max_limit: env::var("KEYBOARD_SEARCH_MAX_LIMIT")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,
            keyboard_send_ticket_rpm: env::var("KEYBOARD_SEND_TICKET_RPM")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
            keyboard_daily_send_cap: env::var("KEYBOARD_DAILY_SEND_CAP")
                .unwrap_or_else(|_| "100".to_string())
                .parse()?,
            keyboard_nonce_ttl_secs: env::var("KEYBOARD_NONCE_TTL_SECS")
                .unwrap_or_else(|_| "300".to_string())
                .parse()?,
            keyboard_ticket_ttl_secs: env::var("KEYBOARD_TICKET_TTL_SECS")
                .unwrap_or_else(|_| "45".to_string())
                .parse()?,
            keyboard_media_url_ttl_secs: env::var("KEYBOARD_MEDIA_URL_TTL_SECS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,

            // Worker
            draft_video_cleanup_hours: env::var("DRAFT_VIDEO_CLEANUP_HOURS")
                .unwrap_or_else(|_| "2".to_string())
                .parse()?,
            worker_retry_max_attempts: env::var("WORKER_RETRY_MAX_ATTEMPTS")
                .unwrap_or_else(|_| "3".to_string())
                .parse()?,
            worker_retry_backoff_base_secs: env::var("WORKER_RETRY_BACKOFF_BASE_SECS")
                .unwrap_or_else(|_| "2".to_string())
                .parse()?,
            ffmpeg_transcode_timeout_secs: env::var("FFMPEG_TRANSCODE_TIMEOUT_SECS")
                .unwrap_or_else(|_| "180".to_string())
                .parse()?,
            ffmpeg_thumbnail_timeout_secs: env::var("FFMPEG_THUMBNAIL_TIMEOUT_SECS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,

            // Realtime WebSocket
            video_ws_enabled: env::var("VIDEO_WS_ENABLED")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
            video_ws_max_conn_per_user: env::var("VIDEO_WS_MAX_CONN_PER_USER")
                .unwrap_or_else(|_| "3".to_string())
                .parse()?,
            video_ws_max_conn_global: env::var("VIDEO_WS_MAX_CONN_GLOBAL")
                .unwrap_or_else(|_| "5000".to_string())
                .parse()?,
            video_ws_connect_rpm_per_ip: env::var("VIDEO_WS_CONNECT_RPM_PER_IP")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
            video_ws_connect_rpm_per_user: env::var("VIDEO_WS_CONNECT_RPM_PER_USER")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            video_ws_send_buffer: env::var("VIDEO_WS_SEND_BUFFER")
                .unwrap_or_else(|_| "64".to_string())
                .parse()?,
            video_ws_heartbeat_secs: env::var("VIDEO_WS_HEARTBEAT_SECS")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_terms_frontmatter;

    #[test]
    fn parse_terms_frontmatter_extracts_metadata_and_body() {
        let raw = r#"---
version: v7
effective_at: "2026-02-20"
jurisdictions:
  - US
  - TR
---
# Terms

Body
"#;

        let (frontmatter, body) = parse_terms_frontmatter(raw).expect("frontmatter should parse");
        let fm = frontmatter.expect("frontmatter should exist");
        assert_eq!(fm.version.as_deref(), Some("v7"));
        assert_eq!(fm.effective_at.as_deref(), Some("2026-02-20"));
        assert_eq!(fm.jurisdictions.unwrap_or_default(), vec!["US", "TR"]);
        assert_eq!(body.trim(), "# Terms\n\nBody");
    }

    #[test]
    fn parse_terms_frontmatter_returns_raw_when_no_frontmatter() {
        let raw = "# Terms\n\nBody";
        let (frontmatter, body) = parse_terms_frontmatter(raw).expect("parse should succeed");
        assert!(frontmatter.is_none());
        assert_eq!(body, raw);
    }
}
