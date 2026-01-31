use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Standardized API error response
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl ApiError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            details: None,
        }
    }

    pub fn with_details(error: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            details: Some(details.into()),
        }
    }
}

/// API result type that returns either a value or an ApiErrorResponse
pub type ApiResult<T> = Result<T, ApiErrorResponse>;

/// Wrapper around ApiError with HTTP status code
#[derive(Debug)]
pub struct ApiErrorResponse {
    pub status: StatusCode,
    pub error: ApiError,
}

impl ApiErrorResponse {
    pub fn new(status: StatusCode, error: impl Into<String>) -> Self {
        Self {
            status,
            error: ApiError::new(error),
        }
    }

    pub fn with_details(
        status: StatusCode,
        error: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            status,
            error: ApiError::with_details(error, details),
        }
    }

    // Convenience constructors for common errors
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    #[allow(dead_code)]
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message.into())
    }

    #[allow(dead_code)]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message.into())
    }

    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for ApiErrorResponse {
    fn into_response(self) -> Response {
        (self.status, Json(self.error)).into_response()
    }
}

// Convert anyhow::Error to ApiErrorResponse
impl From<anyhow::Error> for ApiErrorResponse {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!("Internal error: {:?}", err);

        // In debug mode, include error details
        if cfg!(debug_assertions) {
            Self::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error",
                format!("{:?}", err),
            )
        } else {
            Self::internal_error("Internal server error")
        }
    }
}

// Convert sea_orm::DbErr to ApiErrorResponse
impl From<sea_orm::DbErr> for ApiErrorResponse {
    fn from(err: sea_orm::DbErr) -> Self {
        tracing::error!("Database error: {:?}", err);

        if cfg!(debug_assertions) {
            Self::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error",
                err.to_string(),
            )
        } else {
            Self::internal_error("Internal server error")
        }
    }
}
