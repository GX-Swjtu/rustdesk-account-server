use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct Rejected(pub StatusCode, pub &'static str);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.1)
    }
}
impl std::error::Error for Rejected {}
pub fn bad(message: &'static str) -> anyhow::Error {
    Rejected(StatusCode::BAD_REQUEST, message).into()
}
pub fn denied() -> anyhow::Error {
    Rejected(StatusCode::FORBIDDEN, "Access denied").into()
}
pub fn unauthorized() -> anyhow::Error {
    Rejected(StatusCode::UNAUTHORIZED, "Invalid credentials or session").into()
}
pub fn conflict(message: &'static str) -> anyhow::Error {
    Rejected(StatusCode::CONFLICT, message).into()
}
pub fn busy() -> anyhow::Error {
    Rejected(StatusCode::TOO_MANY_REQUESTS, "Please retry later").into()
}

pub struct ApiError(pub anyhow::Error);
impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self(value)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0.downcast_ref::<Rejected>() {
            Some(e) => (e.0, e.1),
            None => {
                eprintln!("Internal request failure (details intentionally omitted)");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
            }
        };
        let mut response = (status, Json(json!({"error":message}))).into_response();
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("60"));
        }
        response
    }
}
