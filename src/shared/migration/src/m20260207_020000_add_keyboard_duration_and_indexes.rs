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
                    .add_column(ColumnDef::new(Videos::DurationSeconds).integer().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_send_tickets_user_created_at")
                    .table(SendTickets::Table)
                    .col(SendTickets::UserId)
                    .col(SendTickets::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_extension_sessions_active_lookup")
                    .table(ExtensionSessions::Table)
                    .col(ExtensionSessions::UserId)
                    .col(ExtensionSessions::RevokedAt)
                    .col(ExtensionSessions::ExpiresAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_extension_sessions_active_lookup")
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_send_tickets_user_created_at")
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .drop_column(Videos::DurationSeconds)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    DurationSeconds,
}

#[derive(DeriveIden)]
enum SendTickets {
    Table,
    UserId,
    CreatedAt,
}

#[derive(DeriveIden)]
enum ExtensionSessions {
    Table,
    UserId,
    RevokedAt,
    ExpiresAt,
}
