use std::path::PathBuf;

/// All errors that can occur in the harness.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("agent binary not found: {binary} (is {agent} installed?)")]
    BinaryNotFound { agent: String, binary: String },

    #[error("failed to spawn agent process: {0}")]
    SpawnFailed(#[source] std::io::Error),

    #[error("agent process failed with exit code {code}: {stderr}")]
    ProcessFailed { code: i32, stderr: String },

    #[error("failed to parse agent output: {0}")]
    ParseError(String),

    #[error("agent timed out after {0} seconds")]
    Timeout(u64),

    #[error("working directory does not exist: {0}")]
    InvalidWorkDir(PathBuf),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("failed to parse models registry: {0}")]
    ModelsParse(String),

    #[error("failed to fetch models registry: {0}")]
    ModelsFetch(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Stable error code string for programmatic consumption.
    pub fn code(&self) -> &'static str {
        match self {
            Error::BinaryNotFound { .. } => "E001",
            Error::SpawnFailed(_) => "E002",
            Error::ProcessFailed { .. } => "E003",
            Error::ParseError(_) => "E004",
            Error::Timeout(_) => "E005",
            Error::InvalidWorkDir(_) => "E006",
            Error::Io(_) => "E007",
            Error::Json(_) => "E008",
            Error::ModelsParse(_) => "E010",
            Error::ModelsFetch(_) => "E011",
            Error::Other(_) => "E999",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
