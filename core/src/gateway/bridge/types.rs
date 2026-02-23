//! Bridge definition types for the Social Connectivity plugin system.
//!
//! A Bridge is a plugin that connects Aleph to an external messaging platform.
//! Bridge definitions are parsed from `bridge.yaml` manifest files.

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// BridgeId
// ---------------------------------------------------------------------------

/// Unique identifier for a bridge plugin (e.g. "telegram", "discord").
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct BridgeId(pub String);

impl BridgeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BridgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// BridgeDefinition
// ---------------------------------------------------------------------------

/// A bridge manifest parsed from `bridge.yaml`.
///
/// Describes the bridge plugin's metadata, runtime configuration,
/// and declared capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeDefinition {
    /// Manifest schema version (e.g. "1")
    pub spec_version: String,

    /// Unique bridge identifier
    pub id: BridgeId,

    /// Human-readable name
    pub name: String,

    /// Semantic version of this bridge
    pub version: String,

    /// Author name or organisation
    #[serde(default)]
    pub author: String,

    /// Short description of what this bridge connects to
    #[serde(default)]
    pub description: String,

    /// How the bridge is executed
    pub runtime: BridgeRuntime,

    /// Declared capabilities
    #[serde(default)]
    pub capabilities: BridgeCapabilities,

    /// JSON Schema for bridge-level settings (validated at link creation time)
    #[serde(default)]
    pub settings_schema: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// BridgeRuntime
// ---------------------------------------------------------------------------

/// How a bridge is executed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BridgeRuntime {
    /// Bridge is compiled into the Aleph binary.
    Builtin,

    /// Bridge runs as a separate child process.
    Process {
        /// Path to the executable (absolute or relative to bridge dir)
        executable: String,

        /// Command-line arguments
        #[serde(default)]
        args: Vec<String>,

        /// IPC transport between Aleph and the bridge process
        #[serde(default)]
        transport: TransportType,

        /// Interval in seconds between health-check pings
        #[serde(default = "default_health_check_interval")]
        health_check_interval_secs: u64,

        /// Maximum number of automatic restarts before giving up
        #[serde(default = "default_max_restarts")]
        max_restarts: u32,

        /// Delay in seconds before restarting after a crash
        #[serde(default = "default_restart_delay")]
        restart_delay_secs: u64,
    },
}

fn default_health_check_interval() -> u64 {
    30
}

fn default_max_restarts() -> u32 {
    5
}

fn default_restart_delay() -> u64 {
    3
}

// ---------------------------------------------------------------------------
// TransportType
// ---------------------------------------------------------------------------

/// IPC transport used between Aleph and a bridge child process.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TransportType {
    /// Communication over a Unix domain socket.
    #[default]
    UnixSocket,
    /// Communication over stdin/stdout.
    Stdio,
}

// ---------------------------------------------------------------------------
// BridgeCapabilities
// ---------------------------------------------------------------------------

