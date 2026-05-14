//! Shared collector error type.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CollectorError>;

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("collector config error: {0}")]
    Config(String),
    #[error("plugin {plugin} failed: {message}")]
    Plugin { plugin: String, message: String },
    #[error("ingest failed with status {status}: {body}")]
    Ingest { status: u16, body: String },
}
