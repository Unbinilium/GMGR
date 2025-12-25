use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("pin not found: {0}")]
    NotFoundPin(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("invalid value: {0}")]
    InvalidValue(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("gpio error: {0}")]
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
        HttpResponse::build(self.status_code()).body(self.to_string())
    }
}
