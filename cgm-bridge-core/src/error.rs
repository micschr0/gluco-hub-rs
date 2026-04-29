use thiserror::Error;

/// Library-level error for cgm-bridge-core.
///
/// Each variant carries a stable `error_code` (the bracketed prefix in
/// `Display`) so logs can be filtered without parsing free-form messages.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("[CORE001] invalid glucose value: {value} mg/dL")]
    InvalidGlucose { value: f64 },

    #[error("[CORE002] invalid identifier: {kind}: {reason}")]
    InvalidId { kind: &'static str, reason: String },

    #[error("[CORE003] source error: {message}")]
    Source { message: String },

    #[error("[CORE004] sink error: {message}")]
    Sink { message: String },
}

impl CoreError {
    pub fn error_code(&self) -> &'static str {
        match self {
            CoreError::InvalidGlucose { .. } => "CORE001",
            CoreError::InvalidId { .. } => "CORE002",
            CoreError::Source { .. } => "CORE003",
            CoreError::Sink { .. } => "CORE004",
        }
    }
}
