use axum::{
    Router,
    routing::{delete, get, post, put},
};
use shared::config::Config;

use crate::state::AppState;

use super::handlers;

pub fn videos_router(_config: &Config) -> Router<AppState> {
    Router::new()
        .route("/feed", get(handlers::get_feed))
        // Upload
        .route("/videos/init", post(handlers::init_upload))
        .route(
            "/videos/init/anonymous",
            post(handlers::init_anonymous_upload),
        )
        .route("/videos/:id/confirm", post(handlers::confirm_upload))
        .route("/videos/:id/report", post(handlers::report_video))
        // Search
        .route("/videos/search", get(handlers::search_videos))
        .route(
            "/videos/search/keyboard",
            get(handlers::search_videos_keyboard),
        )
        .route(
            "/videos/:id/send-ticket",
            post(handlers::create_send_ticket),
        )
        .route(
            "/videos/send-ticket/:ticket_id/media",
            get(handlers::redeem_send_ticket_media),
        )
        // Download
        .route("/videos/:id/download", get(handlers::download_video))
        .route(
            "/videos/:id/download/refresh",
            post(handlers::refresh_download_url),
        )
        // Bulk Download
        .route(
            "/videos/download/bulk",
            post(handlers::create_bulk_download),
        )
        .route(
            "/videos/download/bulk/:id",
            get(handlers::get_bulk_download_status),
        )
        // Actions
        .route(
            "/videos/:id/like",
            put(handlers::set_like_video).delete(handlers::unset_like_video),
        )
        .route(
            "/videos/:id",
            delete(handlers::delete_video).patch(handlers::update_video_metadata),
        )
        // Bulk Delete (New Feature)
        .route("/videos/bulk-delete", post(handlers::bulk_delete_videos))
}
