use serde::ser::SerializeStruct;
use serde::Serialize;

/// Every error that crosses the IPC boundary. The UI receives `{ kind, message }` so it can
/// show what actually went wrong (bad key vs. offline vs. model returned garbage).
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("File error: {0}")]
    Io(#[from] std::io::Error),
    #[error("PDF error: {0}")]
    Pdf(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    RateLimited(String),
    #[error("{0}")]
    Provider(String),
    #[error("{0}")]
    InvalidOutput(String),
    #[error("{0}")]
    Secret(String),
    #[error("Cancelled")]
    Cancelled,
}

impl AppError {
    fn kind(&self) -> &'static str {
        match self {
            AppError::NotFound(_) => "notFound",
            AppError::Invalid(_) => "invalid",
            AppError::Db(_) => "db",
            AppError::Io(_) => "io",
            AppError::Pdf(_) => "pdf",
            AppError::Network(_) => "network",
            AppError::Auth(_) => "auth",
            AppError::RateLimited(_) => "rateLimited",
            AppError::Provider(_) => "provider",
            AppError::InvalidOutput(_) => "invalidOutput",
            AppError::Secret(_) => "secret",
            AppError::Cancelled => "cancelled",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Invalid(format!("JSON error: {e}"))
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            AppError::Network("The request timed out".into())
        } else if e.is_connect() {
            AppError::Network(format!("Could not connect: {e}"))
        } else {
            AppError::Network(e.to_string())
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
