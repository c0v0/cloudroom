//! Uniform API error type. Every error is serialized as
//! `{"error": {"code": "...", "message": "..."}}` with a stable machine code.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }

    pub fn bad_request(code: &'static str, msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, msg)
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", msg)
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", msg)
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", msg)
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", msg)
    }

    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", msg)
    }

    pub fn validation(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation_error", msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", msg)
    }

    pub fn room_not_found() -> Self {
        Self::not_found("房间不存在")
    }

    pub fn room_unavailable() -> Self {
        Self::new(StatusCode::GONE, "room_unavailable", "房间已删除或已过期")
    }

    pub fn sfu_required() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "sfu_required",
            "P2P 房间语音人数超过上限，请申请切换为 SFU 模式",
        )
    }

    pub fn sfu_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "sfu_unavailable",
            "LiveKit 未配置或不可达",
        )
    }

    pub fn media_mode_mismatch() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "media_mode_mismatch",
            "媒体接口与房间媒体模式不匹配",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "error": { "code": self.code, "message": self.message }
        }));
        (self.status, body).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                ApiError::conflict("数据约束冲突")
            }
            other => ApiError::internal(other.to_string()),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::bad_request("invalid_json", e.to_string())
    }
}

impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(r: axum::extract::rejection::JsonRejection) -> Self {
        ApiError::bad_request("invalid_json", r.body_text())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
