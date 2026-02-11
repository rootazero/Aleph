//! Action dispatcher with closed-loop validation.
//!
//! Coordinates simulation actions with state verification:
//! 1. Pre-action validation (element exists, enabled, etc.)
//! 2. Execute simulation (click, type, scroll)
//! 3. Post-action validation (verify expected state change)
//!
//! # Example
//!
//! ```ignore
//! let dispatcher = ActionDispatcher::new(state_bus, executor);
//!
//! let request = ActionRequest {
//!     target_id: "btn_send_001".to_string(),
//!     method: ActionMethod::Click,
//!     expect: ExpectCondition {
//!         condition: ConditionType::ElementDisappear,
//!         timeout_ms: 500,
//!     },
//! };
//!
//! let result = dispatcher.execute(request).await?;
//! ```

use crate::error::{AlephError, Result};
use crate::perception::simulation_executor::SimulationExecutor;
use crate::perception::state_bus::{StateCache, SystemStateBus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Action dispatcher with closed-loop validation.
pub struct ActionDispatcher {
    /// State bus for coordinate lookup
    state_bus: Arc<SystemStateBus>,

    /// Simulation executor
    executor: Arc<SimulationExecutor>,
}

/// Action request.
#[derive(Debug, Clone, Deserialize)]
pub struct ActionRequest {
    /// Target element ID
    pub target_id: String,

    /// Action method
    pub method: ActionMethod,

    /// Expected outcome
    pub expect: ExpectCondition,

    /// Visual fallback (optional)
    #[serde(default)]
    pub fallback: Option<VisualFallback>,
}

/// Action method.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionMethod {
    /// Click element
    Click,

    /// Type text
    Type { text: String },

    /// Scroll
    Scroll { delta: i32 },
}

/// Expected condition after action.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectCondition {
    /// Condition type
    pub condition: ConditionType,

    /// Timeout (milliseconds)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    500
}

/// Condition type.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConditionType {
    /// Element disappears
    ElementDisappear,

    /// Value changes to expected
    ValueChanged { expected: String },

    /// State property changes
    StateChanged { key: String, value: Value },
}

/// Visual fallback when ID fails.
#[derive(Debug, Clone, Deserialize)]
pub struct VisualFallback {
    /// Visual anchor (image path or description)
    pub visual_anchor: String,

    /// Offset from anchor (x, y)
    #[serde(default)]
    pub offset: (f64, f64),
}

/// Action result.
#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    /// Success flag
    pub success: bool,

    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Used fallback
    #[serde(default)]
    pub used_fallback: bool,
}

impl ActionDispatcher {
    /// Create a new action dispatcher.
    pub fn new(state_bus: Arc<SystemStateBus>, executor: Arc<SimulationExecutor>) -> Self {
        Self {
            state_bus,
            executor,
        }
    }

    /// Execute an action with validation.
    pub async fn execute(&self, request: ActionRequest) -> Result<ActionResult> {
        debug!("Executing action: {:?}", request.method);

        // Step 1: Get element from state cache
        let state_cache = self.state_bus.state_cache();
        let cache_read = state_cache.read().await;

        let element = cache_read.get_element(&request.target_id);

        if element.is_none() {
            // Try visual fallback if available
            if let Some(ref fallback) = request.fallback {
                warn!("Element {} not found, trying visual fallback", request.target_id);
                return self.execute_with_fallback(&request, fallback).await;
            }

            return Err(AlephError::tool(format!(
                "Element {} not found in state cache",
                request.target_id
            )));
        }

        let element = element.unwrap();

        // Step 2: Pre-action validation
        if !element.state.enabled {
            return Ok(ActionResult {
                success: false,
                error: Some("Element is disabled".to_string()),
                used_fallback: false,
            });
        }

        let rect = element.rect.ok_or_else(|| {
            AlephError::tool(format!("Element {} has no bounding rect", request.target_id))
        })?;

        drop(cache_read); // Release lock before async operations

        // Step 3: Execute simulation
        match request.method {
            ActionMethod::Click => {
                let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
                self.executor.click(center).await?;
            }
            ActionMethod::Type { ref text } => {
                self.executor.focus(rect).await?;
                self.executor.type_text(text).await?;
            }
            ActionMethod::Scroll { delta } => {
                self.executor.scroll(rect, delta).await?;
            }
        }

        // Step 4: Wait for state change
        tokio::time::sleep(Duration::from_millis(request.expect.timeout_ms)).await;

        // Step 5: Post-action validation
        let validation_result = self.validate_condition(&request).await?;

        if !validation_result {
            return Ok(ActionResult {
                success: false,
                error: Some("Expected condition not met".to_string()),
                used_fallback: false,
            });
        }

        Ok(ActionResult {
            success: true,
            error: None,
            used_fallback: false,
        })
    }