/// Capabilities declared by a bridge plugin.
///
/// Each field is a list of capability strings. The gateway uses these to
/// validate link configurations and to advertise features to the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeCapabilities {
    /// Core messaging capabilities (e.g. "send_text", "send_image")
    #[serde(default)]
    pub messaging: Vec<String>,

    /// Interaction capabilities (e.g. "inline_buttons", "reactions")
    #[serde(default)]
    pub interactions: Vec<String>,

    /// Lifecycle hooks the bridge supports (e.g. "on_start", "on_stop")
    #[serde(default)]
    pub lifecycle: Vec<String>,

    /// Optional / experimental capabilities
    #[serde(default)]
    pub optional: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_id_basics() {
        let id = BridgeId::new("telegram");
        assert_eq!(id.as_str(), "telegram");
        assert_eq!(format!("{}", id), "telegram");
        assert_eq!(id, BridgeId::new("telegram"));
    }

    #[test]
    fn test_deserialize_builtin_bridge() {
        let yaml = r#"
spec_version: "1"
id: telegram
name: Telegram Bridge
version: "0.1.0"
author: Aleph
description: Built-in Telegram integration
runtime:
  type: builtin
capabilities:
  messaging:
    - send_text
    - send_image
    - receive_text
  interactions:
    - inline_buttons
    - reactions
  lifecycle:
    - on_start
    - on_stop
"#;
        let def: BridgeDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.id.as_str(), "telegram");
        assert_eq!(def.name, "Telegram Bridge");
        assert_eq!(def.spec_version, "1");
        assert_eq!(def.version, "0.1.0");
        assert_eq!(def.author, "Aleph");
        assert!(matches!(def.runtime, BridgeRuntime::Builtin));
        assert_eq!(def.capabilities.messaging.len(), 3);
        assert_eq!(def.capabilities.interactions.len(), 2);
        assert_eq!(def.capabilities.lifecycle.len(), 2);
        assert!(def.capabilities.optional.is_empty());
        assert!(def.settings_schema.is_none());
    }

    #[test]
    fn test_deserialize_process_bridge() {
        let yaml = r#"
spec_version: "1"
id: signal
name: Signal Bridge
version: "0.2.0"
runtime:
  type: process
  executable: ./signal-bridge
  args:
    - "--verbose"
    - "--port"
    - "9000"
  transport: unix-socket
  health_check_interval_secs: 15
  max_restarts: 3
  restart_delay_secs: 5
capabilities:
  messaging:
    - send_text
settings_schema:
  type: object
  properties:
    phone_number:
      type: string
"#;
        let def: BridgeDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.id.as_str(), "signal");
        match &def.runtime {
            BridgeRuntime::Process {
                executable,
                args,
                transport,
                health_check_interval_secs,
                max_restarts,
                restart_delay_secs,
            } => {
                assert_eq!(executable, "./signal-bridge");
                assert_eq!(args, &["--verbose", "--port", "9000"]);
                assert_eq!(*transport, TransportType::UnixSocket);
                assert_eq!(*health_check_interval_secs, 15);
                assert_eq!(*max_restarts, 3);
                assert_eq!(*restart_delay_secs, 5);
            }
            _ => panic!("Expected Process runtime"),
        }
        assert!(def.settings_schema.is_some());
    }

    #[test]
    fn test_process_bridge_defaults() {
        let yaml = r#"
spec_version: "1"
id: minimal
name: Minimal Process Bridge
version: "0.1.0"
runtime:
  type: process
  executable: ./my-bridge
"#;
        let def: BridgeDefinition = serde_yaml::from_str(yaml).unwrap();
        match &def.runtime {
            BridgeRuntime::Process {
                args,
                transport,
                health_check_interval_secs,
                max_restarts,
                restart_delay_secs,
                ..
            } => {
                assert!(args.is_empty());
                assert_eq!(*transport, TransportType::UnixSocket);
                assert_eq!(*health_check_interval_secs, 30);
                assert_eq!(*max_restarts, 5);
                assert_eq!(*restart_delay_secs, 3);
            }
            _ => panic!("Expected Process runtime"),
        }
        // author and description should default to empty strings
        assert_eq!(def.author, "");
        assert_eq!(def.description, "");
        assert!(def.capabilities.messaging.is_empty());
    }

    #[test]
    fn test_transport_type_stdio() {
        let yaml = r#"
spec_version: "1"
id: stdio-bridge
name: Stdio Bridge
version: "0.1.0"
runtime:
  type: process
  executable: ./my-bridge
  transport: stdio
"#;
        let def: BridgeDefinition = serde_yaml::from_str(yaml).unwrap();
        match &def.runtime {
            BridgeRuntime::Process { transport, .. } => {
                assert_eq!(*transport, TransportType::Stdio);
            }
            _ => panic!("Expected Process runtime"),
        }
    }
}
