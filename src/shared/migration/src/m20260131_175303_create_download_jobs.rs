use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create download_jobs table
        manager
            .create_table(
                Table::create()
                    .table(DownloadJobs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DownloadJobs::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DownloadJobs::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(DownloadJobs::VideoIds)
                            .array(ColumnType::Uuid)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DownloadJobs::Status)
                            .string()
                            .not_null()
                            .default("PENDING"),
                    )
                    .col(ColumnDef::new(DownloadJobs::IdempotencyKey).string())
                    .col(ColumnDef::new(DownloadJobs::ZipS3Key).string())
                    .col(ColumnDef::new(DownloadJobs::ZipUrl).string())
                    .col(ColumnDef::new(DownloadJobs::ZipSizeBytes).big_integer())
                    .col(ColumnDef::new(DownloadJobs::ErrorMessage).text())
                    .col(ColumnDef::new(DownloadJobs::PartialManifest).json())
                    .col(
                        ColumnDef::new(DownloadJobs::RetryCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(DownloadJobs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(DownloadJobs::UpdatedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(DownloadJobs::CompletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(DownloadJobs::ExpiresAt).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;

        // Create indexes for performance
        manager
            .create_index(
                Index::create()
                    .table(DownloadJobs::Table)
                    .name("idx_download_jobs_user_id")
                    .col(DownloadJobs::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .table(DownloadJobs::Table)
                    .name("idx_download_jobs_idempotency_key")
                    .col(DownloadJobs::IdempotencyKey)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .table(DownloadJobs::Table)
                    .name("idx_download_jobs_status")
                    .col(DownloadJobs::Status)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .table(DownloadJobs::Table)
                    .name("idx_download_jobs_created_at")
                    .col(DownloadJobs::CreatedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DownloadJobs::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum DownloadJobs {
    Table,
    Id,
    UserId,
    VideoIds,
    Status,          // PENDING, PROCESSING, COMPLETED, FAILED
    IdempotencyKey,  // For preventing duplicate jobs
    ZipS3Key,        // S3 key of generated ZIP
    ZipUrl,          // Presigned URL for download
    ZipSizeBytes,    // Size of generated ZIP
    ErrorMessage,    // Error details if failed
    PartialManifest, // JSON manifest of successful/failed videos
    RetryCount,      // Number of retry attempts
    CreatedAt,
    UpdatedAt,
    CompletedAt, // When job finished (success or failure)
    ExpiresAt,   // When ZIP file expires (7 days default)
}
