//! System State Bus - Real-time application state streaming.
//!
//! The State Bus provides continuous, event-driven access to application UI state
//! through the Gateway's EventBus. It bridges macOS Accessibility API events to
//! WebSocket clients via JSON-RPC 2.0 notifications.
//!
//! # Architecture
//!
//! ```text
//! AX API → AxObserver → SystemStateBus → GatewayEventBus → WebSocket Clients
//! ```
//!
//! # Key Features
//!
//! - Event-driven (no polling): O(1) complexity
//! - JSON Patch incremental updates (RFC 6902)
//! - Topic-based subscriptions (e.g., "system.state.com.apple.mail.*")
//! - State caching for real-time coordinate mapping
//! - Privacy filtering middleware
//!
//! # Example
//!
//! ```ignore
//! // Subscribe to Mail.app state changes
//! let bus = SystemStateBus::new(gateway.event_bus.clone());
//! bus.start().await?;
//!
//! // Client subscribes via RPC
//! gateway.rpc_call("system.state.subscribe", json!({
//!     "patterns": ["system.state.com.apple.mail.*"]
//! })).await?;
//! ```

mod types;
mod state_cache;
mod element_id;
mod state_history;
mod privacy_filter;

#[cfg(target_os = "macos")]
mod ax_observer;

pub use types::*;
pub use state_cache::StateCache;
pub use element_id::{StableElementId, ElementInfo};
pub use state_history::{StateHistory, JsonPatch};
pub use privacy_filter::{PrivacyFilter, PrivacyFilterConfig};

#[cfg(target_os = "macos")]
pub use ax_observer::AxObserver;

use crate::error::Result;
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::perception::connectors::ConnectorRegistry;
use crate::perception::pal::SystemSensor;
use std::sync::Arc;
use tokio::sync::RwLock;

/// System State Bus - manages application state streaming.
pub struct SystemStateBus {
    /// Gateway event bus for publishing state events
    event_bus: GatewayEventBus,

    /// Current state cache (app_id -> AppState)
    state_cache: Arc<RwLock<StateCache>>,

    /// State history (I-Frame + P-Frame)
    state_history: Arc<RwLock<StateHistory>>,

    /// Privacy filter middleware
    privacy_filter: Arc<PrivacyFilter>,

    /// Connector registry for multi-source state capture
    connector_registry: Arc<ConnectorRegistry>,

    /// Platform sensor (optional, for cross-platform perception)
    sensor: Option<Arc<dyn SystemSensor>>,

    /// AX observer (macOS only, legacy)
    #[cfg(target_os = "macos")]
    ax_observer: Option<Arc<AxObserver>>,
}

impl SystemStateBus {
    /// Create a new System State Bus.
    pub fn new(event_bus: GatewayEventBus) -> Self {
        Self {
            event_bus,
            state_cache: Arc::new(RwLock::new(StateCache::new())),
            state_history: Arc::new(RwLock::new(StateHistory::new(30))),
            privacy_filter: Arc::new(PrivacyFilter::default()),
            connector_registry: Arc::new(ConnectorRegistry::new()),
            sensor: None,
            #[cfg(target_os = "macos")]
            ax_observer: None,
        }
    }

    /// Create with a platform sensor for cross-platform perception.
    pub fn with_sensor(event_bus: GatewayEventBus, sensor: Arc<dyn SystemSensor>) -> Self {
        Self {
            event_bus,
            state_cache: Arc::new(RwLock::new(StateCache::new())),
            state_history: Arc::new(RwLock::new(StateHistory::new(30))),
            privacy_filter: Arc::new(PrivacyFilter::default()),
            connector_registry: Arc::new(ConnectorRegistry::new()),
            sensor: Some(sensor),
            #[cfg(target_os = "macos")]
            ax_observer: None,
        }
    }

    /// Create with platform-specific sensor (auto-detected).
    pub fn new_with_platform_sensor(event_bus: GatewayEventBus) -> Result<Self> {
        use crate::perception::pal::sensors::create_platform_sensor;
        let sensor = create_platform_sensor()?;
        Ok(Self::with_sensor(event_bus, sensor))
    }

