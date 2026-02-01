use anyhow::Result;
use chrono::{Duration, Utc};
use sea_orm::*;
use shared::{
    config::Config,
    entities::{users, videos},
    storage::StorageBackend,
};
use std::sync::Arc;
use tokio::time::sleep;

pub struct AccountCleanupWorker {
    db: DatabaseConnection,
    storage: Arc<dyn StorageBackend + Send + Sync>,
    _config: Config,
}

impl AccountCleanupWorker {
    pub fn new(
        db: DatabaseConnection,
        storage: Arc<dyn StorageBackend + Send + Sync>,
        config: Config,
    ) -> Self {
        Self {
            db,
            storage,
            _config: config,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let check_interval = std::time::Duration::from_secs(3600); // Check every hour

        loop {
            if let Err(e) = self.process_expired_accounts().await {
                tracing::error!("Account cleanup error: {:?}", e);
            }
            sleep(check_interval).await;
        }
    }

    async fn process_expired_accounts(&self) -> Result<()> {
        let threshold = Utc::now() - Duration::days(30);

        // 1. Find expired users
        // `deleted_at` is TIMESTAMP WITH TIME ZONE
        let expired_users = users::Entity::find()
            .filter(users::Column::DeletedAt.lt(threshold))
            .all(&self.db)
            .await?;

        if expired_users.is_empty() {
            return Ok(());
        }

        tracing::info!("Found {} expired accounts to cleanup", expired_users.len());

        for user in expired_users {
            tracing::info!("Cleaning up account: {} ({})", user.username, user.id);

            // 2. Fetch all S3 keys for this user BEFORE deleting the user
            let user_videos = videos::Entity::find()
                .filter(videos::Column::UserId.eq(user.id))
                .all(&self.db)
                .await?;

            // 3. Delete from S3
            for video in user_videos {
                // Delete video file
                if let Err(e) = self
                    .storage
                    .delete_file(&video.s3_bucket, &video.s3_key)
                    .await
                {
                    tracing::error!("Failed to delete S3 video {}: {:?}", video.s3_key, e);
                }

                // Delete thumbnail (Derived key pattern: {video_id}_thumb.jpg)
                let thumb_key = format!("{}_thumb.jpg", video.id);
                if let Err(e) = self.storage.delete_file(&video.s3_bucket, &thumb_key).await {
                    tracing::error!("Failed to delete S3 thumbnail {}: {:?}", thumb_key, e);
                }
            }

            // 4. Hard Delete User (DB Cascade handles videos, likes, etc.)
            users::Entity::delete_by_id(user.id).exec(&self.db).await?;

            tracing::info!("Account hard deleted: {}", user.id);
        }

        Ok(())
    }
}
