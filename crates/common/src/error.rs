use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommonError {
    #[error("invalid identifier or name: {0}")]
    InvalidName(String),

    #[error("invalid network address or hostname: {0}")]
    InvalidAddress(String),

    #[error("invalid CIDR range: {0}")]
    InvalidCidr(String),

    #[error("invalid SNMP OID: {0}")]
    InvalidOid(String),

    #[error("invalid port number: {0}")]
    InvalidPort(u32),

    #[error("invalid color hex value: {0}")]
    InvalidColor(String),

    #[error("invalid URL (HTTPS required): {0}")]
    InvalidUrl(String),

    #[error("invalid password policy: {0}")]
    InvalidPassword(String),

    #[error("unknown permission key: {0}")]
    UnknownPermission(String),
}
