use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::net::IpAddr;

use crate::{services::rate_limiter::RateLimiter, state::AppState};

/// IP-based rate limiting middleware for public endpoints
pub async fn ip_rate_limit(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    // Extract IP from  connection info or X-Forwarded-For header
    let ip = get_client_ip(&req);

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

fn get_client_ip(req: &Request) -> IpAddr {
    // Try X-Forwarded-For first (for proxies)
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            // Take the first IP from the list
            if let Some(first_ip) = forwarded_str.split(',').next() {
                if let Ok(ip) = first_ip.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }
    }

    // Try X-Real-IP
    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(ip_str) = real_ip.to_str() {
            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                return ip;
            }
        }
    }

    // Fallback to connection remote address or localhost
    // In production with a reverse proxy, this would be the proxy's IP
    // which is why we check headers first
    "127.0.0.1".parse().unwrap()
}