    /// Execute with visual fallback.
    async fn execute_with_fallback(
        &self,
        request: &ActionRequest,
        fallback: &VisualFallback,
    ) -> Result<ActionResult> {
        // TODO: Implement visual anchor matching
        // For now, return error
        warn!("Visual fallback not yet implemented");

        Ok(ActionResult {
            success: false,
            error: Some("Visual fallback not yet implemented".to_string()),
            used_fallback: true,
        })
    }

    /// Validate expected condition.
    async fn validate_condition(&self, request: &ActionRequest) -> Result<bool> {
        let state_cache = self.state_bus.state_cache();
        let cache_read = state_cache.read().await;

        match &request.expect.condition {
            ConditionType::ElementDisappear => {
                // Element should not exist in cache
                Ok(cache_read.get_element(&request.target_id).is_none())
            }
            ConditionType::ValueChanged { expected } => {
                // Element value should match expected
                if let Some(element) = cache_read.get_element(&request.target_id) {
                    Ok(element.current_value.as_ref() == Some(expected))
                } else {
                    Ok(false)
                }
            }
            ConditionType::StateChanged { key, value } => {
                // Element state property should match
                if let Some(element) = cache_read.get_element(&request.target_id) {
                    // TODO: Implement state property lookup
                    // For now, just check focused state
                    if key == "focused" {
                        let expected_focused = value.as_bool().unwrap_or(false);
                        Ok(element.state.focused == expected_focused)
                    } else {
                        Ok(false)
                    }
                } else {
                    Ok(false)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::perception::state_bus::types::{
        AppState, Element, ElementSource, ElementState, StateSource,
    };

    fn create_test_state_bus() -> Arc<SystemStateBus> {
        let event_bus = GatewayEventBus::new();
        Arc::new(SystemStateBus::new(event_bus))
    }

    async fn populate_cache(state_bus: &SystemStateBus) {
        let mut cache = state_bus.state_cache().write().await;

        let state = AppState {
            app_id: "com.test.app".to_string(),
            elements: vec![Element {
                id: "btn_test".to_string(),
                role: "button".to_string(),
                label: Some("Test Button".to_string()),
                current_value: None,
                rect: Some(crate::perception::state_bus::types::Rect {
                    x: 100.0,
                    y: 200.0,
                    width: 50.0,
                    height: 20.0,
                }),
                state: ElementState {
                    focused: false,
                    enabled: true,
                    selected: false,
                },
                source: ElementSource::Ax,
                confidence: 1.0,
            }],
            app_context: None,
            source: StateSource::Accessibility,
            confidence: 1.0,
        };

        cache.update(state);
    }

    #[tokio::test]
    async fn test_execute_click() {
        let state_bus = create_test_state_bus();
        populate_cache(&state_bus).await;

        let executor = Arc::new(SimulationExecutor::dry_run());
        let dispatcher = ActionDispatcher::new(state_bus, executor);

        let request = ActionRequest {
            target_id: "btn_test".to_string(),
            method: ActionMethod::Click,
            expect: ExpectCondition {
                condition: ConditionType::ElementDisappear,
                timeout_ms: 100,
            },
            fallback: None,
        };

        let result = dispatcher.execute(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_element_not_found() {
        let state_bus = create_test_state_bus();
        let executor = Arc::new(SimulationExecutor::dry_run());
        let dispatcher = ActionDispatcher::new(state_bus, executor);

        let request = ActionRequest {
            target_id: "nonexistent".to_string(),
            method: ActionMethod::Click,
            expect: ExpectCondition {
                condition: ConditionType::ElementDisappear,
                timeout_ms: 100,
            },
            fallback: None,
        };

        let result = dispatcher.execute(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_element_disabled() {
        let state_bus = create_test_state_bus();

        // Add disabled element
        {
            let mut cache = state_bus.state_cache().write().await;
            let state = AppState {
                app_id: "com.test.app".to_string(),
                elements: vec![Element {
                    id: "btn_disabled".to_string(),
                    role: "button".to_string(),
                    label: Some("Disabled Button".to_string()),
                    current_value: None,
                    rect: Some(crate::perception::state_bus::types::Rect {
                        x: 100.0,
                        y: 200.0,
                        width: 50.0,
                        height: 20.0,
                    }),
                    state: ElementState {
                        focused: false,
                        enabled: false, // Disabled
                        selected: false,
                    },
                    source: ElementSource::Ax,
                    confidence: 1.0,
                }],
                app_context: None,
                source: StateSource::Accessibility,
                confidence: 1.0,
            };
            cache.update(state);
        }

        let executor = Arc::new(SimulationExecutor::dry_run());
        let dispatcher = ActionDispatcher::new(state_bus, executor);

        let request = ActionRequest {
            target_id: "btn_disabled".to_string(),
            method: ActionMethod::Click,
            expect: ExpectCondition {
                condition: ConditionType::ElementDisappear,
                timeout_ms: 100,
            },
            fallback: None,
        };

        let result = dispatcher.execute(request).await.unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }
}
