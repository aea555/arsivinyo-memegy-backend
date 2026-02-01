use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add deleted_at to users
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(Users::AvatarUrl).text().null())
                    .to_owned(),
            )
            .await?;

        // Add index on deleted_at for performance (filtering active users)
        manager
            .create_index(
                Index::create()
                    .name("idx_users_deleted_at")
                    .table(Users::Table)
                    .col(Users::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_users_deleted_at")
                    .table(Users::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::DeletedAt)
                    .drop_column(Users::AvatarUrl)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    DeletedAt,
    AvatarUrl,
}
