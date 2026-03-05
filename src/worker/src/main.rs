use anyhow::Result;
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use sea_orm_migration::prelude::*;
use serde::Deserialize;
use shared::{
    config::Config,
    entities::{likes, videos},
    queue::{QueueService, VideoProcessJob},
    storage::{S3Storage, StorageBackend},
};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::process::Command;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

mod bulk_download;
use bulk_download::BulkDownloadWorker;

mod account_cleanup;
use account_cleanup::AccountCleanupWorker;

const STDERR_TAIL_BYTES: usize = 8 * 1024;
const MAX_ERROR_MESSAGE_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessingErrorKind {
    Transient,
    Permanent,
}

#[derive(Debug, Clone)]
struct ProcessingError {
    kind: ProcessingErrorKind,
    code: &'static str,
    message: String,
}

impl ProcessingError {
    fn transient(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: ProcessingErrorKind::Transient,
            code,
            message: message.into(),
        }
    }

    fn permanent(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: ProcessingErrorKind::Permanent,
            code,
            message: message.into(),
        }
    }

    fn is_transient(&self) -> bool {
        self.kind == ProcessingErrorKind::Transient
    }
}

impl std::fmt::Display for ProcessingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ProcessingError {}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Option<Vec<FfprobeStream>>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

struct TranscodeProfile<'a> {
    name: &'a str,
    args: Vec<String>,
}

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

    let storage: Arc<dyn StorageBackend + Send + Sync> = Arc::new(S3Storage::new(&config).await);
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

    // 7. Account Cleanup Task
    let account_cleaner = AccountCleanupWorker::new(db.clone(), storage.clone(), config.clone());

    tokio::spawn(async move {
        if let Err(e) = account_cleaner.run().await {
            tracing::error!("Account cleanup worker error: {:?}", e);
        }
    });

    tracing::info!("Worker started, waiting for jobs...");

    loop {
        match queue.pop_video_job().await {
            Ok(Some(job)) => {
                tracing::info!("Processing job: {:?}", job);

                // Retry logic with exponential backoff
                let mut attempt = 1;
                let mut last_error: Option<ProcessingError> = None;

                while attempt <= config.worker_retry_max_attempts {
                    match process_video(&db, storage.clone(), &queue, &config, &job).await {
                        Ok(()) => {
                            tracing::info!(
                                "Video {} processed successfully on attempt {}",
                                job.video_id,
                                attempt
                            );
                            break; // Success!
                        }
                        Err(e) => {
                            let should_retry =
                                e.is_transient() && attempt < config.worker_retry_max_attempts;
                            let is_transient = e.is_transient();
                            last_error = Some(e.clone());
                            if should_retry {
                                let backoff_secs = config.worker_retry_backoff_base_secs
                                    * (2_u64.pow(attempt as u32 - 1));
                                tracing::warn!(
                                    "Failed to process video {} (attempt {}/{}): code={} transient={} error={}. Retrying in {}s",
                                    job.video_id,
                                    attempt,
                                    config.worker_retry_max_attempts,
                                    e.code,
                                    is_transient,
                                    e.message,
                                    backoff_secs
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs))
                                    .await;
                                attempt += 1;
                            } else {
                                tracing::error!(
                                    "Failed to process video {} after {} attempts: code={} transient={} error={}",
                                    job.video_id,
                                    config.worker_retry_max_attempts,
                                    e.code,
                                    is_transient,
                                    e.message
                                );
                                break;
                            }
                        }
                    }
                }

                // If all retries failed update status to FAILED
                if let Some(err) = last_error
                    && let Ok(Some(v)) = videos::Entity::find_by_id(job.video_id).one(&db).await
                {
                    let previous_status = v.status.clone();
                    let mut active: videos::ActiveModel = v.into();
                    active.status = Set("FAILED".to_string());
                    active.processing_error_code = Set(Some(err.code.to_string()));
                    active.processing_error_message =
                        Set(Some(sanitize_error_message(&err.message)));
                    active.failed_at = Set(Some(chrono::Utc::now().into()));
                    if let Ok(updated_video) = active.update(&db).await
                        && let Err(e) = publish_video_status_signal(
                            &db,
                            &queue,
                            &config,
                            job.user_id,
                            "video.status.failed",
                            Some(previous_status),
                            &updated_video,
                        )
                        .await
                    {
                        tracing::warn!("Failed to publish failed realtime signal: {:?}", e);
                    }
                    invalidate_user_video_cache(&queue, job.user_id).await;
                    tracing::error!(
                        "Marked video {} as FAILED after exhausting retries: code={} transient={} error={}",
                        job.video_id,
                        err.code,
                        err.is_transient(),
                        err.message
                    );
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
    storage: Arc<dyn StorageBackend + Send + Sync>,
    queue: &QueueService,
    config: &Config,
    job: &VideoProcessJob,
) -> Result<(), ProcessingError> {
    tracing::info!("Starting video processing for video_id={}", job.video_id);

    // 1. Fetch video metadata from database
    let video = videos::Entity::find_by_id(job.video_id)
        .one(db)
        .await
        .map_err(|e| ProcessingError::transient("DB_READ_FAILED", e.to_string()))?
        .ok_or_else(|| {
            ProcessingError::permanent("VIDEO_NOT_FOUND", "Video not found in database")
        })?;

    // 2. Create temporary working directory
    let temp_dir =
        TempDir::new().map_err(|e| ProcessingError::transient("TEMP_DIR_FAILED", e.to_string()))?;
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
        .map_err(|e| ProcessingError::transient("RAW_DOWNLOAD_FAILED", e.to_string()))?;

    // 4. Preflight media validation with ffprobe.
    let duration_seconds =
        run_ffprobe_preflight(&input_path, config.ffmpeg_transcode_timeout_secs).await?;

    // 5. Compress video with FFmpeg fallback profiles.
    tracing::info!("Compressing video with FFmpeg");
    run_transcode_fallbacks(&input_path, &output_path, config, job).await?;

    // 6. Generate thumbnail (best effort; does not fail processing if both attempts fail).
    let mut thumbnail_ready = false;
    tracing::info!("Generating thumbnail from transcoded output");
    if generate_thumbnail(
        &output_path,
        &thumb_path,
        config.ffmpeg_thumbnail_timeout_secs,
    )
    .await
    .is_ok()
    {
        thumbnail_ready = true;
    } else {
        tracing::warn!(
            "Thumbnail generation from output failed for video {}, retrying from input",
            job.video_id
        );
        if generate_thumbnail(
            &input_path,
            &thumb_path,
            config.ffmpeg_thumbnail_timeout_secs,
        )
        .await
        .is_ok()
        {
            thumbnail_ready = true;
        } else {
            tracing::warn!(
                "THUMBNAIL_FAILED video={} user={}",
                job.video_id,
                job.user_id
            );
        }
    }

    // 7. Upload compressed video to public bucket
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
        .map_err(|e| ProcessingError::transient("PUBLISHED_UPLOAD_FAILED", e.to_string()))?;

    if thumbnail_ready {
        tracing::info!("Uploading thumbnail to public bucket: key={}", thumb_key);
        if let Err(e) = storage
            .upload_file(
                &config.minio_bucket_videos,
                &thumb_key,
                &thumb_path,
                "image/jpeg",
            )
            .await
        {
            tracing::warn!(
                "THUMBNAIL_UPLOAD_FAILED video={} user={} error={}",
                job.video_id,
                job.user_id,
                e
            );
        }
    }

    // 8. Update database with PUBLISHED status
    let mut active: videos::ActiveModel = video.into();
    active.status = Set("PUBLISHED".to_string());
    active.s3_bucket = Set(config.minio_bucket_videos.clone());
    active.s3_key = Set(video_key.clone());
    active.duration_seconds = Set(duration_seconds);
    active.processing_error_code = Set(None);
    active.processing_error_message = Set(None);
    active.failed_at = Set(None);
    let published_video = active
        .update(db)
        .await
        .map_err(|e| ProcessingError::transient("DB_WRITE_FAILED", e.to_string()))?;

    // 9. Invalidate caches affected by publish transition.
    invalidate_feed_cache(queue).await;
    invalidate_user_video_cache(queue, job.user_id).await;

    if let Err(e) = publish_video_status_signal(
        db,
        queue,
        config,
        job.user_id,
        "video.status.published",
        Some("PROCESSING".to_string()),
        &published_video,
    )
    .await
    {
        tracing::warn!("Failed to publish published realtime signal: {:?}", e);
    }

    tracing::info!("Video {} published successfully", job.video_id);

    Ok(())
}

async fn invalidate_feed_cache(queue: &QueueService) {
    use redis::AsyncCommands;
    if let Ok(mut conn) = queue.get_conn().await
        && let Ok(keys) = conn.keys::<_, Vec<String>>("feed:*").await
        && !keys.is_empty()
    {
        let _: Result<(), _> = conn.del(&keys).await;
        tracing::debug!("Invalidated feed cache after publishing video");
    }
}

async fn invalidate_user_video_cache(queue: &QueueService, user_id: Uuid) {
    use redis::AsyncCommands;
    if let Ok(mut conn) = queue.get_conn().await {
        let pattern = format!("user:{}:videos:*", user_id);
        if let Ok(keys) = conn.keys::<_, Vec<String>>(&pattern).await
            && !keys.is_empty()
        {
            let _: Result<(), _> = conn.del(&keys).await;
            tracing::debug!(
                "Invalidated {} /users/me/videos cache key(s) for user {}",
                keys.len(),
                user_id
            );
        }
    }
}

async fn publish_video_status_signal(
    db: &sea_orm::DatabaseConnection,
    queue: &QueueService,
    config: &Config,
    user_id: Uuid,
    event_type: &str,
    previous_status: Option<String>,
    video: &videos::Model,
) -> Result<()> {
    let is_liked = likes::Entity::find_by_id((user_id, video.id))
        .one(db)
        .await?
        .is_some();
    let realtime_video = build_realtime_video_payload(
        &config.minio_public_endpoint,
        &config.minio_bucket_videos,
        video,
        is_liked,
    );

    let payload = serde_json::json!({
        "type": event_type,
        "event_id": Uuid::new_v4(),
        "event_at": chrono::Utc::now().fixed_offset(),
        "version": 1,
        "previous_status": previous_status,
        "video": realtime_video
    });

    queue
        .publish_video_status_event(user_id, &payload.to_string())
        .await?;

    Ok(())
}

fn build_realtime_video_payload(
    minio_public_endpoint: &str,
    minio_bucket_videos: &str,
    video: &videos::Model,
    is_liked: bool,
) -> serde_json::Value {
    let is_published_like = video.status.eq_ignore_ascii_case("PUBLISHED")
        || video.status.eq_ignore_ascii_case("COMPLETED");
    let has_public_object = is_published_like || video.s3_bucket == minio_bucket_videos;
    let url_bucket = if is_published_like {
        minio_bucket_videos.to_string()
    } else {
        video.s3_bucket.clone()
    };
    let url = if has_public_object {
        Some(format!(
            "{}/{}/{}",
            minio_public_endpoint, url_bucket, video.s3_key
        ))
    } else {
        None
    };
    let thumbnail_url = if has_public_object {
        Some(format!(
            "{}/{}/{}_thumb.jpg",
            minio_public_endpoint, minio_bucket_videos, video.id
        ))
    } else {
        None
    };

    serde_json::json!({
        "id": video.id,
        "title": video.title.clone(),
        "description": video.description.clone(),
        "status": video.status.clone(),
        "created_at": video.created_at,
        "updated_at": video.updated_at,
        "is_anonymous": video.is_anonymous,
        "is_nsfw": video.is_nsfw,
        "is_liked": is_liked,
        "like_count": video.like_count,
        "url": url,
        "thumbnail_url": thumbnail_url,
        "processing_error_code": video.processing_error_code.clone(),
        "processing_error_message": video.processing_error_message.clone(),
    })
}

async fn run_ffprobe_preflight(
    input: &Path,
    timeout_secs: u64,
) -> Result<Option<i32>, ProcessingError> {
    let output = run_command_with_timeout(
        "ffprobe",
        vec![
            "-v".to_string(),
            "error".to_string(),
            "-print_format".to_string(),
            "json".to_string(),
            "-show_streams".to_string(),
            "-show_format".to_string(),
            input.display().to_string(),
        ],
        timeout_secs.max(5),
    )
    .await?;

    if !output.status.success() {
        let stderr_tail = tail_text(&output.stderr, STDERR_TAIL_BYTES);
        return Err(ProcessingError::permanent(
            "INVALID_MEDIA",
            format!("ffprobe failed: {}", sanitize_error_message(&stderr_tail)),
        ));
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| ProcessingError::permanent("CORRUPT_INPUT", e.to_string()))?;
    let has_video_stream = parsed
        .streams
        .unwrap_or_default()
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("video"));
    if !has_video_stream {
        return Err(ProcessingError::permanent(
            "NO_VIDEO_STREAM",
            "Input has no video stream",
        ));
    }

    let duration_seconds = parsed
        .format
        .and_then(|format| format.duration)
        .and_then(|duration_str| duration_str.parse::<f64>().ok())
        .map(|duration| duration.round() as i32);

    if let Some(duration) = duration_seconds
        && duration <= 0
    {
        return Err(ProcessingError::permanent(
            "INVALID_MEDIA",
            "Input media duration is not positive",
        ));
    }

    Ok(duration_seconds)
}

fn build_transcode_profiles(input: &Path, output: &Path) -> Vec<TranscodeProfile<'static>> {
    let input = input.display().to_string();
    let output = output.display().to_string();
    let base_tail = vec![
        "-c:v".to_string(),
        "libx264".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-vf".to_string(),
        "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-y".to_string(),
        output.clone(),
    ];

    let mut primary = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-i".to_string(),
        input.clone(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "128k".to_string(),
        "-preset".to_string(),
        "medium".to_string(),
        "-crf".to_string(),
        "23".to_string(),
        "-maxrate".to_string(),
        "2M".to_string(),
        "-bufsize".to_string(),
        "4M".to_string(),
    ];
    primary.extend(base_tail.clone());

    let mut tolerant = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-fflags".to_string(),
        "+genpts+discardcorrupt".to_string(),
        "-err_detect".to_string(),
        "ignore_err".to_string(),
        "-analyzeduration".to_string(),
        "100M".to_string(),
        "-probesize".to_string(),
        "100M".to_string(),
        "-i".to_string(),
        input.clone(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "96k".to_string(),
        "-preset".to_string(),
        "veryfast".to_string(),
        "-crf".to_string(),
        "26".to_string(),
    ];
    tolerant.extend(base_tail.clone());

    let mut no_audio = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-fflags".to_string(),
        "+genpts+discardcorrupt".to_string(),
        "-analyzeduration".to_string(),
        "100M".to_string(),
        "-probesize".to_string(),
        "100M".to_string(),
        "-i".to_string(),
        input,
        "-an".to_string(),
        "-preset".to_string(),
        "veryfast".to_string(),
        "-crf".to_string(),
        "26".to_string(),
    ];
    no_audio.extend(base_tail);

    vec![
        TranscodeProfile {
            name: "primary_h264_aac",
            args: primary,
        },
        TranscodeProfile {
            name: "fallback_tolerant_decode",
            args: tolerant,
        },
        TranscodeProfile {
            name: "fallback_no_audio",
            args: no_audio,
        },
    ]
}

