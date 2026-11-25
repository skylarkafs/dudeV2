use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientCoreError {
    #[error("connection error: {0}")]
    Connection(String),

    #[error("TLS TOFU confirmation required for {host} (fingerprint: {fingerprint})")]
    TofuFirstUse { host: String, fingerprint: String },

    #[error("TLS certificate fingerprint CHANGED for {host}: old={previous}, new={current}")]
    TofuMismatch {
        host: String,
        previous: String,
        current: String,
    },

    #[error("gRPC error: {0}")]
    Grpc(String),

    #[error("configuration I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
