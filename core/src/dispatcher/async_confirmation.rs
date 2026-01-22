//! Async Confirmation Flow for Dispatcher Layer
//!
//! This module implements non-blocking confirmation flow for tool execution:
//!
//! - Returns `PendingConfirmation` state instead of blocking
//! - Swift calls back with `UserConfirmationDecision`
//! - Pending confirmations stored by ID for resumption
//!
//! # Architecture
//!
//! ```text
//! L3 Routing Result (confidence < threshold)
//!       ↓
//! ┌───────────────────────────────────────┐
//! │      Async Confirmation Flow          │
//! │                                       │
//! │  should_confirm(confidence)           │
//! │           ↓                           │
//! │  create_pending(tool, params)         │
//! │           ↓                           │
//! │  Return ConfirmationState::Pending    │
//! │           ↓                           │
//! │  [Non-blocking - Swift shows UI]      │
//! │           ↓                           │
//! │  Swift calls confirm_action(id, dec)  │
//! │           ↓                           │
//! │  resume_with_decision(id, decision)   │
//! └───────────────────────────────────────┘
//!       ↓
//! Execute Tool / Fallback to Chat / Error
//! ```
//!
//! # Migration from Blocking Flow
//!
//! Old (blocking):
//! ```rust,ignore
//! let result = event_handler.on_clarification_needed(request);
//! let action = confirmation.handle_result(result);
//! ```
//!
//! New (async):
//! ```rust,ignore
//! let pending = confirmation.create_pending(tool, params, reason);
//! store.insert(pending.clone());
//! event_handler.on_confirmation_needed(pending.to_ffi());
//! // Returns immediately, Swift will call confirm_action() later
//! ```

use crate::dispatcher::{RoutingLayer, UnifiedTool};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

// =============================================================================
// Pending Confirmation
// =============================================================================

/// A pending confirmation awaiting user decision
///
/// This represents a confirmation request that has been sent to the UI
/// but not yet resolved. It contains all information needed to resume
/// processing once the user makes a decision.
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    /// Unique confirmation ID (used for resumption)
    pub id: String,

    /// The tool that requires confirmation
    pub tool: UnifiedTool,

    /// Parameters for tool execution (JSON serialized)
    pub parameters: serde_json::Value,

    /// Confidence score that triggered confirmation
    pub confidence: f32,

    /// Reason for tool selection (from L3 routing)
    pub reason: String,

    /// Routing layer that produced this result
    pub routing_layer: RoutingLayer,

    /// Timestamp when confirmation was created
    pub created_at: Instant,

    /// Timeout duration for this confirmation
    pub timeout: Duration,
}

impl PendingConfirmation {
    /// Create a new pending confirmation
    pub fn new(
        tool: UnifiedTool,
        parameters: serde_json::Value,
        confidence: f32,
        reason: impl Into<String>,
        routing_layer: RoutingLayer,
        timeout: Duration,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            tool,
            parameters,
            confidence,
            reason: reason.into(),
            routing_layer,
            created_at: Instant::now(),
            timeout,
        }
    }

    /// Check if this confirmation has timed out
    /// Returns false if timeout is zero (no timeout, wait indefinitely)
    pub fn is_expired(&self) -> bool {
        // timeout == 0 means no timeout, wait indefinitely
        if self.timeout == Duration::ZERO {
            return false;
        }
        self.created_at.elapsed() > self.timeout
    }

    /// Get remaining time before timeout
    pub fn remaining_time(&self) -> Duration {
        self.timeout.saturating_sub(self.created_at.elapsed())
    }

    /// Convert to FFI-safe representation
    pub fn to_ffi(&self) -> PendingConfirmationInfo {
        PendingConfirmationInfo {
            id: self.id.clone(),
            tool_id: self.tool.id.clone(),
            tool_name: self.tool.name.clone(),
            tool_display_name: self.tool.display_name.clone(),
            tool_description: self.tool.description.clone(),
            parameters_json: serde_json::to_string(&self.parameters).unwrap_or_default(),
            confidence: self.confidence,
            reason: self.reason.clone(),
            timeout_ms: self.timeout.as_millis() as u64,
        }
    }
}

// =============================================================================
// FFI Types
// =============================================================================

