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

    /// AX observer (macOS only)
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
            #[cfg(target_os = "macos")]
            ax_observer: None,
        }
    }

    /// Create with custom privacy filter configuration.
    pub fn with_privacy_config(event_bus: GatewayEventBus, privacy_config: PrivacyFilterConfig) -> Self {
        Self {
            event_bus,
            state_cache: Arc::new(RwLock::new(StateCache::new())),
            state_history: Arc::new(RwLock::new(StateHistory::new(30))),
            privacy_filter: Arc::new(PrivacyFilter::new(privacy_config)),
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
}
