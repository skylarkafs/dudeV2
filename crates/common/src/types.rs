use crate::error::CommonError;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MapId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProbeId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinkId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinkTypeId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdminId(pub i64);

/// Wrapper that prevents accidental secret exposure in logs or Debug output.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RedactSecret(String);

impl RedactSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for RedactSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for RedactSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Granular RBAC permissions enforced server-side on every RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    ReadMonitoring,
    RunTools,
    AcknowledgeAlerts,
    EditMapsAndDevices,
    ManageDiscovery,
    ManageImages,
    ManageNotificationsAndAlerts,
    ManageAdmins,
    ManageServerSettings,
}

impl Permission {
    pub const ALL: [Permission; 9] = [
        Permission::ReadMonitoring,
        Permission::RunTools,
        Permission::AcknowledgeAlerts,
        Permission::EditMapsAndDevices,
        Permission::ManageDiscovery,
        Permission::ManageImages,
        Permission::ManageNotificationsAndAlerts,
        Permission::ManageAdmins,
        Permission::ManageServerSettings,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadMonitoring => "read_monitoring",
            Self::RunTools => "run_tools",
            Self::AcknowledgeAlerts => "acknowledge_alerts",
            Self::EditMapsAndDevices => "edit_maps_and_devices",
            Self::ManageDiscovery => "manage_discovery",
            Self::ManageImages => "manage_images",
            Self::ManageNotificationsAndAlerts => "manage_notifications_and_alerts",
            Self::ManageAdmins => "manage_admins",
            Self::ManageServerSettings => "manage_server_settings",
        }
    }

    pub fn parse(s: &str) -> Result<Self, CommonError> {
        match s {
            "read_monitoring" => Ok(Self::ReadMonitoring),
            "run_tools" => Ok(Self::RunTools),
            "acknowledge_alerts" => Ok(Self::AcknowledgeAlerts),
            "edit_maps_and_devices" => Ok(Self::EditMapsAndDevices),
            "manage_discovery" => Ok(Self::ManageDiscovery),
            "manage_images" => Ok(Self::ManageImages),
            "manage_notifications_and_alerts" => Ok(Self::ManageNotificationsAndAlerts),
            "manage_admins" => Ok(Self::ManageAdmins),
            "manage_server_settings" => Ok(Self::ManageServerSettings),
            other => Err(CommonError::UnknownPermission(other.to_string())),
        }
    }

    pub fn viewer_defaults() -> Vec<Self> {
        vec![Self::ReadMonitoring]
    }

    pub fn operator_defaults() -> Vec<Self> {
        vec![
            Self::ReadMonitoring,
            Self::RunTools,
            Self::AcknowledgeAlerts,
            Self::EditMapsAndDevices,
            Self::ManageDiscovery,
            Self::ManageImages,
        ]
    }

    pub fn admin_defaults() -> Vec<Self> {
        Self::ALL.to_vec()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IcmpCapabilityMode {
    Unavailable = 0,
    Datagram = 1,
    Raw = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageSizeVariant {
    Thumb,
    Px64,
    Px128,
    Px256,
    Original,
}

impl ImageSizeVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thumb => "thumb",
            Self::Px64 => "64",
            Self::Px128 => "128",
            Self::Px256 => "256",
            Self::Original => "original",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "thumb" => Self::Thumb,
            "64" => Self::Px64,
            "128" => Self::Px128,
            "256" => Self::Px256,
            _ => Self::Original,
        }
    }

    pub fn target_pixels(self) -> Option<u32> {
        match self {
            Self::Thumb => Some(32),
            Self::Px64 => Some(64),
            Self::Px128 => Some(128),
            Self::Px256 => Some(256),
            Self::Original => None,
        }
    }
}
