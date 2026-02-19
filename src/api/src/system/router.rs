use axum::{Router, routing::get};
use shared::config::Config;

use crate::state::AppState;

use super::handlers;

pub fn system_router(_config: &Config) -> Router<AppState> {
    Router::new()
        .route("/terms", get(handlers::get_terms))
        .route("/read-only", get(handlers::get_read_only_status))
        .route("/maintenance", get(handlers::get_maintenance_status))
}
