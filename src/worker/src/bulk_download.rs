use anyhow::Result;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use shared::{
    config::Config,
    entities::{download_jobs, videos},
    queue::QueueService,
    storage::StorageBackend,
};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;
use zip::write::FileOptions;

pub struct BulkDownloadWorker {
    db: DatabaseConnection,
    queue: QueueService,
    storage: Arc<dyn StorageBackend + Send + Sync>,
    config: Config,
}

impl BulkDownloadWorker {
    pub fn new(
        db: DatabaseConnection,
        queue: QueueService,
        storage: Arc<dyn StorageBackend + Send + Sync>,
        config: Config,
    ) -> Self {
        Self {
            db,
            queue,
            storage,
            config,
        }
    }

    /// Main worker loop - polls Redis queue for jobs
    pub async fn run(&self) -> Result<()> {
        info!("Bulk download worker started");

        loop {
            match self.process_next_job().await {
                Ok(processed) => {
                    if !processed {
                        // No jobs available, sleep briefly
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
                Err(e) => {
                    error!("Error processing job: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        }
    }

    /// Process one job from the queue
    async fn process_next_job(&self) -> Result<bool> {
        let mut conn = self.queue.get_conn().await?;

        // Pop job from queue (blocking with 1s timeout)
        let job_data: Option<String> = redis::cmd("BRPOP")
            .arg("bulk_download_queue")
            .arg(1) // 1 second timeout
            .query_async(&mut conn)
            .await
            .ok()
            .and_then(|v: Option<(String, String)>| v.map(|(_, data)| data));

        let Some(job_json) = job_data else {
            return Ok(false); // No job available
        };

        // Parse job payload
        let job_info: serde_json::Value = serde_json::from_str(&job_json)?;
        let job_id = Uuid::parse_str(job_info["job_id"].as_str().unwrap())?;

        info!("Processing bulk download job: {}", job_id);

        // Fetch job from database
        let job = download_jobs::Entity::find_by_id(job_id)
            .one(&self.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Job not found: {}", job_id))?;

        // Update status to PROCESSING
        let mut active_job: download_jobs::ActiveModel = job.clone().into();
        active_job.status = Set("PROCESSING".to_string());
        active_job.updated_at = Set(Some(chrono::Utc::now().fixed_offset()));
        active_job.update(&self.db).await?;

        // Process the job with retries
        match self.process_job_with_retries(&job).await {
            Ok(_) => {
                info!("Job {} completed successfully", job_id);
                Ok(true)
            }
            Err(e) => {
                error!("Job {} failed after retries: {:?}", job_id, e);
                self.mark_job_failed(&job, &e.to_string()).await?;
                Ok(true)
            }
        }
    }

    /// Process job with exponential backoff retries
    async fn process_job_with_retries(&self, job: &download_jobs::Model) -> Result<()> {
        let max_retries = 3;
        let mut attempt = job.retry_count;

        while attempt < max_retries {
            match self.process_job(job).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    attempt += 1;
                    warn!(
                        "Job {} attempt {}/{} failed: {:?}",
                        job.id, attempt, max_retries, e
                    );

                    if attempt < max_retries {
                        // Exponential backoff: 2^attempt seconds
                        let backoff_secs = 2_u64.pow(attempt as u32);
                        info!("Retrying in {} seconds...", backoff_secs);

                        // Update retry count
                        let mut active_job: download_jobs::ActiveModel = job.clone().into();
                        active_job.retry_count = Set(attempt);
                        active_job.updated_at = Set(Some(chrono::Utc::now().fixed_offset()));
                        active_job.update(&self.db).await?;

                        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(anyhow::anyhow!("Max retries exceeded"))
    }

    /// Process a single bulk download job
    async fn process_job(&self, job: &download_jobs::Model) -> Result<()> {
        // CHECKPOINT #2: Re-validate videos (soft-delete protection)
        let videos_available = videos::Entity::find()
            .filter(videos::Column::Id.is_in(job.video_ids.clone()))
            .filter(videos::Column::DeletedAt.is_null())
            .filter(videos::Column::Status.eq("PUBLISHED"))
            .all(&self.db)
            .await?;

        let available_ids: HashMap<Uuid, _> =
            videos_available.iter().map(|v| (v.id, v.clone())).collect();

        // Build manifest for partial success
        let mut manifest = serde_json::json!({
            "job_id": job.id,
            "requested_videos": job.video_ids.len(),
            "successful": [],
            "failed": []
        });

        // Create temp directory for downloads
        let temp_dir = std::env::temp_dir().join(format!("bulk_{}", job.id));
        tokio::fs::create_dir_all(&temp_dir).await?;

        // Download all available videos
        let mut downloaded_files = vec![];
        for video_id in &job.video_ids {
            if let Some(video) = available_ids.get(video_id) {
                match self.download_video(&temp_dir, video).await {
                    Ok(path) => {
                        downloaded_files.push((video.clone(), path));
                        manifest["successful"]
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::json!({
                                "video_id": video_id,
                                "filename": format!("{}.mp4", video_id),
                                "size_bytes": video.size_bytes
                            }));
                    }
                    Err(e) => {
                        warn!("Failed to download video {}: {:?}", video_id, e);
                        manifest["failed"]
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::json!({
                                "video_id": video_id,
                                "reason": e.to_string()
                            }));
                    }
                }
            } else {
                // Video deleted between API and worker
                manifest["failed"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({
                        "video_id": video_id,
                        "reason": "Video no longer available"
                    }));
            }
        }

        // If no videos downloaded successfully, fail the job
        if downloaded_files.is_empty() {
            return Err(anyhow::anyhow!("No videos available to download"));
        }

        // Create ZIP archive
        let zip_path = temp_dir.join(format!("{}.zip", job.id));
        self.create_zip(&zip_path, downloaded_files, &manifest)
            .await?;

        let zip_size = tokio::fs::metadata(&zip_path).await?.len() as i64;

        // Upload ZIP to S3
        let zip_s3_key = format!("bulk_downloads/{}/{}.zip", job.user_id, job.id);
        self.storage
            .upload_file(
                &self.config.minio_bucket_videos, // Using videos bucket for now
                &zip_s3_key,
                &zip_path,
                "application/zip",
            )
            .await?;

        // Generate presigned URL (24h expiry)
        let zip_url = self
            .storage
            .generate_presigned_get(
                &self.config.minio_bucket_videos,
                &zip_s3_key,
                Duration::from_secs(86400), // 24 hours
            )
            .await?;

        // Log stats before moving manifest
        let successful_count = manifest["successful"].as_array().unwrap().len();
        let failed_count = manifest["failed"].as_array().unwrap().len();

        // Update job record
        let mut active_job: download_jobs::ActiveModel = job.clone().into();
        active_job.status = Set("COMPLETED".to_string());
        active_job.zip_s3_key = Set(Some(zip_s3_key));
        active_job.zip_url = Set(Some(zip_url));
        active_job.zip_size_bytes = Set(Some(zip_size));
        active_job.partial_manifest = Set(Some(manifest));
        active_job.completed_at = Set(Some(chrono::Utc::now().fixed_offset()));
        active_job.updated_at = Set(Some(chrono::Utc::now().fixed_offset()));
        active_job.update(&self.db).await?;

        // Cleanup temp directory
        tokio::fs::remove_dir_all(&temp_dir).await.ok();

        info!(
            "Job {} completed: {} successful, {} failed",
            job.id, successful_count, failed_count
        );

        Ok(())
    }

    /// Download a single video from S3
    async fn download_video(&self, temp_dir: &PathBuf, video: &videos::Model) -> Result<PathBuf> {
        let dest_path = temp_dir.join(format!("{}.mp4", video.id));
        self.storage
            .download_file(&video.s3_bucket, &video.s3_key, &dest_path)
            .await?;
        Ok(dest_path)
    }

    /// Create ZIP archive with videos and manifest
    async fn create_zip(
        &self,
        zip_path: &PathBuf,
        files: Vec<(videos::Model, PathBuf)>,
        manifest: &serde_json::Value,
    ) -> Result<()> {
        let file = std::fs::File::create(zip_path)?;
        let mut zip = zip::ZipWriter::new(file);

        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored); // No compression for videos

        // Add manifest.json
        zip.start_file("manifest.json", options)?;
        zip.write_all(serde_json::to_string_pretty(manifest)?.as_bytes())?;

        // Add video files
        for (video, path) in files {
            let filename = format!("{}.mp4", video.id);
            zip.start_file(&filename, options)?;
            let data = tokio::fs::read(&path).await?;
            zip.write_all(&data)?;
        }

        zip.finish()?;
        Ok(())
    }

    /// Mark job as failed
    async fn mark_job_failed(&self, job: &download_jobs::Model, error: &str) -> Result<()> {
        let mut active_job: download_jobs::ActiveModel = job.clone().into();
        active_job.status = Set("FAILED".to_string());
        active_job.error_message = Set(Some(error.to_string()));
        active_job.completed_at = Set(Some(chrono::Utc::now().fixed_offset()));
        active_job.updated_at = Set(Some(chrono::Utc::now().fixed_offset()));
        active_job.update(&self.db).await?;
        Ok(())
    }

    /// Cleanup stale jobs (PENDING > 1 hour)
    pub async fn cleanup_stale_jobs(&self) -> Result<()> {
        let cutoff = chrono::Utc::now().fixed_offset() - chrono::Duration::hours(1);

        let stale_jobs = download_jobs::Entity::find()
            .filter(download_jobs::Column::Status.eq("PENDING"))
            .filter(download_jobs::Column::CreatedAt.lt(cutoff))
            .all(&self.db)
            .await?;

        for job in stale_jobs {
            info!("Marking stale job as failed: {}", job.id);
            self.mark_job_failed(&job, "Job timeout - worker did not process within 1 hour")
                .await?;
        }

        Ok(())
    }
}
