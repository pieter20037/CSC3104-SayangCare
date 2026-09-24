use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Session already exists: {0}")]
    SessionConflict(String),

    #[error("Invalid state transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    #[error("Circuit breaker open for service: {0}")]
    CircuitOpen(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Telephony provider error: {0}")]
    Telephony(String),

    #[error("LLM inference failed: {0}")]
    Inference(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

pub type CoreResult<T> = Result<T, CoreError>;