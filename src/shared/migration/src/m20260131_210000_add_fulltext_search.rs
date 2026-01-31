use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add tsvector column for full-text search
        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .add_column(
                        ColumnDef::new(Videos::SearchVector)
                            .custom(sea_query::Alias::new("tsvector"))
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Create GIN index on search_vector
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_videos_search_vector ON videos USING GIN(search_vector)",
            )
            .await?;

        // Create trigger to automatically update search_vector
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE OR REPLACE FUNCTION videos_search_vector_update() RETURNS trigger AS $$
                BEGIN
                    NEW.search_vector := 
                        setweight(to_tsvector('english', COALESCE(NEW.title, '')), 'A') ||
                        setweight(to_tsvector('english', COALESCE(NEW.description, '')), 'B');
                    RETURN NEW;
                END
                $$ LANGUAGE plpgsql;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TRIGGER videos_search_vector_trigger
                BEFORE INSERT OR UPDATE ON videos
                FOR EACH ROW
                EXECUTE FUNCTION videos_search_vector_update();
                "#,
            )
            .await?;

        // Populate existing rows
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                UPDATE videos SET search_vector = 
                    setweight(to_tsvector('english', COALESCE(title, '')), 'A') ||
                    setweight(to_tsvector('english', COALESCE(description, '')), 'B')
                WHERE search_vector IS NULL;
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop trigger
        manager
            .get_connection()
            .execute_unprepared("DROP TRIGGER IF EXISTS videos_search_vector_trigger ON videos")
            .await?;

        // Drop function
        manager
            .get_connection()
            .execute_unprepared("DROP FUNCTION IF EXISTS videos_search_vector_update()")
            .await?;

        // Drop index
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_videos_search_vector")
            .await?;

        // Drop column
        manager
            .alter_table(
                Table::alter()
                    .table(Videos::Table)
                    .drop_column(Videos::SearchVector)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Videos {
    Table,
    SearchVector,
}
