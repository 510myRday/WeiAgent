use std::net::SocketAddr;
use uuid::Uuid;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("storage error: {0}")]
    Storage(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("llm request error: {0}")]
    Llm(String),
    #[error("address error: {0}")]
    Address(#[from] std::net::AddrParseError),
    #[error("address {0} is already in use")]
    AddressInUse(SocketAddr),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("process error: {0}")]
    Process(String),
    #[error("database error: {0}")]
    Database(String),
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Storage(_)
            | AppError::Json(_)
            | AppError::Llm(_)
            | AppError::Process(_)
            | AppError::Database(_)
            | AppError::Address(_)
            | AppError::AddressInUse(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let public_message = match status {
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::CONFLICT => {
                self.to_string()
            }
            _ => {
                let error_id = Uuid::new_v4();
                tracing::error!(%error_id, error = %self, "request failed");
                format!("internal server error (id: {error_id})")
            }
        };

        let body = Json(ErrorBody {
            error: public_message,
        });
        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
