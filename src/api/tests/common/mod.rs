use api::{
    auth::revocation::TokenRevocationService, cache::feed_cache::FeedCacheService,
    cache::otc_cache::OtcCacheService, create_router, realtime::hub::RealtimeHub,
    services::rate_limiter::RateLimiter, state::AppState,
};
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use shared::{config::Config, queue::QueueService, storage::StorageBackend};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::{ContainerAsync, runners::AsyncRunner};
use testcontainers_modules::{postgres::Postgres, redis::Redis};
use tokio::net::TcpListener;

pub struct TestApp {
    pub address: String,
    pub db: sea_orm::DatabaseConnection, // Exposed for assertions
    pub _config: Config,                 // Keep alive / potential use

    // Containers kept alive
    pub _pg_container: ContainerAsync<Postgres>,
    pub _redis_container: ContainerAsync<Redis>,
}

#[derive(Clone)]
pub struct MockStorage;

#[async_trait::async_trait]
impl StorageBackend for MockStorage {
    async fn delete_file(&self, _bucket: &str, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn generate_presigned_put(
        &self,
        _bucket: &str,
        _key: &str,
        _expires_in: Duration,
    ) -> anyhow::Result<String> {
        Ok("http://mock/put".to_string())
    }
    async fn generate_presigned_get(
        &self,
        _bucket: &str,
        _key: &str,
        _expires_in: Duration,
    ) -> anyhow::Result<String> {
        Ok("http://mock/get".to_string())
    }
    async fn file_exists(&self, _bucket: &str, _key: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
    async fn get_file_size(&self, _bucket: &str, _key: &str) -> anyhow::Result<u64> {
        Ok(1024) // Mock size matching test expectations
    }
    async fn download_file(
        &self,
        _bucket: &str,
        _key: &str,
        _dest_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        // Create empty file
        tokio::fs::File::create(_dest_path).await?;
        Ok(())
    }
    async fn upload_file(
        &self,
        _bucket: &str,
        _key: &str,
        _src_path: &std::path::Path,
        _content_type: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

pub async fn spawn_app() -> TestApp {
    // 1. Start Containers
    let pg_container: ContainerAsync<Postgres> = Postgres::default()
        .start()
        .await
        .expect("Failed to start Postgres");
    let redis_container: ContainerAsync<Redis> = Redis::default()
        .start()
        .await
        .expect("Failed to start Redis");

    let db_host_port = pg_container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to get PG port");
    let db_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        db_host_port
    );

    let redis_host_port = redis_container
        .get_host_port_ipv4(6379)
        .await
        .expect("Failed to get Redis port");
    let redis_url = format!("redis://127.0.0.1:{}", redis_host_port);

    // 2. Run Migrations
    let db = Database::connect(&db_url)
        .await
        .expect("Failed to connect to test DB");
    migration::Migrator::up(&db, None)
        .await
        .expect("Failed to run migrations");

    // 3. Config
    let config = Config {
        server_host: "127.0.0.1".to_string(),
        server_port: 0,
        oauth_redirect_base_url: "http://localhost".to_string(),
        frontend_app_url: "http://localhost".to_string(),
        cors_allowed_origins: "".to_string(),
        environment: "test".to_string(),
        require_cloudflare_headers: false,
        database_url: db_url.clone(),
        valkey_url: redis_url.clone(), // Using Redis URL for Valkey
        jwt_secret: "test_secret_key_needs_to_be_long_enough".to_string(),
        access_token_ttl_secs: 900,
        refresh_token_ttl_days: 14,
        google_client_id: "test_client_id".to_string(),
        google_client_secret: "test_client_secret".to_string(),
        minio_endpoint: "http://mock".to_string(),
        minio_public_endpoint: "http://mock".to_string(),
        minio_access_key: "minio".to_string(),
        minio_secret_key: "minio123".to_string(),
        minio_bucket_videos: "videos".to_string(),
        minio_bucket_raw: "raw".to_string(),
        presigned_url_expiry_secs: 3600,
        max_file_size_bytes: 1024 * 1024 * 10,
        limit_upload_bytes_hourly: 1024 * 1024 * 100,
        otc_rate_limit_max_attempts: 5,
        otc_rate_limit_window_seconds: 60,
        feed_page_size: 20,
        feed_cache_ttl_secs: 60,
        limit_feed_rpm: 100,
        rate_limit_window_secs: 3600,
        ip_rate_limit_rpm: 100,
        like_actions_rpm_limit: 60,
        like_actions_window_secs: 60,
        search_max_tokens: 50,
        search_max_token_length: 50,
        search_max_query_chars: 200,
        search_timeout_secs: 5,
        search_cache_ttl_secs: 10,
        search_rpm_limit: 30,
        draft_video_cleanup_hours: 24,
        worker_retry_max_attempts: 3,
        worker_retry_backoff_base_secs: 2,
        mobile_app_scheme: "memegy://".to_string(),
        video_ws_enabled: true,
        video_ws_max_conn_per_user: 3,
        video_ws_max_conn_global: 5000,
        video_ws_connect_rpm_per_ip: 30,
        video_ws_connect_rpm_per_user: 60,
        video_ws_send_buffer: 64,
        video_ws_heartbeat_secs: 20,
    };

    // 4. Queues & Services
    let queue = QueueService::new(&config).expect("Failed to init queue");
    let storage = Arc::new(MockStorage);
    let token_revocation = TokenRevocationService::new(queue.clone());
    let feed_cache = FeedCacheService::new(queue.clone());
    let otc_cache = OtcCacheService::new(queue.clone());
    let rate_limiter = RateLimiter::new(queue.clone());
    let otc_rate_limiter = api::middleware::rate_limit::RateLimiter::new(5, 60);

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config.clone()),
        storage,
        queue,
        token_revocation,
        feed_cache,
        otc_cache,
        rate_limiter,
        otc_rate_limiter,
        realtime_hub: RealtimeHub::new(
            config.video_ws_max_conn_per_user,
            config.video_ws_max_conn_global,
            config.video_ws_send_buffer,
        ),
    };

    // 5. App Router
    let app = create_router(state);

    // 6. Bind to random port
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind port");
    let port = listener.local_addr().unwrap().port();
    let address = format!("http://127.0.0.1:{}", port);

    // 7. Spawn server in background with ConnectInfo for client IP
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });

    TestApp {
        address,
        db,
        _config: config,
        _pg_container: pg_container,
        _redis_container: redis_container,
    }
}

