//! ACP (Agent Communication Protocol) configuration types
//!
//! Contains configuration for ACP harness management:
//! - AcpConfig: Top-level ACP settings (enable/disable, harness registry)
//! - AcpHarnessEntry: Individual harness configuration

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// AcpConfig
// =============================================================================

/// ACP harness management configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpConfig {
    /// Enable/disable ACP functionality
    #[serde(default)]
    pub enabled: bool,

    /// Registered ACP harnesses keyed by name
    #[serde(default)]
    pub harnesses: HashMap<String, AcpHarnessEntry>,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            harnesses: HashMap::new(),
        }
    }
}

// =============================================================================
// AcpHarnessEntry
// =============================================================================

/// Configuration for a single ACP harness
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpHarnessEntry {
    /// Path to the harness executable (optional, resolved from PATH if absent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,

    /// Whether this harness is enabled
    #[serde(default = "super::search::default_true")]
    pub enabled: bool,
}

impl Default for AcpHarnessEntry {
    fn default() -> Self {
        Self {
            executable: None,
            enabled: true,
        }
    }
}
