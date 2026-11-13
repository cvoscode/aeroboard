use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Application failures preserve their source until the HTTP/Flight boundary.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{message}")]
    Http { status: StatusCode, message: String },
    #[error("{0}")]
    Catalog(#[from] rusqlite::Error),
    #[error("{0}")]
    Storage(#[from] crate::storage::BoxError),
    #[error("{0}")]
    Encoding(#[from] serde_json::Error),
    #[error("{0}")]
    Task(#[from] tokio::task::JoinError),
}
impl AppError {
    pub fn http(status: StatusCode, message: String) -> Self {
        Self::Http { status, message }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::http(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::http(StatusCode::CONFLICT, message.into())
    }
    pub fn missing() -> Self {
        Self::http(StatusCode::NOT_FOUND, "record not found".into())
    }
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Http { status, .. } => *status,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status(), Json(json!({"error":self.to_string()}))).into_response()
    }
}
impl From<AppError> for tonic::Status {
    fn from(error: AppError) -> Self {
        let code = match error.status() {
            StatusCode::BAD_REQUEST => tonic::Code::InvalidArgument,
            StatusCode::CONFLICT => tonic::Code::AlreadyExists,
            StatusCode::NOT_FOUND => tonic::Code::NotFound,
            StatusCode::GONE => tonic::Code::FailedPrecondition,
            _ => tonic::Code::Internal,
        };
        Self::new(code, error.to_string())
    }
}
