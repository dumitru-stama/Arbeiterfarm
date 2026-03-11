use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    BadRequest(String),
    PayloadTooLarge(String),
    QuotaExceeded(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg),
            ApiError::QuotaExceeded(msg) => (StatusCode::TOO_MANY_REQUESTS, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = axum::Json(json!({ "error": message }));
        (status, body).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        // Log full details server-side but return a generic message to the client
        // to avoid leaking table names, constraint names, or query details.
        tracing::error!("database error: {e}");
        // Surface unique violation as a 409-style bad request (common case: duplicate insert)
        if let sqlx::Error::Database(ref db_err) = e {
            if db_err.code().as_deref() == Some("23505") {
                return ApiError::BadRequest("duplicate entry".into());
            }
            // Check constraint violation → bad request
            if db_err.code().as_deref() == Some("23514") {
                return ApiError::BadRequest("invalid value".into());
            }
            // Foreign key violation
            if db_err.code().as_deref() == Some("23503") {
                return ApiError::BadRequest("referenced entity not found".into());
            }
        }
        ApiError::Internal("internal database error".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_forbidden_response() {
        let err = ApiError::Forbidden("not a project member".into());
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "not a project member");
    }

    #[tokio::test]
    async fn test_quota_exceeded_response() {
        let err = ApiError::QuotaExceeded("daily LLM token quota exceeded".into());
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "daily LLM token quota exceeded");
    }
}
