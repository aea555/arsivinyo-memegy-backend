use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Likes::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Likes::UserId).uuid().not_null())
                    .col(ColumnDef::new(Likes::VideoId).uuid().not_null())
                    .col(
                        ColumnDef::new(Likes::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .name("pk_likes")
                            .col(Likes::UserId)
                            .col(Likes::VideoId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_likes_users")
                            .from(Likes::Table, Likes::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_likes_videos")
                            .from(Likes::Table, Likes::VideoId)
                            .to(Videos::Table, Videos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Likes::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Likes {
    Table,
    UserId,
    VideoId,
    CreatedAt,
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
