#[derive(Debug, thiserror::Error)]
pub enum OverleafError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected status {status} for {url}: {body}")]
    Status { status: u16, url: String, body: String },
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("realtime protocol error: {0}")]
    Protocol(String),
    #[error("document out of sync: {0}")]
    OutOfSync(String),
    #[error("edit rejected: {0}")]
    Edit(String),
    #[error("access denied: {0}")]
    Denied(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("timed out: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, OverleafError>;
