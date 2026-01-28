use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Videos::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Videos::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Videos::UserId).uuid().not_null())
                    .col(ColumnDef::new(Videos::Title).string().null())
                    .col(ColumnDef::new(Videos::Description).string().null())
                    .col(ColumnDef::new(Videos::S3Bucket).string().not_null())
                    .col(ColumnDef::new(Videos::S3Key).string().not_null())
                    .col(ColumnDef::new(Videos::Status).string().not_null().default("DRAFT")) // Simple string for enum
                    .col(ColumnDef::new(Videos::SizeBytes).big_integer().not_null().default(0))
                    .col(ColumnDef::new(Videos::LikeCount).big_integer().not_null().default(0))
                    .col(
                        ColumnDef::new(Videos::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Videos::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_videos_users")
                            .from(Videos::Table, Videos::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Videos::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    Id,
    UserId,
    Title,
    Description,
    S3Bucket,
    S3Key,
    Status,
    SizeBytes,
    LikeCount,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}
