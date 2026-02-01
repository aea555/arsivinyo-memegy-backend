use api::{
    auth::revocation::TokenRevocationService, cache::feed_cache::FeedCacheService, create_router,
    services::rate_limiter::RateLimiter, state::AppState,
};
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use shared::{config::Config, queue::QueueService, storage::StorageBackend};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use testcontainers_modules::{postgres::Postgres, redis::Redis};
use tokio::net::TcpListener;

pub struct TestApp {
    pub address: String,
    pub db: sea_orm::DatabaseConnection, // Exposed for assertions
    pub config: Config,                  // Exposed for assertions

    // Containers kept alive
    pub pg_container: ContainerAsync<Postgres>,
    pub redis_container: ContainerAsync<Redis>,
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
        minio_access_key: "minio".to_string(),
        minio_secret_key: "minio123".to_string(),
        minio_bucket_videos: "videos".to_string(),
        minio_bucket_raw: "raw".to_string(),
        presigned_url_expiry_secs: 3600,
        max_file_size_bytes: 1024 * 1024 * 10,
        limit_upload_bytes_hourly: 1024 * 1024 * 100,
        feed_page_size: 20,
        feed_cache_ttl_secs: 60,
        limit_feed_rpm: 100,
        rate_limit_window_secs: 3600,
        ip_rate_limit_rpm: 100,
        search_max_tokens: 50,
        search_max_token_length: 50,
        search_max_query_chars: 200,
        search_timeout_secs: 5,
        search_cache_ttl_secs: 10,
        search_rpm_limit: 30,
        draft_video_cleanup_hours: 24,
        worker_retry_max_attempts: 3,
        worker_retry_backoff_base_secs: 2,
    };

    // 4. Queues & Services
    let queue = QueueService::new(&config).expect("Failed to init queue");
    let storage = Arc::new(MockStorage);
    let token_revocation = TokenRevocationService::new(queue.clone());
    let feed_cache = FeedCacheService::new(queue.clone());
    let rate_limiter = RateLimiter::new(queue.clone());

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config.clone()),
        storage,
        queue,
        token_revocation,
        feed_cache,
        rate_limiter,
    };

    // 5. App Router
    let app = create_router(state);

    // 6. Bind to random port
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind port");
    let port = listener.local_addr().unwrap().port();
    let address = format!("http://127.0.0.1:{}", port);

    // 7. Spawn server in background
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestApp {
        address,
        db,
        config,
        pg_container,
        redis_container,
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
}
