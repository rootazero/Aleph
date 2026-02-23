// Connector system for multi-source state capture
//
// Architecture:
// - StateConnector trait: Interface for all connectors
// - ConnectorRegistry: Auto-selects best connector for each app
// - VisionConnector: OCR-based fallback for apps without AX API

mod registry;
mod vision;

pub use registry::{ConnectorRegistry, ConnectorType};
pub use vision::VisionConnector;

use crate::perception::state_bus::AppState;
use crate::error::Result;
use async_trait::async_trait;

/// Trait for state capture connectors
#[async_trait]
pub trait StateConnector: Send + Sync {
    /// Connector type identifier
    fn connector_type(&self) -> ConnectorType;

    /// Check if this connector can handle the given app
    async fn can_handle(&self, bundle_id: &str) -> bool;

    /// Capture current state of the application
    async fn capture_state(&self, bundle_id: &str, window_id: &str) -> Result<AppState>;

    /// Start continuous monitoring (optional)
    async fn start_monitoring(&self, _bundle_id: &str) -> Result<()> {
        Ok(())
    }

    /// Stop monitoring
    async fn stop_monitoring(&self, _bundle_id: &str) -> Result<()> {
        Ok(())
    }
}
