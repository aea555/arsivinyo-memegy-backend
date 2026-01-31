pub use sea_orm_migration::prelude::*;

mod m20220101_000001_create_table;
mod m20260128_171456_m20220101_000002_create_videos_table;
mod m20260128_171917_m20220101_000003_create_likes_table;
mod m20260131_000001_add_indexes;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220101_000001_create_table::Migration),
            Box::new(m20260128_171456_m20220101_000002_create_videos_table::Migration),
            Box::new(m20260128_171917_m20220101_000003_create_likes_table::Migration),
            Box::new(m20260131_000001_add_indexes::Migration),
        ]
    }
}
