use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create index on videos.status for filtering PUBLISHED videos
        manager
            .create_index(
                Index::create()
                    .name("idx_videos_status")
                    .table(Videos::Table)
                    .col(Videos::Status)
                    .to_owned(),
            )
            .await?;

        // Create composite index for feed sorting by created_at (latest)
        manager
            .create_index(
                Index::create()
                    .name("idx_videos_status_created_at")
                    .table(Videos::Table)
                    .col(Videos::Status)
                    .col(Videos::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Create composite index for feed sorting by like_count (popular)
        manager
            .create_index(
                Index::create()
                    .name("idx_videos_status_like_count")
                    .table(Videos::Table)
                    .col(Videos::Status)
                    .col(Videos::LikeCount)
                    .to_owned(),
            )
            .await?;

        // Create index on videos.user_id for user-specific queries
        manager
            .create_index(
                Index::create()
                    .name("idx_videos_user_id")
                    .table(Videos::Table)
                    .col(Videos::UserId)
                    .to_owned(),
            )
            .await?;

        // Create index on likes for counting (video_id)
        // The composite primary key already handles uniqueness
        manager
            .create_index(
                Index::create()
                    .name("idx_likes_video_id")
                    .table(Likes::Table)
                    .col(Likes::VideoId)
                    .to_owned(),
            )
            .await?;

        // Create index on refresh_tokens.user_id for token lookups
        manager
            .create_index(
                Index::create()
                    .name("idx_refresh_tokens_user_id")
                    .table(RefreshTokens::Table)
                    .col(RefreshTokens::UserId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_videos_status").to_owned())
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_videos_status_created_at")
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_videos_status_like_count")
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(Index::drop().name("idx_videos_user_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_likes_video_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_refresh_tokens_user_id").to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    Status,
    CreatedAt,
    LikeCount,
    UserId,
}

#[derive(DeriveIden)]
enum Likes {
    Table,
    VideoId,
}

#[derive(DeriveIden)]
enum RefreshTokens {
    Table,
    UserId,
}
