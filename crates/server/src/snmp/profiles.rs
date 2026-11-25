use crate::error::ServerError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSensor {
    pub name: String,
    pub category: String, // "cpu", "memory", "temperature", "voltage", "fan", "psu", "other"
    pub oid: String,
    #[serde(default = "default_one")]
    pub scale: f64,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub is_table: bool,
    #[serde(default)]
    pub warn_threshold: Option<f64>,
    #[serde(default)]
    pub crit_threshold: Option<f64>,
}

fn default_one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelImageRule {
    pub descr_or_model_pattern: String,
    pub model_name: String,
    pub image_key: String,
    #[serde(default)]
    pub device_type_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorProfile {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub sys_object_id_prefixes: Vec<String>,
    #[serde(default)]
    pub default_vendor_image_key: String,
    #[serde(default)]
    pub sensors: Vec<ProfileSensor>,
    #[serde(default)]
    pub model_rules: Vec<ModelImageRule>,
}

#[derive(Debug, Clone, Default)]
pub struct VendorProfileRegistry {
    profiles: Vec<VendorProfile>,
}

impl VendorProfileRegistry {
    pub fn load(system_dir: &Path, override_dir: &Path) -> Result<Self, ServerError> {
        let mut by_id: BTreeMap<String, VendorProfile> = BTreeMap::new();
        load_dir(system_dir, &mut by_id)?;
        load_dir(override_dir, &mut by_id)?;
        Ok(Self {
            profiles: by_id.into_values().collect(),
        })
    }

    pub fn from_profiles(profiles: Vec<VendorProfile>) -> Self {
        Self { profiles }
    }

    pub fn all(&self) -> &[VendorProfile] {
        &self.profiles
    }

    /// Finds the profile whose `sysObjectID` prefix has the longest match.
    pub fn match_sys_object_id(&self, sys_object_id: &str) -> Option<&VendorProfile> {
        let clean = sys_object_id.trim().trim_start_matches('.');
        let mut best: Option<(usize, &VendorProfile)> = None;
        for profile in &self.profiles {
            for prefix in &profile.sys_object_id_prefixes {
                let p = prefix.trim().trim_start_matches('.');
                let matches = if p.is_empty() {
                    true
                } else {
                    clean == p || clean.starts_with(&format!("{p}."))
                };
                if matches {
                    let score = p.len();
                    if best.map_or(true, |(prev_len, _)| score > prev_len) {
                        best = Some((score, profile));
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }

    pub fn match_model_and_image<'a>(
        &self,
        profile: &'a VendorProfile,
        sys_descr: &str,
    ) -> (String, Option<&'a str>, Option<&'a str>) {
        let lower = sys_descr.to_ascii_lowercase();
        for rule in &profile.model_rules {
            if lower.contains(&rule.descr_or_model_pattern.to_ascii_lowercase()) {
                let img = if rule.image_key.is_empty() {
                    None
                } else {
                    Some(rule.image_key.as_str())
                };
                let dtype = if rule.device_type_name.is_empty() {
                    None
                } else {
                    Some(rule.device_type_name.as_str())
                };
                return (rule.model_name.clone(), img, dtype);
            }
        }
        let fallback_img = if profile.default_vendor_image_key.is_empty() {
            None
        } else {
            Some(profile.default_vendor_image_key.as_str())
        };
        (String::new(), fallback_img, None)
    }
}

fn load_dir(dir: &Path, out: &mut BTreeMap<String, VendorProfile>) -> Result<(), ServerError> {
    if !dir.exists() {
        return Ok(());
    }
    let mut entries = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        let raw = fs::read_to_string(&path).map_err(|e| {
            ServerError::Config(format!("failed reading profile {}: {e}", path.display()))
        })?;
        let profile: VendorProfile = toml::from_str(&raw).map_err(|e| {
            ServerError::Config(format!("invalid vendor profile {}: {e}", path.display()))
        })?;
        out.insert(profile.id.clone(), profile);
    }
    Ok(())
}
