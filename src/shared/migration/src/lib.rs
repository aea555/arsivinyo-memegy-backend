pub use sea_orm_migration::prelude::*;

mod m20220101_000001_create_table;
mod m20260128_171456_m20220101_000002_create_videos_table;
mod m20260128_171917_m20220101_000003_create_likes_table;
mod m20260131_000001_add_indexes;
mod m20260131_000002_add_anonymous_videos;
mod m20260131_000003_add_soft_delete;
mod m20260131_175303_create_download_jobs;
mod m20260131_210000_add_fulltext_search;
mod m20260201_000001_add_user_deletion_and_avatar;
mod m20260203_214537_add_refresh_token_rotation_fields;
mod m20260207_000001_add_video_processing_failure_fields;
mod m20260207_010000_create_extension_sessions_and_send_tickets;
mod m20260207_020000_add_keyboard_duration_and_indexes;
mod m20260208_000001_add_username_normalization;
mod m20260208_000002_create_admin_audit_logs;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220101_000001_create_table::Migration),
            Box::new(m20260128_171456_m20220101_000002_create_videos_table::Migration),
            Box::new(m20260128_171917_m20220101_000003_create_likes_table::Migration),
            Box::new(m20260131_000001_add_indexes::Migration),
            Box::new(m20260131_000002_add_anonymous_videos::Migration),
            Box::new(m20260131_000003_add_soft_delete::Migration),
            Box::new(m20260131_175303_create_download_jobs::Migration),
            Box::new(m20260131_210000_add_fulltext_search::Migration),
            Box::new(m20260201_000001_add_user_deletion_and_avatar::Migration),
            Box::new(m20260203_214537_add_refresh_token_rotation_fields::Migration),
            Box::new(m20260207_000001_add_video_processing_failure_fields::Migration),
            Box::new(m20260207_010000_create_extension_sessions_and_send_tickets::Migration),
            Box::new(m20260207_020000_add_keyboard_duration_and_indexes::Migration),
            Box::new(m20260208_000001_add_username_normalization::Migration),
            Box::new(m20260208_000002_create_admin_audit_logs::Migration),
        ]
    }
}
