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
        ]
    }
}
