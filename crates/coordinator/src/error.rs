//! The one error type of the HTTP API: `{"error": "<code>", "message": "..."}` with a status (docs/JOBS.md).
//! A 429 also carries `retry_after` (seconds) in the body and a `Retry-After` header.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// Seconds until a retry can succeed: `retry_after` in the body and a `Retry-After` header.
    pub retry_after: Option<u64>,
    /// More fields merged into the JSON body (e.g. `reason` and `until` of a suspension).
    pub details: Option<serde_json::Map<String, serde_json::Value>>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            code,
            message: message.into(),
            retry_after: None,
            details: None,
        }
    }
    /// 429 with `retry_after` seconds (at least 1) in the body and the `Retry-After` header.
    pub fn rate_limited(
        code: &'static str,
        message: impl Into<String>,
        retry_after: u64,
    ) -> ApiError {
        ApiError {
            retry_after: Some(retry_after.max(1)),
            ..ApiError::new(StatusCode::TOO_MANY_REQUESTS, code, message)
        }
    }
    pub fn bad_request(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    pub fn unauthorized() -> ApiError {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or invalid bearer token",
        )
    }
    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, "not_found", message)
    }
    pub fn conflict(code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::CONFLICT, code, message)
    }
    /// 409: wrong or expired lease token. The worker must drop the job.
    pub fn lease_lost() -> ApiError {
        ApiError::conflict("lease_lost", "wrong or expired lease_token; drop the job")
    }
    pub fn internal(e: impl std::fmt::Display) -> ApiError {
        eprintln!("internal error: {e}");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> ApiError {
        ApiError::internal(format!("sqlite: {e}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> ApiError {
        ApiError::internal(format!("io: {e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({"error": self.code, "message": self.message});
        if let (Some(details), Some(obj)) = (self.details, body.as_object_mut()) {
            obj.extend(details);
        }
        let Some(secs) = self.retry_after else {
            return (self.status, Json(body)).into_response();
        };
        body["retry_after"] = json!(secs);
        (
            self.status,
            [(header::RETRY_AFTER, secs.to_string())],
            Json(body),
        )
            .into_response()
    }
}
