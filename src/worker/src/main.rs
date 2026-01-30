use anyhow::{Result, Context};
use sea_orm::{Database, ActiveModelTrait, Set, EntityTrait};
use sea_orm_migration::prelude::*;
use shared::{
    config::Config,
    queue::{QueueService, VideoProcessJob},
    storage::StorageService,
    entities::videos,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tempfile::TempDir;
use std::process::Command;
use std::path::Path;

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
    
    // 4. Run Migrations 
    tracing::info!("Running database migrations...");
    migration::Migrator::up(&db, None).await?;
    tracing::info!("Database migrations completed");
    
    let storage = StorageService::new(&config).await;
    let queue = QueueService::new(&config)?;

    tracing::info!("Worker started, waiting for jobs...");

    loop {
        match queue.pop_video_job().await {
            Ok(Some(job)) => {
                tracing::info!("Processing job: {:?}", job);
                if let Err(e) = process_video(&db, &storage, &config, &job).await {
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

async fn process_video(
    db: &sea_orm::DatabaseConnection, 
    storage: &StorageService,
    config: &Config,
    job: &VideoProcessJob
) -> Result<()> {
    tracing::info!("Starting video processing for video_id={}", job.video_id);
    
    // 1. Fetch video metadata from database
    let video = videos::Entity::find_by_id(job.video_id)
        .one(db)
        .await?
        .context("Video not found in database")?;
    
    // 2. Create temporary working directory
    let temp_dir = TempDir::new().context("Failed to create temp directory")?;
    let input_path = temp_dir.path().join("input.mp4");
    let output_path = temp_dir.path().join("output.mp4");
    let thumb_path = temp_dir.path().join("thumb.jpg");
    
    tracing::info!("Downloading video from S3: bucket={}, key={}", video.s3_bucket, video.s3_key);
    
    // 3. Download raw video from S3
    storage.download_file(&video.s3_bucket, &video.s3_key, &input_path)
        .await
        .context("Failed to download video from S3")?;
    
    // 4. Compress video with FFmpeg
    tracing::info!("Compressing video with FFmpeg");
    compress_video(&input_path, &output_path)
        .context("FFmpeg compression failed")?;
    
    // 5. Generate thumbnail
    tracing::info!("Generating thumbnail");
    generate_thumbnail(&input_path, &thumb_path)
        .context("Thumbnail generation failed")?;
    
    // 6. Upload compressed video to public bucket
    let video_key = format!("{}.mp4", job.video_id);
    let thumb_key = format!("{}_thumb.jpg", job.video_id);
    
    tracing::info!("Uploading compressed video to public bucket: key={}", video_key);
    storage.upload_file(
        &config.minio_bucket_videos,
        &video_key,
        &output_path,
        "video/mp4"
    ).await.context("Failed to upload compressed video")?;
    
    tracing::info!("Uploading thumbnail to public bucket: key={}", thumb_key);
    storage.upload_file(
        &config.minio_bucket_videos,
        &thumb_key,
        &thumb_path,
        "image/jpeg"
    ).await.context("Failed to upload thumbnail")?;
    
    // 7. Update database with PUBLISHED status
    let mut active: videos::ActiveModel = video.into();
    active.status = Set("PUBLISHED".to_string());
    active.s3_bucket = Set(config.minio_bucket_videos.clone());
    active.s3_key = Set(video_key.clone());
    active.update(db).await?;
    
    tracing::info!("Video {} published successfully", job.video_id);
    
    Ok(())
}

fn compress_video(input: &Path, output: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-i", input.to_str().unwrap(),
            "-c:v", "libx264",          // H.264 codec
            "-preset", "medium",         // Encoding speed/quality tradeoff
            "-crf", "23",                // Quality (0-51, lower = better quality)
            "-maxrate", "2M",            // Max bitrate
            "-bufsize", "4M",            // Buffer size
            "-vf", "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease", // Max 1080p
            "-c:a", "aac",               // AAC audio codec
            "-b:a", "128k",              // Audio bitrate
            "-movflags", "+faststart",   // Enable streaming
            "-y",                        // Overwrite output
            output.to_str().unwrap(),
        ])
        .status()
        .context("Failed to execute FFmpeg")?;
    
    if !status.success() {
        anyhow::bail!("FFmpeg exited with status: {}", status);
    }
    
    Ok(())
}

fn generate_thumbnail(input: &Path, output: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-i", input.to_str().unwrap(),
            "-ss", "00:00:01",           // Seek to 1 second
            "-vframes", "1",             // Extract 1 frame
            "-vf", "scale=480:-1",       // Width 480px, maintain aspect ratio
            "-y",                        // Overwrite output
            output.to_str().unwrap(),
        ])
        .status()
        .context("Failed to execute FFmpeg for thumbnail")?;
    
    if !status.success() {
        anyhow::bail!("FFmpeg thumbnail generation exited with status: {}", status);
    }
    
    Ok(())
}
