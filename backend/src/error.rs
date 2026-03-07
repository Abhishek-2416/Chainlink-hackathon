use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("Blockchain error: {0}")]
    Blockchain(String),

    #[error("Invalid order: {0}")]
    InvalidOrder(String),

    #[error("Market not found: {0}")]
    MarketNotFound(i32),

    #[error("Order not found: {0}")]
    OrderNotFound(i32),

    #[error("Similar market exists: {0}")]
    SimilarMarketExists(String),

    #[error("Order expired")]
    OrderExpired,

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Insufficient allowance")]
    InsufficientAllowance,

    #[error("Bad request: {0}")]
    BadRequest(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Db(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            AppError::Blockchain(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::InvalidOrder(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::MarketNotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::OrderNotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::SimilarMarketExists(_) => (StatusCode::CONFLICT, self.to_string()),
            AppError::OrderExpired => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::InvalidSignature => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::InsufficientAllowance => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}
