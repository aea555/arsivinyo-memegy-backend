use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::Response,
};

use crate::{error::ApiErrorResponse, state::AppState};

fn is_admin_path(path: &str) -> bool {
    path == "/admin" || path.starts_with("/admin/")
}

fn is_maintenance_allowlisted(method: &Method, path: &str) -> bool {
    matches!(
        (method, path),
        (&Method::GET, "/health")
            | (&Method::GET, "/auth/google/login")
            | (&Method::GET, "/auth/google/callback")
            | (&Method::POST, "/auth/exchange-otc")
            | (&Method::POST, "/auth/dev/login")
            | (&Method::GET, "/system/maintenance")
    )
}

fn is_confirm_upload_path(method: &Method, path: &str) -> bool {
    if method != Method::POST {
        return false;
    }

    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    parts.len() == 3 && parts[0] == "videos" && parts[2] == "confirm" && !parts[1].is_empty()
}

fn is_read_only_blocked_path(method: &Method, path: &str) -> bool {
    matches!(
        (method, path),
        (&Method::POST, "/videos/init") | (&Method::POST, "/videos/init/anonymous")
    ) || is_confirm_upload_path(method, path)
}

pub async fn platform_mode_guard(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiErrorResponse> {
    let path = req.uri().path();
    let method = req.method();

    if is_admin_path(path) {
        return Ok(next.run(req).await);
    }

    if state.config.maintenance_mode_enabled && !is_maintenance_allowlisted(method, path) {
        return Err(ApiErrorResponse::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "maintenance_mode_enabled",
        ));
    }

    if state.config.read_only_mode_enabled && is_read_only_blocked_path(method, path) {
        return Err(ApiErrorResponse::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "read_only_mode_enabled",
        ));
    }

    Ok(next.run(req).await)
}
