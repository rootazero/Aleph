// core/src/gateway/security/device.rs

//! Device types and registry for device authentication.

use serde::{Deserialize, Serialize};

use super::crypto::DeviceFingerprint;
use super::store::DeviceRow;

/// Device type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    MacOS,
    IOS,
    Android,
    CLI,
    Web,
}

impl DeviceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceType::MacOS => "macos",
            DeviceType::IOS => "ios",
            DeviceType::Android => "android",
            DeviceType::CLI => "cli",
            DeviceType::Web => "web",
        }
    }

    /// Parse from string, returning None for unknown types
    pub fn from_str_opt(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl std::str::FromStr for DeviceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "macos" => Ok(DeviceType::MacOS),
            "ios" => Ok(DeviceType::IOS),
            "android" => Ok(DeviceType::Android),
            "cli" => Ok(DeviceType::CLI),
            "web" => Ok(DeviceType::Web),
            _ => Err(format!("Unknown device type: {}", s)),
        }
    }
}

/// Device role - determines permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum DeviceRole {
    /// Full control (CLI, macOS App, Web UI)
    #[default]
    Operator,
    /// Limited execution (iOS/Android nodes)
    Node,
}

impl DeviceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceRole::Operator => "operator",
            DeviceRole::Node => "node",
        }
    }

    /// Parse from string, returning None for unknown roles
    pub fn from_str_opt(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl std::str::FromStr for DeviceRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "operator" => Ok(DeviceRole::Operator),
            "node" => Ok(DeviceRole::Node),
            _ => Err(format!("Unknown device role: {}", s)),
        }
    }
}


/// An approved device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub device_id: String,
    pub device_name: String,
    pub device_type: Option<DeviceType>,
    pub public_key: Vec<u8>,
    pub fingerprint: DeviceFingerprint,
    pub role: DeviceRole,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub approved_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl Device {
    /// Check if device is active (not revoked)
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Check if device has a specific scope
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(&"*".to_string()) || self.scopes.iter().any(|s| s == scope)
    }
}

impl From<DeviceRow> for Device {
    fn from(row: DeviceRow) -> Self {
        Device {
            device_id: row.device_id,
            device_name: row.device_name,
            device_type: row.device_type.and_then(|s| DeviceType::from_str_opt(&s)),
            public_key: row.public_key,
            fingerprint: DeviceFingerprint(row.fingerprint),
            role: DeviceRole::from_str_opt(&row.role).unwrap_or_default(),
            scopes: row.scopes,
            created_at: row.created_at,
            approved_at: row.approved_at,
            last_seen_at: row.last_seen_at,
            revoked_at: row.revoked_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_type_conversion() {
        assert_eq!(DeviceType::MacOS.as_str(), "macos");
        assert_eq!(DeviceType::from_str_opt("macos"), Some(DeviceType::MacOS));
        assert_eq!(DeviceType::from_str_opt("MACOS"), Some(DeviceType::MacOS));
        assert_eq!(DeviceType::from_str_opt("unknown"), None);
    }

    #[test]
    fn test_device_role_conversion() {
        assert_eq!(DeviceRole::Operator.as_str(), "operator");
        assert_eq!(DeviceRole::from_str_opt("operator"), Some(DeviceRole::Operator));
        assert_eq!(DeviceRole::from_str_opt("NODE"), Some(DeviceRole::Node));
    }

    #[test]
    fn test_device_has_scope() {
        let device = Device {
            device_id: "test".into(),
            device_name: "Test".into(),
            device_type: None,
            public_key: vec![],
            fingerprint: DeviceFingerprint("abc".into()),
            role: DeviceRole::Operator,
            scopes: vec!["read".into(), "write".into()],
            created_at: 0,
            approved_at: 0,
            last_seen_at: None,
            revoked_at: None,
        };

        assert!(device.has_scope("read"));
        assert!(device.has_scope("write"));
        assert!(!device.has_scope("admin"));
    }

    #[test]
    fn test_device_wildcard_scope() {
        let device = Device {
            device_id: "test".into(),
            device_name: "Test".into(),
            device_type: None,
            public_key: vec![],
            fingerprint: DeviceFingerprint("abc".into()),
            role: DeviceRole::Operator,
            scopes: vec!["*".into()],
            created_at: 0,
            approved_at: 0,
            last_seen_at: None,
            revoked_at: None,
        };

        assert!(device.has_scope("anything"));
        assert!(device.has_scope("admin"));
    }
}
