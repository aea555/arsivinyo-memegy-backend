use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE IF NOT EXISTS abuse_reports (
                    id UUID PRIMARY KEY,
                    video_id UUID NOT NULL,
                    reporter_user_id UUID NOT NULL,
                    reason_codes TEXT[] NOT NULL,
                    details TEXT NULL,
                    timestamp_seconds INTEGER NULL,
                    severity_score SMALLINT NOT NULL DEFAULT 0,
                    status TEXT NOT NULL DEFAULT 'open',
                    client_ip TEXT NOT NULL,
                    user_agent TEXT NULL,
                    auto_quarantined BOOLEAN NOT NULL DEFAULT false,
                    auto_rule TEXT NULL,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    closed_at TIMESTAMPTZ NULL,
                    closed_by_admin_sub TEXT NULL,
                    resolution_code TEXT NULL,
                    resolution_note TEXT NULL,
                    CONSTRAINT fk_abuse_reports_video_id
                        FOREIGN KEY(video_id) REFERENCES videos(id)
                        ON UPDATE CASCADE ON DELETE CASCADE,
                    CONSTRAINT fk_abuse_reports_reporter_user_id
                        FOREIGN KEY(reporter_user_id) REFERENCES users(id)
                        ON UPDATE CASCADE ON DELETE CASCADE
                );
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE abuse_reports
                ADD CONSTRAINT chk_abuse_reports_status
                CHECK (status IN ('open', 'under_review', 'auto_quarantined', 'resolved', 'rejected'));
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE abuse_reports
                ADD CONSTRAINT chk_abuse_reports_reason_codes_nonempty
                CHECK (COALESCE(array_length(reason_codes, 1), 0) > 0);
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE videos
                ADD COLUMN IF NOT EXISTS moderation_state TEXT NOT NULL DEFAULT 'VISIBLE';
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE videos
                ADD COLUMN IF NOT EXISTS moderation_reason_code TEXT NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE videos
                ADD COLUMN IF NOT EXISTS moderation_updated_at TIMESTAMPTZ NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE videos
                ADD COLUMN IF NOT EXISTS moderation_updated_by TEXT NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE videos
                ADD COLUMN IF NOT EXISTS moderation_source_report_id UUID NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE videos
                ADD CONSTRAINT chk_videos_moderation_state
                CHECK (moderation_state IN ('VISIBLE', 'QUARANTINED', 'REMOVED'));
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE videos
                ADD CONSTRAINT fk_videos_moderation_source_report
                FOREIGN KEY(moderation_source_report_id) REFERENCES abuse_reports(id)
                ON UPDATE CASCADE ON DELETE SET NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE UNIQUE INDEX IF NOT EXISTS uniq_abuse_reports_open_video_reporter
                ON abuse_reports (video_id, reporter_user_id)
                WHERE closed_at IS NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX IF NOT EXISTS idx_abuse_reports_queue
                ON abuse_reports (status, severity_score DESC, created_at ASC)
                WHERE closed_at IS NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX IF NOT EXISTS idx_abuse_reports_video_created_at
                ON abuse_reports (video_id, created_at DESC);
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX IF NOT EXISTS idx_abuse_reports_reporter_created_at
                ON abuse_reports (reporter_user_id, created_at DESC);
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX IF NOT EXISTS idx_videos_visible_latest
                ON videos (created_at DESC, id DESC)
                WHERE deleted_at IS NULL AND moderation_state = 'VISIBLE';
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX IF NOT EXISTS idx_videos_visible_popular
                ON videos (like_count DESC, created_at DESC, id DESC)
                WHERE deleted_at IS NULL AND moderation_state = 'VISIBLE';
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX IF NOT EXISTS idx_videos_visible_nsfw_filter
                ON videos (is_nsfw, created_at DESC, id DESC)
                WHERE deleted_at IS NULL AND moderation_state = 'VISIBLE';
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_videos_visible_nsfw_filter;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_videos_visible_popular;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_videos_visible_latest;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_abuse_reports_reporter_created_at;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_abuse_reports_video_created_at;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_abuse_reports_queue;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS uniq_abuse_reports_open_video_reporter;")
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE videos DROP CONSTRAINT IF EXISTS fk_videos_moderation_source_report;",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE videos DROP CONSTRAINT IF EXISTS chk_videos_moderation_state;",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE videos DROP COLUMN IF EXISTS moderation_source_report_id;",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE videos DROP COLUMN IF EXISTS moderation_updated_by;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE videos DROP COLUMN IF EXISTS moderation_updated_at;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE videos DROP COLUMN IF EXISTS moderation_reason_code;")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE videos DROP COLUMN IF EXISTS moderation_state;")
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE abuse_reports DROP CONSTRAINT IF EXISTS chk_abuse_reports_reason_codes_nonempty;",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE abuse_reports DROP CONSTRAINT IF EXISTS chk_abuse_reports_status;",
            )
            .await?;
        manager
            .drop_table(Table::drop().table(AbuseReports::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum AbuseReports {
    Table,
}
