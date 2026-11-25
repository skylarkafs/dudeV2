use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("cryptography error: {0}")]
    Crypto(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("optimistic concurrency conflict: expected version {expected}, current version {current}")]
    VersionConflict { expected: u64, current: u64 },

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("account temporarily locked until epoch {0}")]
    AccountLocked(i64),

    #[error("permission denied: missing {0}")]
    PermissionDenied(String),

    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("validation error: {0}")]
    Validation(#[from] nextdude_common::error::CommonError),

    #[error("SNMP protocol error: {0}")]
    Snmp(String),

    #[error("probe execution error: {0}")]
    Probe(String),

    #[error("image validation error: {0}")]
    Image(String),

    #[error("network/I/O error: {0}")]
    Io(#[from] std::io::Error),
}
