use std::{collections::HashSet, net::IpAddr, str::FromStr, time::UNIX_EPOCH};

use axum::http::{StatusCode, request::Parts};
use ipnet::IpNet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;

use crate::{error::ApiErrorResponse, state::AppState};

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AudClaim {
    One(String),
    Many(Vec<String>),
}

impl AudClaim {
    fn contains(&self, audience: &str) -> bool {
        match self {
            AudClaim::One(value) => value == audience,
            AudClaim::Many(values) => values.iter().any(|value| value == audience),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminClaims {
    pub iss: String,
    pub sub: String,
    pub aud: AudClaim,
    pub jti: String,
    pub iat: usize,
    pub nbf: usize,
    pub exp: usize,
    pub role: String,
}

pub fn extract_client_ip(
    parts: &Parts,
    environment: &str,
    require_cloudflare_headers: bool,
) -> Result<IpAddr, ApiErrorResponse> {
    if environment == "production" || require_cloudflare_headers {
        if let Some(cf_ip) = parts.headers.get("cf-connecting-ip") {
            let ip = cf_ip
                .to_str()
                .ok()
                .and_then(|v| v.parse::<IpAddr>().ok())
                .ok_or_else(|| {
                    ApiErrorResponse::bad_request("Malformed CF-Connecting-IP header")
                })?;
            return Ok(ip);
        }
        return Err(ApiErrorResponse::forbidden(
            "Direct access not allowed. Use Cloudflare endpoint.",
        ));
    }

    if let Some(cf_ip) = parts.headers.get("cf-connecting-ip") {
        if let Ok(ip) = cf_ip.to_str().unwrap_or_default().parse::<IpAddr>() {
            return Ok(ip);
        }
    }
    if let Some(real_ip) = parts.headers.get("x-real-ip") {
        if let Ok(ip) = real_ip.to_str().unwrap_or_default().parse::<IpAddr>() {
            return Ok(ip);
        }
    }
    if let Some(forwarded) = parts.headers.get("x-forwarded-for") {
        if let Some(first_ip) = forwarded
            .to_str()
            .ok()
            .and_then(|raw| raw.split(',').next())
            .map(str::trim)
        {
            if let Ok(ip) = first_ip.parse::<IpAddr>() {
                return Ok(ip);
            }
        }
    }

    Ok("127.0.0.1".parse().expect("valid localhost IP"))
}

fn enforce_ip_allowlist(configured: &str, ip: IpAddr) -> Result<(), ApiErrorResponse> {
    let cidrs: Vec<&str> = configured
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();

    if cidrs.is_empty() {
        return Ok(());
    }

    let mut allowed = false;
    for cidr in cidrs {
        let net = IpNet::from_str(cidr).map_err(|_| {
            ApiErrorResponse::internal_error("Invalid ADMIN_ALLOWED_IP_CIDRS configuration")
        })?;
        if net.contains(&ip) {
            allowed = true;
            break;
        }
    }

    if allowed {
        Ok(())
    } else {
        Err(ApiErrorResponse::new(
            StatusCode::FORBIDDEN,
            "Admin IP is not allowed",
        ))
    }
}

pub async fn verify_admin_jwt(
    state: &AppState,
    token: &str,
    client_ip: IpAddr,
) -> Result<AdminClaims, ApiErrorResponse> {
    let issuer =
        state.config.admin_jwt_issuer.as_ref().ok_or_else(|| {
            ApiErrorResponse::internal_error("Admin JWT issuer is not configured")
        })?;
    let audience =
        state.config.admin_jwt_audience.as_ref().ok_or_else(|| {
            ApiErrorResponse::internal_error("Admin JWT audience is not configured")
        })?;

    enforce_ip_allowlist(&state.config.admin_allowed_ip_cidrs, client_ip)?;

    let header = decode_header(token)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid admin token header"))?;
    if header.alg != Algorithm::EdDSA {
        return Err(ApiErrorResponse::unauthorized(
            "Invalid admin token algorithm",
        ));
    }

    let kid = header
        .kid
        .ok_or_else(|| ApiErrorResponse::unauthorized("Admin token missing kid"))?;
    let public_key_pem = state
        .config
        .admin_jwt_public_keys
        .get(&kid)
        .ok_or_else(|| ApiErrorResponse::unauthorized("Unknown admin key id"))?;

    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.leeway = state.config.admin_jwt_clock_skew_secs as u64;
    validation.validate_nbf = true;
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    validation.required_spec_claims = HashSet::from_iter(
        ["iss", "aud", "sub", "jti", "iat", "nbf", "exp"]
            .into_iter()
            .map(str::to_string),
    );

    let decoding_key = DecodingKey::from_ed_pem(public_key_pem.as_bytes())
        .map_err(|_| ApiErrorResponse::internal_error("Invalid admin public key configuration"))?;
    let token_data = decode::<AdminClaims>(token, &decoding_key, &validation)
        .map_err(|_| ApiErrorResponse::unauthorized("Invalid admin token"))?;
    let claims = token_data.claims;

    if claims.role != "superadmin" {
        return Err(ApiErrorResponse::new(
            StatusCode::FORBIDDEN,
            "Admin role is required",
        ));
    }
    if claims.iss != *issuer {
        return Err(ApiErrorResponse::unauthorized("Invalid admin token issuer"));
    }
    if !claims.aud.contains(audience) {
        return Err(ApiErrorResponse::unauthorized(
            "Invalid admin token audience",
        ));
    }
    if claims.exp <= claims.iat {
        return Err(ApiErrorResponse::unauthorized(
            "Invalid admin token lifetime",
        ));
    }
    if claims.exp - claims.iat > state.config.admin_jwt_max_ttl_secs {
        return Err(ApiErrorResponse::unauthorized(
            "Admin token TTL exceeds policy",
        ));
    }
    if claims.nbf > claims.exp {
        return Err(ApiErrorResponse::unauthorized(
            "Invalid admin token validity range",
        ));
    }

    if state.config.admin_replay_protection_enabled {
        let now_secs = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as usize)
            .unwrap_or(0);
        let replay_ttl = claims.exp.saturating_sub(now_secs).max(1) as i64;
        let replay_key = format!("admin:replay:jti:{}", claims.jti);

        let mut conn = state.queue.get_conn().await.map_err(|_| {
            ApiErrorResponse::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Admin auth temporarily unavailable",
            )
        })?;

        let set_result: Option<String> = redis::cmd("SET")
            .arg(&replay_key)
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(replay_ttl)
            .query_async(&mut conn)
            .await
            .map_err(|_| {
                ApiErrorResponse::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Admin auth temporarily unavailable",
                )
            })?;

        if set_result.is_none() {
            return Err(ApiErrorResponse::unauthorized(
                "Admin token replay detected",
            ));
        }
    }

    Ok(claims)
}
