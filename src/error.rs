use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Pin not found: {0}")]
    NotFoundPin(String),
    #[error("Invalid state: {0}")]
    InvalidState(String),
    #[error("Invalid value: {0}")]
    InvalidValue(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("GPIO error: {0}")]
    Gpio(String),
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::NotFoundPin(_) => StatusCode::NOT_FOUND,
            AppError::InvalidState(_) | AppError::InvalidValue(_) => StatusCode::BAD_REQUEST,
            AppError::PermissionDenied(_) => StatusCode::FORBIDDEN,
            AppError::Config(_) | AppError::Gpio(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code()).json(json!({ "error": self.to_string() }))
    }
}
