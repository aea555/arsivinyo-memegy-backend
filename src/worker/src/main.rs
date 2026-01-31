use anyhow::{Context, Result};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use sea_orm_migration::prelude::*;
use shared::{
    config::Config,
    entities::videos,
    queue::{QueueService, VideoProcessJob},
    storage::StorageService,
};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod bulk_download;
use bulk_download::BulkDownloadWorker;

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

    // 5. Start bulk download worker in background
    let bulk_worker =
        BulkDownloadWorker::new(db.clone(), queue.clone(), storage.clone(), config.clone());

    tokio::spawn(async move {
        if let Err(e) = bulk_worker.run().await {
            tracing::error!("Bulk download worker error: {:?}", e);
        }
    });

    // 6. Periodic cleanup task
    let cleanup_worker =
        BulkDownloadWorker::new(db.clone(), queue.clone(), storage.clone(), config.clone());

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await; // Every hour
            if let Err(e) = cleanup_worker.cleanup_stale_jobs().await {
                tracing::error!("Cleanup task error: {:?}", e);
            }
        }
    });

    tracing::info!("Worker started, waiting for jobs...");

    loop {
        match queue.pop_video_job().await {
            Ok(Some(job)) => {
                tracing::info!("Processing job: {:?}", job);

                // Retry logic with exponential backoff
                let mut attempt = 1;
                let mut last_error = None;

                while attempt <= config.worker_retry_max_attempts {
                    match process_video(&db, &storage, &queue, &config, &job).await {
                        Ok(()) => {
                            tracing::info!(
                                "Video {} processed successfully on attempt {}",
                                job.video_id,
                                attempt
                            );
                            break; // Success!
                        }
                        Err(e) => {
                            last_error = Some(e);
                            if attempt < config.worker_retry_max_attempts {
                                let backoff_secs = config.worker_retry_backoff_base_secs
                                    * (2_u64.pow(attempt as u32 - 1));
                                tracing::warn!(
                                    "Failed to process video {} (attempt {}/{}): {:?}. Retrying in {}s",
                                    job.video_id, attempt, config.worker_retry_max_attempts, last_error, backoff_secs
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs))
                                    .await;
                                attempt += 1;
                            } else {
                                tracing::error!(
                                    "Failed to process video {} after {} attempts: {:?}",
                                    job.video_id,
                                    config.worker_retry_max_attempts,
                                    last_error
                                );
                                break;
                            }
                        }
                    }
                }

                // If all retries failed update status to FAILED
                if let Some(err) = last_error {
                    if let Ok(Some(v)) = videos::Entity::find_by_id(job.video_id).one(&db).await {
                        let mut active: videos::ActiveModel = v.into();
                        active.status = Set("FAILED".to_string());
                        let _ = active.update(&db).await;
                        tracing::error!(
                            "Marked video {} as FAILED after exhausting retries: {:?}",
                            job.video_id,
                            err
                        );
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
    queue: &QueueService,
    config: &Config,
    job: &VideoProcessJob,
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

    tracing::info!(
        "Downloading video from S3: bucket={}, key={}",
        video.s3_bucket,
        video.s3_key
    );

    // 3. Download raw video from S3
    storage
        .download_file(&video.s3_bucket, &video.s3_key, &input_path)
        .await
        .context("Failed to download video from S3")?;

    // 4. Compress video with FFmpeg
    tracing::info!("Compressing video with FFmpeg");
    compress_video(&input_path, &output_path).context("FFmpeg compression failed")?;

    // 5. Generate thumbnail
    tracing::info!("Generating thumbnail");
    generate_thumbnail(&input_path, &thumb_path).context("Thumbnail generation failed")?;

    // 6. Upload compressed video to public bucket
    let video_key = format!("{}.mp4", job.video_id);
    let thumb_key = format!("{}_thumb.jpg", job.video_id);

    tracing::info!(
        "Uploading compressed video to public bucket: key={}",
        video_key
    );
    storage
        .upload_file(
            &config.minio_bucket_videos,
            &video_key,
            &output_path,
            "video/mp4",
        )
        .await
        .context("Failed to upload compressed video")?;

    tracing::info!("Uploading thumbnail to public bucket: key={}", thumb_key);
    storage
        .upload_file(
            &config.minio_bucket_videos,
            &thumb_key,
            &thumb_path,
            "image/jpeg",
        )
        .await
        .context("Failed to upload thumbnail")?;

    // 7. Update database with PUBLISHED status
    let mut active: videos::ActiveModel = video.into();
    active.status = Set("PUBLISHED".to_string());
    active.s3_bucket = Set(config.minio_bucket_videos.clone());
    active.s3_key = Set(video_key.clone());
    active.update(db).await?;

    // 8. Invalidate feed cache
    use redis::AsyncCommands;
    if let Ok(mut conn) = queue.get_conn().await {
        // Use SCAN in production, KEYS is ok for moderate load
        if let Ok(keys) = conn.keys::<_, Vec<String>>("feed:*").await {
            if !keys.is_empty() {
                let _: Result<(), _> = conn.del(keys).await;
                tracing::debug!("Invalidated feed cache after publishing video");
            }
        }
    }

    tracing::info!("Video {} published successfully", job.video_id);

    Ok(())
}

fn compress_video(input: &Path, output: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-i",
            input.to_str().unwrap(),
            "-c:v",
            "libx264", // H.264 codec
            "-preset",
            "medium", // Encoding speed/quality tradeoff
            "-crf",
            "23", // Quality (0-51, lower = better quality)
            "-maxrate",
            "2M", // Max bitrate
            "-bufsize",
            "4M", // Buffer size
            "-vf",
            "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease", // Max 1080p
            "-c:a",
            "aac", // AAC audio codec
            "-b:a",
            "128k", // Audio bitrate
            "-movflags",
            "+faststart", // Enable streaming
            "-y",         // Overwrite output
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
            "-i",
            input.to_str().unwrap(),
            "-ss",
            "00:00:01", // Seek to 1 second
            "-vframes",
            "1", // Extract 1 frame
            "-vf",
            "scale=480:-1", // Width 480px, maintain aspect ratio
            "-y",           // Overwrite output
            output.to_str().unwrap(),
        ])
        .status()
        .context("Failed to execute FFmpeg for thumbnail")?;

    if !status.success() {
        anyhow::bail!("FFmpeg thumbnail generation exited with status: {}", status);
    }

    Ok(())
}
