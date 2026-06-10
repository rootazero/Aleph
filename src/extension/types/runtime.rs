//! Runtime interaction types for plugin extensions
//!
//! This module contains types for plugin runtime interactions including:
//! - Background services (lifecycle management)

use serde::{Deserialize, Serialize};

// =============================================================================
// Service Types (V2 Background Services)
// =============================================================================

/// Service state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ServiceState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

/// Running service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub id: String,
    pub plugin_id: String,
    pub name: String,
    pub state: ServiceState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

/// Service lifecycle result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResult {
    pub success: bool,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

impl ServiceResult {
    #[must_use]
    pub fn ok() -> Self {
        Self {
            success: true,
            message: None,
            data: None,
        }
    }

    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(msg.into()),
            data: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(msg.into()),
            data: None,
        }
    }
}
