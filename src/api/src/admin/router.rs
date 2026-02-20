use axum::{
    Router,
    routing::{delete, get, post, put},
};

use crate::state::AppState;

use super::handlers;

pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/db/tables", get(handlers::list_tables))
        .route("/db/tables/:table/rows", get(handlers::list_table_rows))
        .route("/db/tables/:table/rows/:id", get(handlers::get_table_row))
        .route("/users/:id/hard", delete(handlers::hard_delete_user))
        .route("/videos/:id/hard", delete(handlers::hard_delete_video))
        .route(
            "/download-jobs/:id/hard",
            delete(handlers::hard_delete_download_job),
        )
        .route(
            "/send-tickets/:id/hard",
            delete(handlers::hard_delete_send_ticket),
        )
        .route(
            "/extension-sessions/:id/hard",
            delete(handlers::hard_delete_extension_session),
        )
        .route(
            "/refresh-tokens/:id/hard",
            delete(handlers::hard_delete_refresh_token),
        )
        .route(
            "/users/:user_id/ban",
            get(handlers::get_user_ban)
                .put(handlers::upsert_user_ban)
                .delete(handlers::delete_user_ban),
        )
        .route(
            "/ip-bans",
            get(handlers::get_ip_ban)
                .put(handlers::upsert_ip_ban)
                .delete(handlers::delete_ip_ban),
        )
        .route(
            "/security/users/:user_id/ips",
            get(handlers::investigate_user_ips),
        )
        .route(
            "/security/ips/:ip/users",
            get(handlers::investigate_ip_users),
        )
        .route("/system/terms", get(handlers::admin_get_terms))
        .route(
            "/system/read-only",
            get(handlers::admin_get_read_only_status),
        )
        .route(
            "/system/maintenance",
            get(handlers::admin_get_maintenance_status),
        )
        // Full regular-user surface via run-as-user admin endpoints.
        .route("/users/:user_id/feed", get(handlers::as_user_feed))
        .route(
            "/users/:user_id/onboarding/status",
            get(handlers::as_user_onboarding_status),
        )
        .route(
            "/users/:user_id/onboarding/complete",
            post(handlers::as_user_onboarding_complete),
        )
        .route(
            "/users/:user_id/profile",
            get(handlers::as_user_get_profile),
        )
        .route(
            "/users/:user_id/account",
            delete(handlers::as_user_delete_account),
        )
        .route(
            "/users/:user_id/username",
            put(handlers::as_user_update_username),
        )
        .route("/users/:user_id/videos", get(handlers::as_user_get_videos))
        .route(
            "/users/:user_id/videos/init",
            post(handlers::as_user_init_upload),
        )
        .route(
            "/users/:user_id/videos/init/anonymous",
            post(handlers::as_user_init_anonymous_upload),
        )
        .route(
            "/users/:user_id/videos/:id/confirm",
            post(handlers::as_user_confirm_upload),
        )
        .route(
            "/users/:user_id/videos/search",
            get(handlers::as_user_search_videos),
        )
        .route(
            "/users/:user_id/videos/search/keyboard",
            get(handlers::as_user_search_videos_keyboard),
        )
        .route(
            "/users/:user_id/videos/:id/send-ticket",
            post(handlers::as_user_create_send_ticket),
        )
        .route(
            "/users/:user_id/videos/send-ticket/:ticket_id/media",
            get(handlers::as_user_redeem_send_ticket_media),
        )
        .route(
            "/users/:user_id/videos/:id/download",
            get(handlers::as_user_download_video),
        )
        .route(
            "/users/:user_id/videos/:id/download/refresh",
            post(handlers::as_user_refresh_download_url),
        )
        .route(
            "/users/:user_id/videos/download/bulk",
            post(handlers::as_user_create_bulk_download),
        )
        .route(
            "/users/:user_id/videos/download/bulk/:id",
            get(handlers::as_user_get_bulk_download_status),
        )
        .route(
            "/users/:user_id/videos/:id/like",
            put(handlers::as_user_set_like_video).delete(handlers::as_user_unset_like_video),
        )
        .route(
            "/users/:user_id/videos/:id",
            delete(handlers::as_user_delete_video).patch(handlers::as_user_update_video_metadata),
        )
        .route(
            "/users/:user_id/videos/bulk-delete",
            post(handlers::as_user_bulk_delete_videos),
        )
        .route(
            "/users/:user_id/videos/ws",
            get(handlers::as_user_my_videos_ws),
        )
        .route(
            "/users/:user_id/session",
            post(handlers::create_user_session),
        )
        .route(
            "/users/:user_id/session/refresh",
            post(handlers::as_user_refresh_session),
        )
        .route(
            "/users/:user_id/session/logout",
            post(handlers::as_user_logout_all),
        )
        .route(
            "/users/:user_id/extension-session",
            post(handlers::create_user_extension_session),
        )
}