impl TestApp {
    pub async fn login_as_dev(&self, username: &str, email: &str) -> String {
        let client = reqwest::Client::new();
        let response = client
            .post(&format!("{}/auth/dev/login", self.address))
            .json(&serde_json::json!({
                "username": username,
                "email": email
            }))
            .send()
            .await
            .expect("Login request failed");

        if !response.status().is_success() {
            panic!(
                "Login failed: status={}, body={}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }

        let body: serde_json::Value = response.json().await.expect("Failed to parse json");
        body["access_token"]
            .as_str()
            .expect("Token missing")
            .to_string()
    }

    pub async fn init_upload(
        &self,
        token: &str,
        filename: &str,
        size_bytes: i64,
    ) -> (String, String) {
        let client = reqwest::Client::new();
        let response = client
            .post(&format!("{}/videos/init", self.address))
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({
                "filename": filename,
                "size_bytes": size_bytes
            }))
            .send()
            .await
            .expect("Init upload request failed");

        if !response.status().is_success() {
            panic!(
                "Init upload failed: status={}, body={}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }

        let body: serde_json::Value = response.json().await.expect("Failed to parse json");
        let video_id = body["video_id"]
            .as_str()
            .expect("video_id missing")
            .to_string();
        let upload_url = body["upload_url"]
            .as_str()
            .expect("upload_url missing")
            .to_string();
        (video_id, upload_url)
    }

    pub async fn confirm_upload(&self, token: &str, video_id: &str) {
        let client = reqwest::Client::new();
        let response = client
            .post(&format!("{}/videos/{}/confirm", self.address, video_id))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .expect("Confirm upload request failed");

        if !response.status().is_success() {
            panic!(
                "Confirm upload failed: status={}, body={}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }
    }

    /// Helper to directly insert a published video into the DB for testing feeds/deletion
    pub async fn create_dummy_video(&self, user_id: uuid::Uuid, is_anonymous: bool) -> uuid::Uuid {
        use sea_orm::{ActiveModelTrait, Set};
        use shared::entities::videos;

        let video_id = uuid::Uuid::new_v4();
        let video = videos::ActiveModel {
            id: Set(video_id),
            user_id: Set(user_id),
            title: Set(Some(format!("Test Video {}", video_id))),
            description: Set(Some("Description".to_string())),
            s3_bucket: Set("raw".to_string()),
            s3_key: Set(format!("{}/{}.mp4", user_id, video_id)),
            status: Set("PUBLISHED".to_string()), // Directly published
            size_bytes: Set(1024),
            like_count: Set(0),
            is_anonymous: Set(is_anonymous),
            deleted_at: Set(None),
            created_at: Set(chrono::Utc::now().into()),
            updated_at: Set(chrono::Utc::now().into()),
        };

        video
            .insert(&self.db)
            .await
            .expect("Failed to insert dummy video");
        video_id
    }
}
