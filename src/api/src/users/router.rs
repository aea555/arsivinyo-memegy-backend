use axum::{Router, routing::get};
use shared::config::Config;

use crate::state::AppState;

use super::handlers;

pub fn users_router(_config: &Config) -> Router<AppState> {
    Router::new()
        .route(
            "/me",
            get(handlers::get_me).delete(handlers::delete_account),
        )
        .route(
            "/me/onboarding/status",
            get(handlers::get_onboarding_status),
        )
        .route(
            "/me/onboarding/complete",
            axum::routing::post(handlers::complete_onboarding),
        )
        .route(
            "/me/username",
            axum::routing::put(handlers::update_username),
        )
        .route("/me/videos", get(handlers::get_my_videos))
        .route("/me/videos/ws", get(handlers::my_videos_ws))
}
