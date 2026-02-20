use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use crate::{
    services::{
        client_ip::extract_client_ip_from_headers_and_extensions, rate_limiter::RateLimiter,
    },
    state::AppState,
};

/// IP-based rate limiting middleware for public endpoints
pub async fn ip_rate_limit(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    let ip = match extract_client_ip_from_headers_and_extensions(
        req.headers(),
        req.extensions(),
        &state.config.environment,
        state.config.require_cloudflare_headers,
    ) {
        Ok(ip) => ip,
        Err(err) => return Err((err.status, err.error.error)),
    };

    // Check rate limit
    let rate_key = RateLimiter::ip_rpm_key(&ip.to_string());
    match state
        .rate_limiter
        .check_and_increment(
            &rate_key,
            state.config.ip_rate_limit_rpm,
            60, // 1 minute window
        )
        .await
    {
        Ok(Ok(_remaining)) => {
            // Within limit, proceed
            Ok(next.run(req).await)
        }
        Ok(Err(count)) => {
            // Rate limit exceeded
            Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!("Rate limit exceeded: {} requests/minute", count),
            ))
        }
        Err(_) => {
            // Redis error, fail open
            Ok(next.run(req).await)
        }
    }
}