async fn run_transcode_fallbacks(
    input: &Path,
    output: &Path,
    config: &Config,
    job: &VideoProcessJob,
) -> Result<(), ProcessingError> {
    let profiles = build_transcode_profiles(input, output);
    let mut last_err = None;
    for profile in profiles {
        let started = std::time::Instant::now();
        match run_command_with_timeout(
            "ffmpeg",
            profile.args.clone(),
            config.ffmpeg_transcode_timeout_secs.max(15),
        )
        .await
        {
            Ok(output) if output.status.success() => {
                tracing::info!(
                    "Transcode succeeded profile={} video={} user={} elapsed_ms={}",
                    profile.name,
                    job.video_id,
                    job.user_id,
                    started.elapsed().as_millis()
                );
                return Ok(());
            }
            Ok(output) => {
                let stderr_tail = tail_text(&output.stderr, STDERR_TAIL_BYTES);
                let msg = sanitize_error_message(&stderr_tail);
                let exit_code = output.status.code().unwrap_or(-1);
                tracing::warn!(
                    "Transcode failed profile={} video={} user={} exit_code={} elapsed_ms={} stderr_tail={}",
                    profile.name,
                    job.video_id,
                    job.user_id,
                    exit_code,
                    started.elapsed().as_millis(),
                    msg
                );
                last_err = Some(msg);
            }
            Err(err) => {
                tracing::warn!(
                    "Transcode error profile={} video={} user={} code={} transient={} message={} elapsed_ms={}",
                    profile.name,
                    job.video_id,
                    job.user_id,
                    err.code,
                    err.is_transient(),
                    err.message,
                    started.elapsed().as_millis()
                );
                if err.is_transient() {
                    return Err(err);
                }
                last_err = Some(err.message);
            }
        }
    }

    Err(ProcessingError::permanent(
        "TRANSCODE_FAILED_PERMANENT",
        format!(
            "All transcode profiles failed{}",
            last_err
                .map(|m| format!(": {}", sanitize_error_message(&m)))
                .unwrap_or_default()
        ),
    ))
}