/// FFI-safe pending confirmation info (for UniFFI export)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingConfirmationInfo {
    /// Unique confirmation ID
    pub id: String,

    /// Tool unique identifier
    pub tool_id: String,

    /// Tool name (command)
    pub tool_name: String,

    /// Tool display name
    pub tool_display_name: String,

    /// Tool description
    pub tool_description: String,

    /// Parameters as JSON string
    pub parameters_json: String,

    /// Confidence score (0.0-1.0)
    pub confidence: f32,

    /// Reason for tool selection
    pub reason: String,

    /// Timeout in milliseconds
    pub timeout_ms: u64,
}

/// User's decision on a pending confirmation (from Swift)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserConfirmationDecision {
    /// Execute the tool as proposed
    Execute,

    /// Cancel and fall back to general chat
    Cancel,

    /// Edit parameters before execution (reserved for future)
    EditParameters,
}

impl UserConfirmationDecision {
    /// Check if decision means proceed with execution
    pub fn should_execute(&self) -> bool {
        matches!(self, Self::Execute)
    }
}

// =============================================================================
// Confirmation State
// =============================================================================

/// State of a confirmation request
///
/// This is returned from routing when confirmation is needed,
/// allowing the caller to handle async confirmation flow.
#[derive(Debug, Clone)]
pub enum ConfirmationState {
    /// No confirmation needed - proceed directly
    NotRequired,

    /// Confirmation needed - contains pending info
    Pending(PendingConfirmation),

    /// User confirmed - execute the tool
    Confirmed {
        tool: UnifiedTool,
        parameters: serde_json::Value,
        routing_layer: RoutingLayer,
        confidence: f32,
    },

    /// User cancelled - fall back to chat
    Cancelled { reason: String },

    /// Confirmation timed out
    TimedOut { confirmation_id: String },
}

impl ConfirmationState {
    /// Create a pending confirmation state
    pub fn pending(confirmation: PendingConfirmation) -> Self {
        Self::Pending(confirmation)
    }

    /// Create a confirmed state from a pending confirmation
    pub fn confirmed(pending: &PendingConfirmation) -> Self {
        Self::Confirmed {
            tool: pending.tool.clone(),
            parameters: pending.parameters.clone(),
            routing_layer: pending.routing_layer,
            confidence: pending.confidence,
        }
    }

    /// Create a cancelled state
    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self::Cancelled {
            reason: reason.into(),
        }
    }

    /// Create a timed out state
    pub fn timed_out(confirmation_id: impl Into<String>) -> Self {
        Self::TimedOut {
            confirmation_id: confirmation_id.into(),
        }
    }

    /// Check if this is a pending state
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    /// Check if this requires UI interaction
    pub fn requires_ui(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    /// Get pending confirmation if in pending state
    pub fn pending_info(&self) -> Option<&PendingConfirmation> {
        match self {
            Self::Pending(p) => Some(p),
            _ => None,
        }
    }
}

// =============================================================================
// Pending Confirmation Store
// =============================================================================

/// Thread-safe store for pending confirmations
///
/// This stores confirmations that are awaiting user decision,
/// allowing lookup by ID when the user responds.
#[derive(Debug)]
pub struct PendingConfirmationStore {
    /// Pending confirmations by ID
    confirmations: RwLock<HashMap<String, PendingConfirmation>>,

    /// Maximum number of pending confirmations
    max_pending: usize,

    /// Default timeout for confirmations
    default_timeout: Duration,
}

impl PendingConfirmationStore {
    /// Create a new store with default settings
    /// Default timeout is zero (no timeout, wait indefinitely like Claude Code)
    pub fn new() -> Self {
        Self {
            confirmations: RwLock::new(HashMap::new()),
            max_pending: 100,
            default_timeout: Duration::ZERO, // No timeout, wait indefinitely
        }
    }

    /// Create a new store with custom settings
    pub fn with_config(max_pending: usize, default_timeout: Duration) -> Self {
        Self {
            confirmations: RwLock::new(HashMap::new()),
            max_pending,
            default_timeout,
        }
    }

    /// Insert a pending confirmation
    ///
    /// Returns false if store is full
    pub fn insert(&self, confirmation: PendingConfirmation) -> bool {
        let mut confirmations = self.confirmations.write().unwrap();

        // Cleanup expired confirmations first
        confirmations.retain(|_, c| !c.is_expired());

        // Check capacity
        if confirmations.len() >= self.max_pending {
            return false;
        }

        confirmations.insert(confirmation.id.clone(), confirmation);
        true
    }

    /// Get a pending confirmation by ID
    pub fn get(&self, id: &str) -> Option<PendingConfirmation> {
        let confirmations = self.confirmations.read().unwrap();
        confirmations.get(id).cloned()
    }

