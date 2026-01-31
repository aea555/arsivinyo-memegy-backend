use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add is_anonymous column to videos table
        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .add_column(
                        ColumnDef::new(Videos::IsAnonymous)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // Create index for querying anonymous videos
        manager
            .create_index(
                Index::create()
                    .name("idx_videos_is_anonymous")
                    .table(Videos::Table)
                    .col(Videos::IsAnonymous)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop index
        manager
            .drop_index(Index::drop().name("idx_videos_is_anonymous").to_owned())
            .await?;

        // Drop column
        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .drop_column(Videos::IsAnonymous)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    IsAnonymous,
}
