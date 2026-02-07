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
                    .add_column(ColumnDef::new(Videos::ProcessingErrorCode).string().null())
                    .add_column(ColumnDef::new(Videos::ProcessingErrorMessage).text().null())
                    .add_column(
                        ColumnDef::new(Videos::FailedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_videos_failed_at")
                    .table(Videos::Table)
                    .col(Videos::FailedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_videos_failed_at").to_owned())
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .drop_column(Videos::ProcessingErrorCode)
                    .drop_column(Videos::ProcessingErrorMessage)
                    .drop_column(Videos::FailedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    ProcessingErrorCode,
    ProcessingErrorMessage,
    FailedAt,
}