    /// Remove and return a pending confirmation by ID
    pub fn remove(&self, id: &str) -> Option<PendingConfirmation> {
        let mut confirmations = self.confirmations.write().unwrap();
        confirmations.remove(id)
    }

    /// Check if a confirmation exists and is not expired
    pub fn is_valid(&self, id: &str) -> bool {
        let confirmations = self.confirmations.read().unwrap();
        confirmations
            .get(id)
            .map(|c| !c.is_expired())
            .unwrap_or(false)
    }

    /// Get the number of pending confirmations
    pub fn count(&self) -> usize {
        let confirmations = self.confirmations.read().unwrap();
        confirmations.len()
    }

    /// Remove all expired confirmations
    ///
    /// Returns the number of removed confirmations
    /// Cleanup expired confirmations
    ///
    /// Returns the count of expired confirmations that were removed.
    pub fn cleanup_expired(&self) -> usize {
        let expired = self.cleanup_expired_with_ids();
        expired.len()
    }

    /// Cleanup expired confirmations and return their IDs
    ///
    /// Returns a list of confirmation IDs that were expired and removed.
    pub fn cleanup_expired_with_ids(&self) -> Vec<String> {
        let mut confirmations = self.confirmations.write().unwrap();
        let expired_ids: Vec<String> = confirmations
            .iter()
            .filter(|(_, c)| c.is_expired())
            .map(|(id, _)| id.clone())
            .collect();
        confirmations.retain(|_, c| !c.is_expired());
        expired_ids
    }

    /// Clear all pending confirmations
    pub fn clear(&self) {
        let mut confirmations = self.confirmations.write().unwrap();
        confirmations.clear();
    }

    /// Get the default timeout
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }
}

impl Default for PendingConfirmationStore {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Async Confirmation Handler
// =============================================================================

/// Handler for async confirmation flow
///
/// This manages the lifecycle of confirmations:
/// 1. Create pending confirmation
/// 2. Store for later resumption
/// 3. Handle user decision when received
pub struct AsyncConfirmationHandler {
    /// Pending confirmation store
    store: Arc<PendingConfirmationStore>,

    /// Configuration
    config: AsyncConfirmationConfig,
}

/// Configuration for async confirmation
#[derive(Debug, Clone)]
pub struct AsyncConfirmationConfig {
    /// Confidence threshold below which confirmation is required
    pub threshold: f32,

    /// Whether confirmation is enabled globally
    pub enabled: bool,

    /// Default timeout for confirmations (ms)
    pub timeout_ms: u64,

    /// Whether to skip confirmation for native tools
    pub skip_native_tools: bool,
}

impl Default for AsyncConfirmationConfig {
    fn default() -> Self {
        Self {
            threshold: 0.7,
            enabled: true,
            timeout_ms: 0, // 0 = no timeout, wait indefinitely (like Claude Code)
            skip_native_tools: false,
        }
    }
}

impl AsyncConfirmationHandler {
    /// Create a new handler with default config
    pub fn new() -> Self {
        Self::with_config(AsyncConfirmationConfig::default())
    }

    /// Create a new handler with custom config
    pub fn with_config(config: AsyncConfirmationConfig) -> Self {
        let timeout = Duration::from_millis(config.timeout_ms);
        Self {
            store: Arc::new(PendingConfirmationStore::with_config(100, timeout)),
            config,
        }
    }

    /// Create a new handler with shared store
    pub fn with_store(
        store: Arc<PendingConfirmationStore>,
        config: AsyncConfirmationConfig,
    ) -> Self {
        Self { store, config }
    }

    /// Check if confirmation is needed for given confidence
    pub fn should_confirm(&self, confidence: f32) -> bool {
        self.config.enabled && confidence < self.config.threshold
    }

    /// Get the confidence threshold
    pub fn threshold(&self) -> f32 {
        self.config.threshold
    }

    /// Create a pending confirmation
    ///
    /// This creates a new pending confirmation and stores it for later resumption.
    /// Returns the confirmation state for the caller to notify Swift.
    pub fn create_pending(
        &self,
        tool: UnifiedTool,
        parameters: serde_json::Value,
        confidence: f32,
        reason: impl Into<String>,
        routing_layer: RoutingLayer,
    ) -> ConfirmationState {
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let pending =
            PendingConfirmation::new(tool, parameters, confidence, reason, routing_layer, timeout);

        if self.store.insert(pending.clone()) {
            ConfirmationState::pending(pending)
        } else {
            // Store is full - cancel with reason
            ConfirmationState::cancelled("Too many pending confirmations")
        }
    }

