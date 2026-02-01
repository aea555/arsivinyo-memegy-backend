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
    // Extract IP with environment-aware logic
    let ip = match get_client_ip(
        &req,
        &state.config.environment,
        state.config.require_cloudflare_headers,
    ) {
        Ok(ip) => ip,
        Err(err) => return Err(err),
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

/// Environment-aware IP extraction
/// Production: Strict CF-Connecting-IP only (fail closed)
/// Development: Permissive fallback chain (local testing)
fn get_client_ip(
    req: &Request,
    environment: &str,
    require_cf_headers: bool,
) -> Result<IpAddr, (StatusCode, String)> {
    // Production or explicit CF requirement: STRICT mode
    if environment == "production" || require_cf_headers {
        // 1. CF-Connecting-IP (ONLY trusted source in production)
        if let Some(cf_ip) = req.headers().get("cf-connecting-ip") {
            if let Ok(ip_str) = cf_ip.to_str() {
                if let Ok(ip) = ip_str.parse::<IpAddr>() {
                    return Ok(ip);
                }
            }
            // CF header exists but malformed - REJECT (security)
            tracing::error!("Malformed CF-Connecting-IP header in production");
            return Err((
                StatusCode::BAD_REQUEST,
                "Malformed CF-Connecting-IP header".to_string(),
            ));
        }

        // Missing CF header in production - REJECT (bypass attempt)
        tracing::error!("Missing CF-Connecting-IP in production - possible bypass attempt");
        return Err((
            StatusCode::FORBIDDEN,
            "Direct access not allowed. Use Cloudflare endpoint.".to_string(),
        ));
    }

    // Development/Staging: Permissive fallback chain
    // 1. Try CF-Connecting-IP first (if testing with Cloudflare)
    if let Some(cf_ip) = req.headers().get("cf-connecting-ip") {
        if let Ok(ip_str) = cf_ip.to_str() {
            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                return Ok(ip);
            }
        }
    }

    // 2. X-Real-IP (local reverse proxy)
    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(ip_str) = real_ip.to_str() {
            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                return Ok(ip);
            }
        }
    }

    // 3. X-Forwarded-For (ONLY in dev - take first IP)
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            if let Some(first_ip) = forwarded_str.split(',').next() {
                if let Ok(ip) = first_ip.trim().parse::<IpAddr>() {
                    return Ok(ip);
                }
            }
        }
    }

    // 4. Fallback to localhost (local development)
    Ok("127.0.0.1".parse().unwrap())
}
