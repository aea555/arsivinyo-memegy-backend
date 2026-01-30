use anyhow::Result;
use dotenvy::dotenv;
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub server_host: String,
    pub server_port: u16,
    pub oauth_redirect_base_url: String,
    pub database_url: String,
    pub valkey_url: String,
    pub jwt_secret: String,
    pub cors_allowed_origins: String,
    pub minio_endpoint: String,
    pub minio_access_key: String,
    pub minio_secret_key: String,
    pub minio_bucket_videos: String,
    pub minio_bucket_raw: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub limit_upload_bytes_hourly: u64,
    pub limit_feed_rpm: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenv().ok();

        Ok(Self {
            server_host: env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            server_port: env::var("SERVER_PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .expect("SERVER_PORT must be a number"),
            oauth_redirect_base_url: env::var("OAUTH_REDIRECT_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3000".to_string())
                .trim_end_matches('/')
                .to_string(),
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            valkey_url: env::var("VALKEY_URL").expect("VALKEY_URL must be set"),
            jwt_secret: env::var("JWT_SECRET").expect("JWT_SECRET must be set"),
            cors_allowed_origins: env::var("CORS_ALLOWED_ORIGINS").unwrap_or_default(),
            minio_endpoint: env::var("MINIO_ENDPOINT").expect("MINIO_ENDPOINT must be set"),
            minio_access_key: env::var("MINIO_ROOT_USER").expect("MINIO_ROOT_USER must be set"),
            minio_secret_key: env::var("MINIO_ROOT_PASSWORD")
                .expect("MINIO_ROOT_PASSWORD must be set"),
            minio_bucket_videos: env::var("MINIO_BUCKET_VIDEOS")
                .expect("MINIO_BUCKET_VIDEOS must be set"),
            minio_bucket_raw: env::var("MINIO_BUCKET_RAW").expect("MINIO_BUCKET_RAW must be set"),
            google_client_id: env::var("GOOGLE_CLIENT_ID").expect("GOOGLE_CLIENT_ID must be set"),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET")
                .expect("GOOGLE_CLIENT_SECRET must be set"),
            limit_upload_bytes_hourly: env::var("LIMIT_UPLOAD_BYTES_HOURLY")
                .unwrap_or_else(|_| "52428800".to_string())
                .parse()?,
            limit_feed_rpm: env::var("LIMIT_FEED_RPM")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
        })
    }
}
