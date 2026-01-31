use anyhow::Result;
use dotenvy::dotenv;
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    // Server
    pub server_host: String,
    pub server_port: u16,
    pub oauth_redirect_base_url: String,
    pub cors_allowed_origins: String,

    // Database & Redis
    pub database_url: String,
    pub valkey_url: String,

    // Security & Auth
    pub jwt_secret: String,
    pub access_token_ttl_secs: usize,
    pub refresh_token_ttl_days: u16,

    // OAuth Provider
    pub google_client_id: String,
    pub google_client_secret: String,

    // Object Storage
    pub minio_endpoint: String,
    pub minio_access_key: String,
    pub minio_secret_key: String,
    pub minio_bucket_videos: String,
    pub minio_bucket_raw: String,
    pub presigned_url_expiry_secs: u64,

    // File Upload Limits
    pub max_file_size_bytes: i64,
    pub limit_upload_bytes_hourly: i64,

    // Feed Configuration
    pub feed_page_size: u64,
    pub feed_cache_ttl_secs: usize,
    pub limit_feed_rpm: u64,

    // Rate Limiting
    pub rate_limit_window_secs: usize,
    pub ip_rate_limit_rpm: u64,

    // Worker
    pub draft_video_cleanup_hours: u16,
    pub worker_retry_max_attempts: u8,
    pub worker_retry_backoff_base_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenv().ok();

        Ok(Self {
            // Server
            server_host: env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            server_port: env::var("SERVER_PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .expect("SERVER_PORT must be a number"),
            oauth_redirect_base_url: env::var("OAUTH_REDIRECT_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3000".to_string())
                .trim_end_matches('/')
                .to_string(),
            cors_allowed_origins: env::var("CORS_ALLOWED_ORIGINS").unwrap_or_default(),

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

            // OAuth Provider
            google_client_id: env::var("GOOGLE_CLIENT_ID").expect("GOOGLE_CLIENT_ID must be set"),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET")
                .expect("GOOGLE_CLIENT_SECRET must be set"),

            // Object Storage
            minio_endpoint: env::var("MINIO_ENDPOINT").expect("MINIO_ENDPOINT must be set"),
            minio_access_key: env::var("MINIO_ROOT_USER").expect("MINIO_ROOT_USER must be set"),
            minio_secret_key: env::var("MINIO_ROOT_PASSWORD")
                .expect("MINIO_ROOT_PASSWORD must be set"),
            minio_bucket_videos: env::var("MINIO_BUCKET_VIDEOS")
                .expect("MINIO_BUCKET_VIDEOS must be set"),
            minio_bucket_raw: env::var("MINIO_BUCKET_RAW").expect("MINIO_BUCKET_RAW must be set"),
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
        })
    }
}
