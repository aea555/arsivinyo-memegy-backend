use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .add_column(ColumnDef::new(Videos::IsNsfw).boolean().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_videos_is_nsfw")
                    .table(Videos::Table)
                    .col(Videos::IsNsfw)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX idx_videos_sfw_created_at
                ON videos (created_at DESC, id DESC)
                WHERE deleted_at IS NULL AND is_nsfw = false;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX idx_videos_sfw_like_count_created_at
                ON videos (like_count DESC, created_at DESC, id DESC)
                WHERE deleted_at IS NULL AND is_nsfw = false;
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_videos_sfw_like_count_created_at;")
            .await?;

        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_videos_sfw_created_at;")
            .await?;

        manager
            .drop_index(Index::drop().name("idx_videos_is_nsfw").to_owned())
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .drop_column(Videos::IsNsfw)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    IsNsfw,
}
