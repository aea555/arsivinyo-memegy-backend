use axum::{routing::get, Router};
use shared::config::Config;

use crate::state::AppState;

use super::handlers;

pub fn users_router(_config: &Config) -> Router<AppState> {
    Router::new()
        .route(
            "/me",
            get(handlers::get_me).delete(handlers::delete_account),
        )
        .route("/me/videos", get(handlers::get_my_videos))
}
