use crate::error::ClientCoreError;
use crate::i18n::Language;
use crate::theme::ThemeMode;
use nextdude_common::DEFAULT_GRPC_PORT;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedServerEntry {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub comment: String,
    pub remember_password: bool,
    /// DPAPI-encrypted password bytes encoded as Base64 (protected per CurrentUser on Windows).
    pub encrypted_password_b64: String,
}

impl Default for SavedServerEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: String::new(),
            port: DEFAULT_GRPC_PORT,
            username: String::new(),
            comment: String::new(),
            remember_password: false,
            encrypted_password_b64: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientLocalConfig {
    pub servers: Vec<SavedServerEntry>,
    pub last_server_id: Option<String>,
    pub auto_connect_last: bool,
    pub known_fingerprints: HashMap<String, String>,
    pub language: Language,
    pub theme: ThemeMode,
    pub close_to_tray: bool,
    pub prevent_sleep: bool,
    pub autostart_on_login: bool,
    pub global_mute: bool,
    pub external_tools: HashMap<String, String>,
}

impl Default for ClientLocalConfig {
    fn default() -> Self {
        let mut external_tools = HashMap::new();
        external_tools.insert("winbox".to_string(), r"C:\Program Files\MikroTik\WinBox\winbox64.exe".to_string());
        external_tools.insert("ssh".to_string(), r"C:\Program Files\PuTTY\putty.exe".to_string());
        external_tools.insert("telnet".to_string(), r"C:\Program Files\PuTTY\putty.exe".to_string());
        Self {
            servers: Vec::new(),
            last_server_id: None,
            auto_connect_last: true,
            known_fingerprints: HashMap::new(),
            language: Language::Id,
            theme: ThemeMode::Dark,
            close_to_tray: true,
            prevent_sleep: false,
            autostart_on_login: false,
            global_mute: false,
            external_tools,
        }
    }
}

impl ClientLocalConfig {
    pub fn default_data_dir() -> PathBuf {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join(nextdude_common::APP_NAME);
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config").join("nextdude");
        }
        PathBuf::from(".nextdude")
    }

    pub fn config_file_path() -> PathBuf {
        Self::default_data_dir().join("client_config.json")
    }

    pub fn logs_dir() -> PathBuf {
        Self::default_data_dir().join("logs")
    }

    pub fn load_from(path: &Path) -> Result<Self, ClientCoreError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        let cfg: Self = serde_json::from_str(&raw)?;
        Ok(cfg)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ClientCoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}
