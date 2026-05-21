use super::{StateConnector, VisionConnector};
use crate::perception::state_bus::AppState;
use crate::error::Result;
use crate::AlephError;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Connector type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectorType {
    /// macOS Accessibility API connector
    Accessibility,
    /// Browser plugin connector (Chrome/Firefox extension)
    Plugin,
    /// Vision-based connector (OCR + CV)
    Vision,
}

/// Registry for managing state connectors
///
/// Auto-selects the best connector for each application:
/// 1. Try Accessibility API first (fastest, most reliable)
/// 2. Fall back to Plugin connector if available
/// 3. Fall back to Vision connector (slowest, universal)
pub struct ConnectorRegistry {
    connectors: HashMap<ConnectorType, Arc<dyn StateConnector>>,
    active_monitors: Arc<RwLock<HashMap<String, ConnectorType>>>,
}

impl ConnectorRegistry {
    /// Create a new connector registry
    pub fn new() -> Self {
        let mut connectors: HashMap<ConnectorType, Arc<dyn StateConnector>> = HashMap::new();

        // Register Vision connector (always available as fallback)
        connectors.insert(
            ConnectorType::Vision,
            Arc::new(VisionConnector::new()),
        );

        // TODO: Register Accessibility connector when implemented
        // TODO: Register Plugin connector when implemented

        Self {
            connectors,
            active_monitors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Select the best connector for the given app
    ///
    /// Priority: Accessibility > Plugin > Vision
    pub async fn select_connector(&self, bundle_id: &str) -> Result<Arc<dyn StateConnector>> {
        // Try connectors in priority order
        let priority = [
            ConnectorType::Accessibility,
            ConnectorType::Plugin,
            ConnectorType::Vision,
        ];

        for connector_type in priority {
            if let Some(connector) = self.connectors.get(&connector_type) {
                if connector.can_handle(bundle_id).await {
                    return Ok(Arc::clone(connector));
                }
            }
        }

        // Should never happen since Vision connector handles everything
        Err(AlephError::tool(format!(
            "No connector available for bundle_id: {}",
            bundle_id
        )))
    }

    /// Capture state using the best available connector
    pub async fn capture_state(
        &self,
        bundle_id: &str,
        window_id: &str,
    ) -> Result<AppState> {
        let connector = self.select_connector(bundle_id).await?;
        connector.capture_state(bundle_id, window_id).await
    }

    /// Start monitoring an application
    pub async fn start_monitoring(&self, bundle_id: &str) -> Result<()> {
        let connector = self.select_connector(bundle_id).await?;
        let connector_type = connector.connector_type();

        connector.start_monitoring(bundle_id).await?;

        // Track active monitor
        self.active_monitors
            .write()
            .await
            .insert(bundle_id.to_string(), connector_type);

        Ok(())
    }

    /// Stop monitoring an application
    pub async fn stop_monitoring(&self, bundle_id: &str) -> Result<()> {
        let mut monitors = self.active_monitors.write().await;

        if let Some(connector_type) = monitors.remove(bundle_id) {
            if let Some(connector) = self.connectors.get(&connector_type) {
                connector.stop_monitoring(bundle_id).await?;
            }
        }

        Ok(())
    }

    /// Get active monitoring sessions
    pub async fn active_monitors(&self) -> Vec<(String, ConnectorType)> {
        self.active_monitors
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = ConnectorRegistry::new();
        assert!(registry.connectors.contains_key(&ConnectorType::Vision));
    }

    #[tokio::test]
    async fn test_connector_selection() {
        let registry = ConnectorRegistry::new();

        // Vision connector should always be available as fallback
        let connector = registry.select_connector("com.example.app").await;
        assert!(connector.is_ok());
        assert_eq!(connector.unwrap().connector_type(), ConnectorType::Vision);
    }

    #[tokio::test]
    async fn test_monitoring_lifecycle() {
        let registry = ConnectorRegistry::new();
        let bundle_id = "com.example.test";

        // Start monitoring
        registry.start_monitoring(bundle_id).await.unwrap();

        // Check active monitors
        let monitors = registry.active_monitors().await;
        assert_eq!(monitors.len(), 1);
        assert_eq!(monitors[0].0, bundle_id);

        // Stop monitoring
        registry.stop_monitoring(bundle_id).await.unwrap();

        // Check monitors cleared
        let monitors = registry.active_monitors().await;
        assert_eq!(monitors.len(), 0);
    }
}
