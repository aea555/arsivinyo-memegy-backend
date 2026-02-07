use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ExtensionSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ExtensionSessions::Jti)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ExtensionSessions::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(ExtensionSessions::DeviceIdHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionSessions::Platform)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionSessions::Scope)
                            .array(ColumnType::String(StringLen::None))
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionSessions::IssuedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(ExtensionSessions::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionSessions::RevokedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_extension_sessions_users")
                            .from(ExtensionSessions::Table, ExtensionSessions::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_extension_sessions_user_revoked")
                    .table(ExtensionSessions::Table)
                    .col(ExtensionSessions::UserId)
                    .col(ExtensionSessions::RevokedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_extension_sessions_expires_at")
                    .table(ExtensionSessions::Table)
                    .col(ExtensionSessions::ExpiresAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SendTickets::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SendTickets::TicketId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SendTickets::UserId).uuid().not_null())
                    .col(ColumnDef::new(SendTickets::VideoId).uuid().not_null())
                    .col(
                        ColumnDef::new(SendTickets::DeviceIdHash)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SendTickets::HostAppHint).string().null())
                    .col(
                        ColumnDef::new(SendTickets::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(SendTickets::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SendTickets::RedeemedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(SendTickets::Status)
                            .string()
                            .not_null()
                            .default("active"),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_send_tickets_users")
                            .from(SendTickets::Table, SendTickets::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_send_tickets_videos")
                            .from(SendTickets::Table, SendTickets::VideoId)
                            .to(Videos::Table, Videos::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_send_tickets_active_by_user")
                    .table(SendTickets::Table)
                    .col(SendTickets::UserId)
                    .col(SendTickets::ExpiresAt)
                    .cond_where(
                        Expr::col(SendTickets::RedeemedAt)
                            .is_null()
                            .and(Expr::col(SendTickets::Status).eq("active")),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SendTickets::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(ExtensionSessions::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum ExtensionSessions {
    Table,
    Jti,
    UserId,
    DeviceIdHash,
    Platform,
    Scope,
    IssuedAt,
    ExpiresAt,
    RevokedAt,
}

#[derive(DeriveIden)]
enum SendTickets {
    Table,
    TicketId,
    UserId,
    VideoId,
    DeviceIdHash,
    HostAppHint,
    CreatedAt,
    ExpiresAt,
    RedeemedAt,
    Status,
}
