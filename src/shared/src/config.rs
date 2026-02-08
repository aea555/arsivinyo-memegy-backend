use anyhow::{Result, anyhow};
use dotenvy::dotenv;
use std::collections::HashMap;
use std::env;

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
    pub fn from_env() -> Result<Self> {
        dotenv().ok();

        let admin_api_enabled = env::var("ADMIN_API_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);
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
