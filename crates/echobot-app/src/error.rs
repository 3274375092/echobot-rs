//! `AppError` — unified error type for the HTTP layer.
//!
//! Mirrors the `HTTPException` behaviour from the Python app: each
//! variant maps to a status code and a JSON body containing a `detail`
//! string. `IntoResponse` is implemented so handlers can return
//! `Result<T, AppError>`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

/// All errors that can be returned from an HTTP handler.
#[derive(Debug, Error)]
pub enum AppError {
    /// Resource was not found. Maps to HTTP 404.
    #[error("not found: {0}")]
    NotFound(String),
    /// Invalid input from the client. Maps to HTTP 400.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Resource conflict (e.g. role already exists). Maps to HTTP 409.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Service temporarily unavailable. Maps to HTTP 503.
    #[error("service unavailable: {0}")]
    Unavailable(String),
    /// Upstream / runtime failure. Maps to HTTP 502.
    #[error("upstream error: {0}")]
    Upstream(String),
    /// Internal error. Maps to HTTP 500.
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({ "detail": self.to_string() }));
        (status, body).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}