async fn generate_thumbnail(input: &Path, output: &Path, timeout_secs: u64) -> Result<()> {
    let output_data = run_command_with_timeout(
        "ffmpeg",
        vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-i".to_string(),
            input.display().to_string(),
            "-ss".to_string(),
            "00:00:01".to_string(),
            "-vframes".to_string(),
            "1".to_string(),
            "-vf".to_string(),
            "scale=480:-1".to_string(),
            "-y".to_string(),
            output.display().to_string(),
        ],
        timeout_secs.max(5),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    if !output_data.status.success() {
        anyhow::bail!(
            "FFmpeg thumbnail generation exited with code {:?}: {}",
            output_data.status.code(),
            tail_text(&output_data.stderr, STDERR_TAIL_BYTES)
        );
    }

    Ok(())
}

async fn run_command_with_timeout(
    program: &str,
    args: Vec<String>,
    timeout_secs: u64,
) -> Result<std::process::Output, ProcessingError> {
    let mut command = Command::new(program);
    command.kill_on_drop(true);
    command.args(&args);

    let timed_output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        command.output(),
    )
    .await;

    match timed_output {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(ProcessingError::transient(
            "PROCESS_EXEC_FAILED",
            format!("{} spawn/wait failed: {}", program, e),
        )),
        Err(_) => Err(ProcessingError::transient(
            "PROCESS_TIMEOUT",
            format!("{} exceeded timeout of {}s", program, timeout_secs),
        )),
    }
}

