use crate::error::ServerError;
use nextdude_common::{
    DEFAULT_GRPC_PORT, DEFAULT_MNDP_PORT, DEFAULT_STATUS_WEB_PORT, DEFAULT_SYSLOG_PORT,
    DEFAULT_TRAP_PORT,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub grpc_port: u16,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub tls_cert_path: Option<PathBuf>,
    pub tls_key_path: Option<PathBuf>,
    pub system_profiles_dir: PathBuf,
    pub override_profiles_dir: PathBuf,
    pub global_poll_interval_sec: u32,
    pub max_concurrent_probes: usize,
    pub max_concurrent_per_host: usize,
    pub flapping_changes: u32,
    pub flapping_window_min: u32,
    pub syslog_enabled: bool,
    pub syslog_bind_addr: String,
    pub syslog_port: u16,
    pub syslog_queue_capacity: usize,
    pub syslog_forward_addr: String,
    pub trap_enabled: bool,
    pub trap_bind_addr: String,
    pub trap_port: u16,
    pub mndp_enabled: bool,
    pub mndp_port: u16,
    pub status_web_enabled: bool,
    pub status_web_addr: String,
    pub status_web_port: u16,
    pub allow_image_url_import: bool,
    pub max_image_upload_bytes: usize,
    pub max_image_dimension_px: u32,
    pub retention_raw_hours: u32,
    pub retention_rollup_days: u32,
    pub retention_events_days: u32,
    pub retention_syslog_days: u32,
    pub retention_audit_days: u32,
    pub login_max_attempts: u32,
    pub login_lockout_sec: i64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0".to_string(),
            grpc_port: DEFAULT_GRPC_PORT,
            data_dir: PathBuf::from("/var/lib/nextdude"),
            log_dir: PathBuf::from("/var/log/nextdude"),
            tls_cert_path: None,
            tls_key_path: None,
            system_profiles_dir: PathBuf::from("/usr/share/nextdude/profiles"),
            override_profiles_dir: PathBuf::from("/etc/nextdude/profiles"),
            global_poll_interval_sec: 30,
            max_concurrent_probes: 256,
            max_concurrent_per_host: 6,
            flapping_changes: 5,
            flapping_window_min: 10,
            syslog_enabled: true,
            syslog_bind_addr: "0.0.0.0".to_string(),
            syslog_port: DEFAULT_SYSLOG_PORT,
            syslog_queue_capacity: 8192,
            syslog_forward_addr: String::new(),
            trap_enabled: true,
            trap_bind_addr: "0.0.0.0".to_string(),
            trap_port: DEFAULT_TRAP_PORT,
            mndp_enabled: true,
            mndp_port: DEFAULT_MNDP_PORT,
            status_web_enabled: true,
            status_web_addr: "127.0.0.1".to_string(),
            status_web_port: DEFAULT_STATUS_WEB_PORT,
            allow_image_url_import: false,
            max_image_upload_bytes: 2 * 1024 * 1024,
            max_image_dimension_px: 2048,
            retention_raw_hours: 48,
            retention_rollup_days: 90,
            retention_events_days: 180,
            retention_syslog_days: 30,
            retention_audit_days: 365,
            login_max_attempts: 5,
            login_lockout_sec: 300,
        }
    }
}

impl ServerConfig {
    pub fn load_from_path(path: &Path) -> Result<Self, ServerError> {
        if !path.exists() {
            return Err(ServerError::Config(format!(
                "configuration file not found: {}",
                path.display()
            )));
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| ServerError::Config(format!("failed reading {}: {e}", path.display())))?;
        let cfg: Self = toml::from_str(&raw)
            .map_err(|e| ServerError::Config(format!("invalid TOML in {}: {e}", path.display())))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ServerError> {
        if self.grpc_port == 0 {
            return Err(ServerError::Config("grpc_port cannot be 0".to_string()));
        }
        if self.global_poll_interval_sec < 5 {
            return Err(ServerError::Config(
                "global_poll_interval_sec must be >= 5".to_string(),
            ));
        }
        if self.max_concurrent_probes == 0 || self.max_concurrent_per_host == 0 {
            return Err(ServerError::Config(
                "concurrency limits must be >= 1".to_string(),
            ));
        }
        if self.syslog_queue_capacity < 64 {
            return Err(ServerError::Config(
                "syslog_queue_capacity must be >= 64".to_string(),
            ));
        }
        if self.max_image_upload_bytes == 0 || self.max_image_upload_bytes > 16 * 1024 * 1024 {
            return Err(ServerError::Config(
                "max_image_upload_bytes must be within 1..=16MiB".to_string(),
            ));
        }
        if self.tls_cert_path.is_some() != self.tls_key_path.is_some() {
            return Err(ServerError::Config(
                "tls_cert_path and tls_key_path must both be set or both omitted".to_string(),
            ));
        }
        if self.status_web_enabled {
            if self.status_web_port == 0 {
                return Err(ServerError::Config(
                    "status_web_port cannot be 0".to_string(),
                ));
            }
            if self.status_web_port == self.grpc_port {
                return Err(ServerError::Config(
                    "status_web_port cannot match grpc_port".to_string(),
                ));
            }
            if self.status_web_addr.parse::<std::net::IpAddr>().is_err() {
                return Err(ServerError::Config(format!(
                    "invalid status_web_addr IP '{}'",
                    self.status_web_addr
                )));
            }
        }
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("nextdude.db")
    }

    pub fn master_key_path(&self) -> PathBuf {
        self.data_dir.join("master.key")
    }

    pub fn effective_tls_cert_path(&self) -> PathBuf {
        self.tls_cert_path
            .clone()
            .unwrap_or_else(|| self.data_dir.join("certs").join("server.crt.pem"))
    }

    pub fn effective_tls_key_path(&self) -> PathBuf {
        self.tls_key_path
            .clone()
            .unwrap_or_else(|| self.data_dir.join("certs").join("server.key.pem"))
    }

    pub fn images_dir(&self) -> PathBuf {
        self.data_dir.join("images")
    }
}
