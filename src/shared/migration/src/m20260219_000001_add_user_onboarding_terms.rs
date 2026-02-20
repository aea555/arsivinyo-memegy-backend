use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::AgeConfirmedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(Users::TermsAcceptedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(Users::TermsAcceptedVersion).text().null())
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE users
                ADD CONSTRAINT chk_users_terms_fields_consistent
                CHECK (
                    (terms_accepted_at IS NULL AND terms_accepted_version IS NULL)
                    OR
                    (terms_accepted_at IS NOT NULL AND terms_accepted_version IS NOT NULL)
                );
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE users DROP CONSTRAINT IF EXISTS chk_users_terms_fields_consistent;",
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::AgeConfirmedAt)
                    .drop_column(Users::TermsAcceptedAt)
                    .drop_column(Users::TermsAcceptedVersion)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    AgeConfirmedAt,
    TermsAcceptedAt,
    TermsAcceptedVersion,
}
