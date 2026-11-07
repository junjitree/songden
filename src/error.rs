use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use tracing::error;

#[derive(Debug)]
pub enum Error {
    /// Shown to the person as-is.
    BadRequest(String),
    /// Something we depend on (Spotify, Jellyfin, Lidarr, Prowlarr) failed.
    Upstream(String),
    Internal(String),

    NotFound,
    TooManyRequests,
    Unauthorized,
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::Upstream(err.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::BadRequest(msg) | Self::Upstream(msg) | Self::Internal(msg) => msg.as_str(),
            Self::NotFound => "Not found",
            Self::TooManyRequests => "Too many tries. Wait a few minutes and try again.",
            Self::Unauthorized => "Sign in first",
        };
        write!(f, "{msg}")
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Upstream(err) => {
                error!("upstream: {err}");
                StatusCode::BAD_GATEWAY
            }
            Self::Internal(err) => {
                error!("internal: {err}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
        };
        // Internal details stay in the log; the page only needs to know it failed.
        let msg = match status {
            StatusCode::INTERNAL_SERVER_ERROR | StatusCode::BAD_GATEWAY => {
                "Something went wrong".to_string()
            }
            _ => self.to_string(),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}
