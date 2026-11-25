pub mod error;
pub mod oid_dict;
pub mod proto;
pub mod types;
pub mod validation;

/// Working product name stored in a single constant for straightforward rebranding.
pub const APP_NAME: &str = "NextDude";

/// Windows AppUserModelID used for Start Menu shortcut and Toast notifications.
pub const APP_USER_MODEL_ID: &str = "NextDude.Client.Desktop.1";

/// Protocol compatibility version verified during Login handshake.
pub const API_VERSION: u32 = 1;

/// Default gRPC-over-TLS server port.
pub const DEFAULT_GRPC_PORT: u16 = 9443;

/// Default syslog UDP receiver port.
pub const DEFAULT_SYSLOG_PORT: u16 = 514;

/// Default SNMP trap/inform UDP receiver port.
pub const DEFAULT_TRAP_PORT: u16 = 162;

/// MikroTik Neighbor Discovery Protocol (MNDP) UDP port.
pub const DEFAULT_MNDP_PORT: u16 = 5678;

/// Default HTTP port for the lightweight read-only server status page.
pub const DEFAULT_STATUS_WEB_PORT: u16 = 9080;
