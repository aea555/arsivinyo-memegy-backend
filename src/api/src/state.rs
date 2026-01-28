use sea_orm::DatabaseConnection;
use shared::{config::Config, queue::QueueService, storage::StorageService};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Arc<Config>,
    pub storage: StorageService,
    pub queue: QueueService,
}
