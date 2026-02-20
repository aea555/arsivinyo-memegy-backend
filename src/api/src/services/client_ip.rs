use std::net::{IpAddr, SocketAddr};

use axum::{
    async_trait,
    extract::{ConnectInfo, FromRequestParts},
    http::{Extensions, HeaderMap, request::Parts},
};

use crate::{error::ApiErrorResponse, state::AppState};

pub struct ClientIp(pub IpAddr);

fn parse_ip_header(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<IpAddr>, ApiErrorResponse> {
    let value = match headers.get(name) {
        Some(value) => value,
        None => return Ok(None),
    };
    let raw = value
        .to_str()
        .map_err(|_| ApiErrorResponse::bad_request(format!("Malformed {} header", name)))?;
    let ip = raw
        .parse::<IpAddr>()
        .map_err(|_| ApiErrorResponse::bad_request(format!("Malformed {} header", name)))?;
    Ok(Some(ip))
}

fn connect_ip_from_extensions(extensions: &Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .or_else(|| extensions.get::<SocketAddr>().map(|addr| addr.ip()))
}

pub fn extract_client_ip_from_headers_and_extensions(
    headers: &HeaderMap,
    extensions: &Extensions,
    environment: &str,
    require_cloudflare_headers: bool,
) -> Result<IpAddr, ApiErrorResponse> {
    if let Some(ip) = parse_ip_header(headers, "cf-connecting-ip")? {
        return Ok(ip);
    }
    if let Some(ip) = parse_ip_header(headers, "x-real-ip")? {
        return Ok(ip);
    }

    if environment == "production" || require_cloudflare_headers {
        return Err(ApiErrorResponse::forbidden(
            "Direct access not allowed. Use Cloudflare endpoint.",
        ));
    }

    Ok(connect_ip_from_extensions(extensions).unwrap_or_else(|| {
        "127.0.0.1"
            .parse::<IpAddr>()
            .expect("localhost IP literal should be valid")
    }))
}

#[async_trait]
impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let ip = extract_client_ip_from_headers_and_extensions(
            &parts.headers,
            &parts.extensions,
            &state.config.environment,
            state.config.require_cloudflare_headers,
        )?;
        Ok(Self(ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header::HeaderName};

    fn header(name: &'static str) -> HeaderName {
        HeaderName::from_static(name)
    }

    #[test]
    fn cf_connecting_ip_overrides_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header("cf-connecting-ip"),
            HeaderValue::from_static("1.1.1.1"),
        );
        headers.insert(header("x-real-ip"), HeaderValue::from_static("2.2.2.2"));
        let ip = extract_client_ip_from_headers_and_extensions(
            &headers,
            &Extensions::new(),
            "production",
            true,
        )
        .expect("ip should parse");
        assert_eq!(ip.to_string(), "1.1.1.1");
    }

    #[test]
    fn x_real_ip_used_when_cf_missing() {
        let mut headers = HeaderMap::new();
        headers.insert(header("x-real-ip"), HeaderValue::from_static("2.2.2.2"));
        let ip = extract_client_ip_from_headers_and_extensions(
            &headers,
            &Extensions::new(),
            "production",
            true,
        )
        .expect("ip should parse");
        assert_eq!(ip.to_string(), "2.2.2.2");
    }

    #[test]
    fn malformed_cf_header_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header("cf-connecting-ip"),
            HeaderValue::from_static("not_an_ip"),
        );
        let err = extract_client_ip_from_headers_and_extensions(
            &headers,
            &Extensions::new(),
            "production",
            true,
        )
        .expect_err("invalid header should fail");
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn missing_headers_rejected_in_strict_mode() {
        let err = extract_client_ip_from_headers_and_extensions(
            &HeaderMap::new(),
            &Extensions::new(),
            "production",
            true,
        )
        .expect_err("missing headers should fail");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn x_forwarded_for_is_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header("x-forwarded-for"),
            HeaderValue::from_static("9.9.9.9, 8.8.8.8"),
        );
        let ip = extract_client_ip_from_headers_and_extensions(
            &headers,
            &Extensions::new(),
            "test",
            false,
        )
        .expect("fallback should work");
        assert_eq!(ip.to_string(), "127.0.0.1");
    }
}
