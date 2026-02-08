use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AdminAuditLogs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AdminAuditLogs::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AdminAuditLogs::ActorSub).string().not_null())
                    .col(ColumnDef::new(AdminAuditLogs::Action).string().not_null())
                    .col(
                        ColumnDef::new(AdminAuditLogs::TargetTable)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AdminAuditLogs::TargetId).string().null())
                    .col(
                        ColumnDef::new(AdminAuditLogs::RequestId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AdminAuditLogs::Ip).string().null())
                    .col(ColumnDef::new(AdminAuditLogs::UserAgent).string().null())
                    .col(ColumnDef::new(AdminAuditLogs::Outcome).string().not_null())
                    .col(
                        ColumnDef::new(AdminAuditLogs::MetadataJson)
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AdminAuditLogs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_admin_audit_logs_created_at")
                    .table(AdminAuditLogs::Table)
                    .col(AdminAuditLogs::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_admin_audit_logs_actor_sub")
                    .table(AdminAuditLogs::Table)
                    .col(AdminAuditLogs::ActorSub)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AdminAuditLogs::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum AdminAuditLogs {
    Table,
    Id,
    ActorSub,
    Action,
    TargetTable,
    TargetId,
    RequestId,
    Ip,
    UserAgent,
    Outcome,
    MetadataJson,
    CreatedAt,
}
