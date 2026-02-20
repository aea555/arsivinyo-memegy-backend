use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::Response,
};

use crate::{
    error::ApiErrorResponse,
    services::{
        ban_service::BanEnforcementError, client_ip::extract_client_ip_from_headers_and_extensions,
    },
    state::AppState,
};

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

pub async fn platform_mode_guard(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiErrorResponse> {
    let path = req.uri().path();
    let method = req.method();

    if state.config.maintenance_mode_enabled
        && !is_admin_path(path)
        && !is_maintenance_allowlisted(method, path)
    {
        return Err(ApiErrorResponse::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "maintenance_mode_enabled",
        ));
    }

    let client_ip = extract_client_ip_from_headers_and_extensions(
        req.headers(),
        req.extensions(),
        &state.config.environment,
        state.config.require_cloudflare_headers,
    )?;

    match state.ban_service.ensure_ip_not_banned(client_ip).await {
        Ok(()) => {}
        Err(BanEnforcementError::Banned) => {
            return Err(ApiErrorResponse::forbidden("ip_banned"));
        }
        Err(BanEnforcementError::Unavailable) => {
            return Err(ApiErrorResponse::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ban_check_unavailable",
            ));
        }
    }

    Ok(next.run(req).await)
}
