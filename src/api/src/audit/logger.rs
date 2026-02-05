use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub enum AuditEvent {
    OAuthLoginInitiated {
        source: String,
        has_pkce: bool,
        timestamp: DateTime<Utc>,
    },
    OAuthCallbackSuccess {
        user_id: Uuid,
        source: String,
        pkce_validated: bool,
        timestamp: DateTime<Utc>,
    },
    OAuthCallbackFailure {
        error: String,
        source: Option<String>,
        timestamp: DateTime<Utc>,
    },
    PkceValidationFailed {
        source: String,
        timestamp: DateTime<Utc>,
    },
    OtcExchangeSuccess {
        user_id: Uuid,
        client_ip: String,
        timestamp: DateTime<Utc>,
    },
    OtcExchangeFailure {
        client_ip: String,
        error: String,
        timestamp: DateTime<Utc>,
    },
    RateLimitExceeded {
        client_ip: String,
        endpoint: String,
        timestamp: DateTime<Utc>,
    },
}

pub fn log_audit_event(event: AuditEvent) {
    // Structured logging - can be extended to external audit service
    match event {
        AuditEvent::OAuthLoginInitiated {
            source,
            has_pkce,
            timestamp,
        } => {
            tracing::info!(
                event = "oauth_login_initiated",
                source = %source,
                has_pkce = %has_pkce,
                timestamp = %timestamp,
                "OAuth login flow initiated"
            );
        }
        AuditEvent::OAuthCallbackSuccess {
            user_id,
            source,
            pkce_validated,
            timestamp,
        } => {
            tracing::info!(
                event = "oauth_callback_success",
                user_id = %user_id,
                source = %source,
                pkce_validated = %pkce_validated,
                timestamp = %timestamp,
                "OAuth callback successful"
            );
        }
        AuditEvent::OAuthCallbackFailure {
            error,
            source,
            timestamp,
        } => {
            tracing::warn!(
                event = "oauth_callback_failure",
                error = %error,
                source = ?source,
                timestamp = %timestamp,
                "OAuth callback failed"
            );
        }
        AuditEvent::PkceValidationFailed { source, timestamp } => {
            tracing::warn!(
                event = "pkce_validation_failed",
                source = %source,
                timestamp = %timestamp,
                "PKCE validation failed"
            );
        }
        AuditEvent::OtcExchangeSuccess {
            user_id,
            client_ip,
            timestamp,
        } => {
            tracing::info!(
                event = "otc_exchange_success",
                user_id = %user_id,
                client_ip = %client_ip,
                timestamp = %timestamp,
                "OTC exchange successful"
            );
        }
        AuditEvent::OtcExchangeFailure {
            client_ip,
            error,
            timestamp,
        } => {
            tracing::warn!(
                event = "otc_exchange_failure",
                client_ip = %client_ip,
                error = %error,
                timestamp = %timestamp,
                "OTC exchange failed"
            );
        }
        AuditEvent::RateLimitExceeded {
            client_ip,
            endpoint,
            timestamp,
        } => {
            tracing::warn!(
                event = "rate_limit_exceeded",
                client_ip = %client_ip,
                endpoint = %endpoint,
                timestamp = %timestamp,
                "Rate limit exceeded"
            );
        }
    }
}
