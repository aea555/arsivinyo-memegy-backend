use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add deleted_at column to videos table
        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .add_column(
                        ColumnDef::new(Videos::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(), // NULL = not deleted
                    )
                    .to_owned(),
            )
            .await?;

        // Create partial index for deleted videos (only index non-null values)
        manager
            .create_index(
                Index::create()
                    .name("idx_videos_deleted_at")
                    .table(Videos::Table)
                    .col(Videos::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop index
        manager
            .drop_index(Index::drop().name("idx_videos_deleted_at").to_owned())
            .await?;

        // Drop column
        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .drop_column(Videos::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    DeletedAt,
}
