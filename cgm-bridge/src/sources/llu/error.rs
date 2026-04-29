use thiserror::Error;

/// Errors surfaced by the LibreLink Up auth client. Each variant carries a
/// stable `error_code` (the bracketed prefix in `Display`) so logs and
/// metrics labels stay grep-friendly across versions.
#[derive(Debug, Error)]
pub enum LluError {
    #[error("[LLU001] HTTP transport error: {0}")]
    Transport(String),

    #[error("[LLU002] LLU returned non-success status: {status}")]
    Status { status: i64 },

    #[error("[LLU003] invalid credentials")]
    InvalidCredentials,

    #[error("[LLU004] malformed response body: {reason}")]
    Protocol { reason: String },

    #[error("[LLU005] region redirect loop or too many redirects")]
    RedirectLoop,

    #[error("[LLU006] unknown LibreLink Up region: {value}")]
    UnknownRegion { value: String },

    #[error("[LLU007] could not parse LLU timestamp: {raw}")]
    BadTimestamp { raw: String },

    #[error("[LLU008] LLU rejected token on {endpoint}: 401")]
    Unauthorized { endpoint: &'static str },
}

impl LluError {
    pub fn error_code(&self) -> &'static str {
        match self {
            LluError::Transport(_) => "LLU001",
            LluError::Status { .. } => "LLU002",
            LluError::InvalidCredentials => "LLU003",
            LluError::Protocol { .. } => "LLU004",
            LluError::RedirectLoop => "LLU005",
            LluError::UnknownRegion { .. } => "LLU006",
            LluError::BadTimestamp { .. } => "LLU007",
            LluError::Unauthorized { .. } => "LLU008",
        }
    }
}

impl From<reqwest::Error> for LluError {
    fn from(value: reqwest::Error) -> Self {
        LluError::Transport(value.to_string())
    }
}
