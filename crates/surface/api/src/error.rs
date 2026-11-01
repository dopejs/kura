//! API error type and its HTTP mapping.
//!
//! Port of the Go writeError helper (daemon/internal/api/server.go): errors are
//! serialized as {\"error\": message} with the matching HTTP status.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub const CODE_TENANT_MIGRATION_IN_PROGRESS: &str = "tenant_migration_in_progress";

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unprocessable(String),
    #[error("{0}")]
    Internal(String),
    /// An upstream (a model) answered with something unusable.
    #[error("{0}")]
    BadGateway(String),
    #[error("tenant migration in progress")]
    TenantMigrationInProgress,
}

impl ApiError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Unprocessable(_) => "unprocessable_entity",
            Self::Internal(_) => "internal",
            Self::BadGateway(_) => "bad_gateway",
            Self::TenantMigrationInProgress => CODE_TENANT_MIGRATION_IN_PROGRESS,
        }
    }

    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::TenantMigrationInProgress => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    #[must_use]
    pub fn from_store(message: String) -> Self {
        Self::Internal(message)
    }

    #[must_use]
    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::Internal(message.to_string())
    }
}

impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self::Internal(message)
    }
}

impl From<&str> for ApiError {
    fn from(message: &str) -> Self {
        Self::Internal(message.to_string())
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            code: self.code(),
            message: self.to_string(),
            error: self.to_string(),
        };
        (self.status(), axum::Json(body)).into_response()
    }
}
