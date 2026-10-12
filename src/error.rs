use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct Error(pub StatusCode, pub String);
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "not found".into())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"success":false,"reason":self.1}))).into_response()
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        if matches!(error, sqlx::Error::RowNotFound) {
            return Self::missing();
        }
        if let sqlx::Error::Database(ref db) = error
            && (db.is_foreign_key_violation() || db.is_check_violation())
        {
            return Self::bad("invalid reference or value");
        }
        tracing::error!(%error, "database operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database operation failed".into(),
        )
    }
}
impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Self {
        // Do not log upstream error bodies or URLs: they can contain credentials.
        let _ = error;
        tracing::error!("media or storage operation failed");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "operation failed".into())
    }
}
