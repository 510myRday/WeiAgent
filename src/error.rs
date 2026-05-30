use std::net::SocketAddr;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("agent not found")]
    NotFound,
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("model provider API key is not configured")]
    MissingApiKey,
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
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::MissingApiKey => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Storage(_)
            | AppError::Json(_)
            | AppError::Llm(_)
            | AppError::Address(_)
            | AppError::AddressInUse(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = Json(ErrorBody {
            error: self.to_string(),
        });
        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