    /// Resume with user's decision
    ///
    /// Called when Swift sends back the user's decision.
    /// Returns the final confirmation state for processing.
    pub fn resume_with_decision(
        &self,
        confirmation_id: &str,
        decision: UserConfirmationDecision,
    ) -> ConfirmationState {
        // Remove the pending confirmation
        let Some(pending) = self.store.remove(confirmation_id) else {
            return ConfirmationState::cancelled("Confirmation not found or expired");
        };

        // Check if expired
        if pending.is_expired() {
            return ConfirmationState::timed_out(confirmation_id);
        }

        // Apply decision
        match decision {
            UserConfirmationDecision::Execute => ConfirmationState::confirmed(&pending),
            UserConfirmationDecision::Cancel => ConfirmationState::cancelled("User cancelled"),
            UserConfirmationDecision::EditParameters => {
                // For now, treat as cancel (parameter editing not implemented)
                ConfirmationState::cancelled("Parameter editing not yet implemented")
            }
        }
    }

    /// Get a pending confirmation by ID
    pub fn get_pending(&self, id: &str) -> Option<PendingConfirmation> {
        self.store.get(id)
    }

    /// Cancel a pending confirmation
    pub fn cancel(&self, id: &str) -> bool {
        self.store.remove(id).is_some()
    }

    /// Get the number of pending confirmations
    pub fn pending_count(&self) -> usize {
        self.store.count()
    }

    /// Cleanup expired confirmations
    ///
    /// Returns the count of expired confirmations that were removed.
    pub fn cleanup_expired(&self) -> Vec<String> {
        self.store.cleanup_expired_with_ids()
    }

    /// Get the store reference
    pub fn store(&self) -> &Arc<PendingConfirmationStore> {
        &self.store
    }
}

impl Default for AsyncConfirmationHandler {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;

    fn create_test_tool() -> UnifiedTool {
        UnifiedTool::new(
            "native:search",
            "search",
            "Search the web",
            ToolSource::Native,
        )
    }

    #[test]
    fn test_pending_confirmation_creation() {
        let tool = create_test_tool();
        let params = serde_json::json!({"query": "test"});

        let pending = PendingConfirmation::new(
            tool,
            params,
            0.6,
            "Test reason",
            RoutingLayer::L3Inference,
            Duration::from_secs(30),
        );

        assert!(!pending.id.is_empty());
        assert_eq!(pending.confidence, 0.6);
        assert_eq!(pending.reason, "Test reason");
        assert!(!pending.is_expired());
    }

    #[test]
    fn test_pending_confirmation_expiry() {
        let tool = create_test_tool();
        let params = serde_json::json!({});

        let pending = PendingConfirmation::new(
            tool,
            params,
            0.5,
            "Test",
            RoutingLayer::L3Inference,
            Duration::from_millis(1), // Very short timeout
        );

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(10));

        assert!(pending.is_expired());
        assert_eq!(pending.remaining_time(), Duration::ZERO);
    }

    #[test]
    fn test_pending_confirmation_to_ffi() {
        let tool = create_test_tool();
        let params = serde_json::json!({"key": "value"});

        let pending = PendingConfirmation::new(
            tool,
            params,
            0.65,
            "Reason",
            RoutingLayer::L2Semantic,
            Duration::from_secs(30),
        );

        let ffi = pending.to_ffi();

        assert_eq!(ffi.id, pending.id);
        assert_eq!(ffi.tool_id, "native:search");
        assert_eq!(ffi.tool_name, "search");
        assert_eq!(ffi.confidence, 0.65);
        assert_eq!(ffi.timeout_ms, 30000); // This test uses explicit 30s timeout
    }

    #[test]
    fn test_confirmation_state_variants() {
        // NotRequired
        let state = ConfirmationState::NotRequired;
        assert!(!state.is_pending());
        assert!(!state.requires_ui());

        // Cancelled
        let state = ConfirmationState::cancelled("User cancelled");
        if let ConfirmationState::Cancelled { reason } = state {
            assert_eq!(reason, "User cancelled");
        } else {
            panic!("Expected Cancelled variant");
        }

        // TimedOut
        let state = ConfirmationState::timed_out("123");
        if let ConfirmationState::TimedOut { confirmation_id } = state {
            assert_eq!(confirmation_id, "123");
        } else {
            panic!("Expected TimedOut variant");
        }
    }