    /// Create with custom privacy filter configuration.
    pub fn with_privacy_config(event_bus: GatewayEventBus, privacy_config: PrivacyFilterConfig) -> Self {
        Self {
            event_bus,
            state_cache: Arc::new(RwLock::new(StateCache::new())),
            state_history: Arc::new(RwLock::new(StateHistory::new(30))),
            privacy_filter: Arc::new(PrivacyFilter::new(privacy_config)),
            connector_registry: Arc::new(ConnectorRegistry::new()),
            sensor: None,
            #[cfg(target_os = "macos")]
            ax_observer: None,
        }
    }

    /// Start the state bus (begins listening for AX events).
    #[cfg(target_os = "macos")]
    pub async fn start(&mut self) -> Result<()> {
        use tracing::info;

        info!("Starting System State Bus");

        // Start AX observer
        let (observer, mut rx) = AxObserver::start()?;
        self.ax_observer = Some(Arc::new(observer));

        // Spawn task to consume AX events
        let bus = self.clone_for_task();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Err(e) = bus.handle_ax_event(event).await {
                    tracing::warn!("Failed to handle AX event: {}", e);
                }
            }
        });

        info!("System State Bus started");
        Ok(())
    }

    /// Start the state bus (stub for non-macOS).
    #[cfg(not(target_os = "macos"))]
    pub async fn start(&mut self) -> Result<()> {
        tracing::warn!("System State Bus not supported on this platform");
        Ok(())
    }

    /// Handle an AX event from the observer.
    #[cfg(target_os = "macos")]
    async fn handle_ax_event(&self, event: AxEvent) -> Result<()> {
        use serde_json::json;

        // Convert AX event to state delta
        let (app_id, patch) = match event {
            AxEvent::ValueChanged { app_id, element_id, new_value } => {
                let patch = json!([{
                    "op": "replace",
                    "path": format!("/elements/{}/current_value", element_id),
                    "value": new_value
                }]);
                (app_id, patch)
            }
            AxEvent::FocusChanged { app_id, from, to } => {
                let patch = json!([
                    {
                        "op": "replace",
                        "path": format!("/elements/{}/state/focused", from),
                        "value": false
                    },
                    {
                        "op": "replace",
                        "path": format!("/elements/{}/state/focused", to),
                        "value": true
                    }
                ]);
                (app_id, patch)
            }
            _ => return Ok(()),
        };

        // Store P-Frame in history
        let patches = vec![JsonPatch {
            op: "replace".to_string(),
            path: "/elements/0/value".to_string(),
            value: Some(patch.clone()),
        }];
        self.state_history.write().await.store_pframe(patches);

        // Check if we should store I-Frame
        if self.state_history.read().await.should_store_iframe() {
            // TODO: Get full state from cache and store as I-Frame
        }

        // Publish to event bus
        let topic = format!("system.state.{}.delta", app_id);
        let event = TopicEvent::new(topic, patch);
        self.event_bus.publish_json(&event)?;

        Ok(())
    }

    /// Clone for use in async task.
    fn clone_for_task(&self) -> Self {
        Self {
            event_bus: self.event_bus.clone(),
            state_cache: self.state_cache.clone(),
            state_history: self.state_history.clone(),
            privacy_filter: self.privacy_filter.clone(),
            connector_registry: self.connector_registry.clone(),
            sensor: self.sensor.clone(),
            #[cfg(target_os = "macos")]
            ax_observer: self.ax_observer.clone(),
        }
    }

    /// Get read access to state cache.
    pub fn state_cache(&self) -> Arc<RwLock<StateCache>> {
        self.state_cache.clone()
    }

    /// Get read access to state history.
    pub fn state_history(&self) -> Arc<RwLock<StateHistory>> {
        self.state_history.clone()
    }

    /// Get privacy filter.
    pub fn privacy_filter(&self) -> Arc<PrivacyFilter> {
        self.privacy_filter.clone()
    }

    /// Get connector registry.
    pub fn connector_registry(&self) -> Arc<ConnectorRegistry> {
        self.connector_registry.clone()
    }

    /// Capture state using the best available connector.
    ///
    /// This method uses the ConnectorRegistry to automatically select
    /// the best connector for the given application (AX > Plugin > Vision).
    pub async fn capture_state_with_connector(
        &self,
        bundle_id: &str,
        window_id: &str,
    ) -> Result<AppState> {
        // Capture state using connector
        let mut state = self
            .connector_registry
            .capture_state(bundle_id, window_id)
            .await?;

        // Apply privacy filter
        self.privacy_filter.filter(&mut state);

        // Update cache
        self.state_cache.write().await.update(state.clone());

        // Store in history
        if self.state_history.read().await.should_store_iframe() {
            self.state_history
                .write()
                .await
                .store_iframe(state.clone());
        }

        Ok(state)
    }

    /// Start monitoring an application using connectors.
    pub async fn start_monitoring(&self, bundle_id: &str) -> Result<()> {
        self.connector_registry.start_monitoring(bundle_id).await
    }

    /// Stop monitoring an application.
    pub async fn stop_monitoring(&self, bundle_id: &str) -> Result<()> {
        self.connector_registry.stop_monitoring(bundle_id).await
    }

    /// Sense UI using platform sensor (PAL).
    ///
    /// This method uses the tiered perception strategy:
    /// 1. Try structured API (Accessibility) if available
    /// 2. Fallback to screenshot + OCR (not yet implemented)
    /// 3. Fallback to cloud vision (not yet implemented)
    ///
    /// Returns error if no sensor is configured or available.
    pub async fn sense_ui(&self, app_id: &str) -> Result<AppState> {
        use crate::AlephError;

        let sensor = self.sensor.as_ref().ok_or_else(|| AlephError::Other {
            message: "No sensor configured".to_string(),
            suggestion: Some("Use SystemStateBus::with_sensor() to configure a sensor".to_string()),
        })?;

        // Check if sensor is available
        if !sensor.is_available() {
            return Err(AlephError::PermissionDenied {
                message: "Sensor not available (missing permissions or headless environment)"
                    .to_string(),
                suggestion: Some("Check permissions with PerceptionHealth::check()".to_string()),
            });
        }

        // Try structured API first
        let caps = sensor.capabilities();
        if caps.has_structured_api {
            match sensor.capture_ui_tree(app_id).await {
                Ok(ui_tree) => {
                    // Convert UINodeTree to AppState
                    let state = self.convert_ui_tree_to_app_state(ui_tree);
                    return Ok(state);
                }
                Err(e) => {
                    tracing::warn!("Structured API failed: {}, will try fallback", e);
                }
            }
        }

        // TODO: Fallback to screenshot + OCR (Phase 6 Task 3)
        // TODO: Fallback to cloud vision (Phase 6 Task 3)

        Err(AlephError::Other {
            message: "All perception methods failed or unavailable".to_string(),
            suggestion: Some("Enable accessibility permissions or use a supported platform".to_string()),
        })
    }

    /// Convert UINodeTree (PAL) to AppState (SSB).
    ///
    /// This is a temporary conversion until we fully integrate PAL types.
    fn convert_ui_tree_to_app_state(
        &self,
        ui_tree: crate::perception::pal::UINodeTree,
    ) -> AppState {
        use crate::perception::state_bus::types::{AppState, Element, ElementSource, StateSource};

        // Convert PAL UINode to SSB Element
        fn convert_node(node: crate::perception::pal::UINode) -> Element {
            Element {
                id: node.id,
                role: node.role,
                label: node.label,
                current_value: node.value,
                rect: Some(crate::perception::state_bus::types::Rect {
                    x: node.rect.x as f64,
                    y: node.rect.y as f64,
                    width: node.rect.width as f64,
                    height: node.rect.height as f64,
                }),
                state: crate::perception::state_bus::types::ElementState {
                    focused: node.state.focused,
                    enabled: node.state.enabled,
                    selected: false, // PAL doesn't track selection state yet
                },
                source: ElementSource::Ax,
                confidence: 1.0, // Structured API has high confidence
            }
        }

        AppState {
            app_id: ui_tree.app_id,
            elements: vec![convert_node(ui_tree.root)],
            app_context: None,
            source: StateSource::Accessibility,
            confidence: 1.0, // Structured API has high confidence
        }
    }
}