fn tail_text(bytes: &[u8], limit: usize) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let start = bytes.len().saturating_sub(limit);
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

fn sanitize_error_message(message: &str) -> String {
    let normalized = message.replace(['\n', '\r', '\t'], " ");
    normalized
        .chars()
        .take(MAX_ERROR_MESSAGE_CHARS)
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::build_realtime_video_payload;
    use shared::entities::videos;
    use uuid::Uuid;

    #[test]
    fn realtime_payload_includes_nsfw_and_thumbnail_url_for_published_video() {
        let now = chrono::Utc::now().fixed_offset();
        let video = videos::Model {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            title: Some("video".to_string()),
            description: Some("desc".to_string()),
            s3_bucket: "raw".to_string(),
            s3_key: "test.mp4".to_string(),
            status: "PUBLISHED".to_string(),
            size_bytes: 1024,
            duration_seconds: Some(5),
            like_count: 3,
            is_anonymous: false,
            is_nsfw: Some(false),
            moderation_state: "VISIBLE".to_string(),
            moderation_reason_code: None,
            moderation_updated_at: None,
            moderation_updated_by: None,
            moderation_source_report_id: None,
            processing_error_code: None,
            processing_error_message: None,
            failed_at: None,
            deleted_at: None,
            created_at: now,
            updated_at: now,
        };

        let payload =
            build_realtime_video_payload("https://cdn.memegy.com", "videos", &video, true);

        assert_eq!(payload["is_nsfw"], serde_json::json!(false));
        assert!(payload["thumbnail_url"].as_str().is_some());
        assert!(payload["url"].as_str().is_some());
    }
}
