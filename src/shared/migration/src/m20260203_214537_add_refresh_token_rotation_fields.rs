use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add replaced_by and replaced_at columns
        manager
            .alter_table(
                Table::alter()
                    .table(RefreshTokens::Table)
                    .add_column(ColumnDef::new(RefreshTokens::ReplacedBy).uuid().null())
                    .add_column(
                        ColumnDef::new(RefreshTokens::ReplacedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk-refresh_tokens-replaced_by")
                            .from_tbl(RefreshTokens::Table)
                            .to_tbl(RefreshTokens::Table)
                            .from_col(RefreshTokens::ReplacedBy)
                            .to_col(RefreshTokens::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop columns
        manager
            .alter_table(
                Table::alter()
                    .table(RefreshTokens::Table)
                    .drop_foreign_key(Alias::new("fk-refresh_tokens-replaced_by"))
                    .drop_column(RefreshTokens::ReplacedBy)
                    .drop_column(RefreshTokens::ReplacedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum RefreshTokens {
    Table,
    Id,
    ReplacedBy,
    ReplacedAt,
}
