use anyhow::Result;
use sea_orm::{Database, ActiveModelTrait, Set, EntityTrait};
use shared::{
    config::Config,
    queue::{QueueService, VideoProcessJob},
    storage::StorageService,
    entities::videos,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("debug"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 2. Config
    let config = Config::from_env()?;

    // 3. Services
    let db = Database::connect(&config.database_url).await?;
    let _storage = StorageService::new(&config).await; // Need to download/upload
    let queue = QueueService::new(&config)?;

    tracing::info!("Worker started, waiting for jobs...");

    loop {
        match queue.pop_video_job().await {
            Ok(Some(job)) => {
                tracing::info!("Processing job: {:?}", job);
                if let Err(e) = process_video(&db, &job).await {
                    tracing::error!("Failed to process video {}: {:?}", job.video_id, e);
                    // Update status to FAILED
                    if let Ok(Some(v)) = videos::Entity::find_by_id(job.video_id).one(&db).await {
                        let mut active: videos::ActiveModel = v.into();
                        active.status = Set("FAILED".to_string());
                        let _ = active.update(&db).await;
                    }
                }
            }
            Ok(None) => {
                // Queue empty (brpop blocks, but if connection drops or timeout)
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(e) => {
                tracing::error!("Queue error: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn process_video(db: &sea_orm::DatabaseConnection, job: &VideoProcessJob) -> Result<()> {
    // TODO: 
    // 1. Download from S3 (StorageService needs `get_object`)
    // 2. FFmpeg (Compress, Thumb)
    // 3. Upload to S3 (Public Bucket)
    // 4. Update DB
    
    // MOCK implementation for now
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    
    // Update to PUBLISHED
    if let Some(v) = videos::Entity::find_by_id(job.video_id).one(db).await? {
        let mut active: videos::ActiveModel = v.into();
        active.status = Set("PUBLISHED".to_string());
        active.update(db).await?;
        tracing::info!("Video {} published successfully", job.video_id);
    }
    
    Ok(())
}