    #[test]
    fn test_pending_store_insert_and_get() {
        let store = PendingConfirmationStore::new();
        let tool = create_test_tool();

        let pending = PendingConfirmation::new(
            tool,
            serde_json::json!({}),
            0.5,
            "Test",
            RoutingLayer::L3Inference,
            Duration::from_secs(30),
        );

        let id = pending.id.clone();

        assert!(store.insert(pending));
        assert_eq!(store.count(), 1);
        assert!(store.is_valid(&id));

        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.id, id);
    }

    #[test]
    fn test_pending_store_remove() {
        let store = PendingConfirmationStore::new();
        let tool = create_test_tool();

        let pending = PendingConfirmation::new(
            tool,
            serde_json::json!({}),
            0.5,
            "Test",
            RoutingLayer::L3Inference,
            Duration::from_secs(30),
        );

        let id = pending.id.clone();
        store.insert(pending);

        let removed = store.remove(&id).unwrap();
        assert_eq!(removed.id, id);
        assert_eq!(store.count(), 0);
        assert!(!store.is_valid(&id));
    }

    #[test]
    fn test_pending_store_cleanup_expired() {
        let store = PendingConfirmationStore::with_config(100, Duration::from_millis(1));
        let tool = create_test_tool();

        let pending = PendingConfirmation::new(
            tool,
            serde_json::json!({}),
            0.5,
            "Test",
            RoutingLayer::L3Inference,
            Duration::from_millis(1),
        );

        store.insert(pending);
        std::thread::sleep(Duration::from_millis(10));

        let cleaned = store.cleanup_expired();
        assert_eq!(cleaned, 1);
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_async_handler_should_confirm() {
        let handler = AsyncConfirmationHandler::new();

        // Below threshold - should confirm
        assert!(handler.should_confirm(0.5));
        assert!(handler.should_confirm(0.69));

        // At or above threshold - should not confirm
        assert!(!handler.should_confirm(0.7));
        assert!(!handler.should_confirm(0.9));
    }

    #[test]
    fn test_async_handler_create_pending() {
        let handler = AsyncConfirmationHandler::new();
        let tool = create_test_tool();

        let state = handler.create_pending(
            tool,
            serde_json::json!({"query": "test"}),
            0.5,
            "Test reason",
            RoutingLayer::L3Inference,
        );

        assert!(state.is_pending());
        assert!(state.requires_ui());
        assert_eq!(handler.pending_count(), 1);
    }

    #[test]
    fn test_async_handler_resume_execute() {
        let handler = AsyncConfirmationHandler::new();
        let tool = create_test_tool();

        let state = handler.create_pending(
            tool,
            serde_json::json!({}),
            0.5,
            "Test",
            RoutingLayer::L3Inference,
        );

        let pending_id = state.pending_info().unwrap().id.clone();

        let resumed = handler.resume_with_decision(&pending_id, UserConfirmationDecision::Execute);

        match resumed {
            ConfirmationState::Confirmed { tool, .. } => {
                assert_eq!(tool.name, "search");
            }
            _ => panic!("Expected Confirmed state"),
        }

        assert_eq!(handler.pending_count(), 0);
    }

    #[test]
    fn test_async_handler_resume_cancel() {
        let handler = AsyncConfirmationHandler::new();
        let tool = create_test_tool();

        let state = handler.create_pending(
            tool,
            serde_json::json!({}),
            0.5,
            "Test",
            RoutingLayer::L3Inference,
        );

        let pending_id = state.pending_info().unwrap().id.clone();

        let resumed = handler.resume_with_decision(&pending_id, UserConfirmationDecision::Cancel);

        match resumed {
            ConfirmationState::Cancelled { reason } => {
                assert_eq!(reason, "User cancelled");
            }
            _ => panic!("Expected Cancelled state"),
        }
    }

    #[test]
    fn test_async_handler_resume_not_found() {
        let handler = AsyncConfirmationHandler::new();

        let resumed =
            handler.resume_with_decision("nonexistent", UserConfirmationDecision::Execute);

        match resumed {
            ConfirmationState::Cancelled { reason } => {
                assert!(reason.contains("not found"));
            }
            _ => panic!("Expected Cancelled state"),
        }
    }

    #[test]
    fn test_user_decision_should_execute() {
        assert!(UserConfirmationDecision::Execute.should_execute());
        assert!(!UserConfirmationDecision::Cancel.should_execute());
        assert!(!UserConfirmationDecision::EditParameters.should_execute());
    }
}
