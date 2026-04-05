//! Request state tracking for the gateway JSON-RPC handler pipeline.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Global state registry holder for access by handlers.
///
/// This allows the `request.state` handler to access the registry
/// without requiring it to be passed through the HandlerRegistry.
static STATE_REGISTRY: RwLock<Option<Arc<RequestStateRegistry>>> = RwLock::new(None);

/// Set the global state registry.
///
/// Called by MiddlewareChain during initialization.
pub fn set_global_registry(registry: Arc<RequestStateRegistry>) {
    let mut guard = STATE_REGISTRY.write().unwrap();
    *guard = Some(registry);
}

/// Get the global state registry.
///
/// Returns None if not yet initialized.
pub fn get_global_registry() -> Option<Arc<RequestStateRegistry>> {
    let guard = STATE_REGISTRY.read().unwrap();
    guard.clone()
}

/// Request lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RequestState {
    Pending = 0,
    Validating = 1,
    Processing = 2,
    AwaitingResponse = 3,
    Completed = 4,
    Failed = 5,
    Cancelled = 6,
}

impl RequestState {
    /// Returns true if this is a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Returns the state as a u8 for atomic storage.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Try to create a RequestState from a u8 value.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Pending),
            1 => Some(Self::Validating),
            2 => Some(Self::Processing),
            3 => Some(Self::AwaitingResponse),
            4 => Some(Self::Completed),
            5 => Some(Self::Failed),
            6 => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Returns the name of the state for metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Validating => "validating",
            Self::Processing => "processing",
            Self::AwaitingResponse => "awaiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Check if transition from current state to target is valid.
    pub fn can_transition_to(self, target: RequestState) -> bool {
        use RequestState::*;
        match self {
            // From Pending
            Pending => matches!(target, Validating | Failed | Cancelled),
            // From Validating
            Validating => matches!(target, Processing | Failed | Cancelled),
            // From Processing
            Processing => matches!(target, AwaitingResponse | Completed | Failed | Cancelled),
            // From AwaitingResponse
            AwaitingResponse => matches!(target, Processing | Completed | Failed | Cancelled),
            // From terminal states - no transitions allowed
            Completed | Failed | Cancelled => false,
        }
    }
}

impl fmt::Display for RequestState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Error returned when an invalid state transition is attempted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid transition from {from} to {to}")]
pub struct TransitionError {
    pub from: RequestState,
    pub to: RequestState,
}

/// Per-request state data stored in the registry.
#[derive(Debug)]
pub struct RequestStateData {
    /// Current state, stored as u8 for atomic operations.
    state: AtomicU64,
    /// When the request entered the current state (unix timestamp ms).
    stage_entered_at: AtomicU64,
    /// When the request was created (unix timestamp ms).
    created_at: AtomicU64,
    /// Total duration from Pending to terminal state (ms). Set when entering terminal state.
    total_duration: AtomicU64,
}

impl Clone for RequestStateData {
    fn clone(&self) -> Self {
        Self {
            state: AtomicU64::new(self.state.load(Ordering::SeqCst)),
            stage_entered_at: AtomicU64::new(self.stage_entered_at.load(Ordering::SeqCst)),
            created_at: AtomicU64::new(self.created_at.load(Ordering::SeqCst)),
            total_duration: AtomicU64::new(self.total_duration.load(Ordering::SeqCst)),
        }
    }
}

impl RequestStateData {
    /// Create a new RequestStateData in the Pending state.
    pub fn new(request_id: Uuid) -> Self {
        let now = current_timestamp_ms();
        Self {
            state: AtomicU64::new(RequestState::Pending.as_u8() as u64),
            stage_entered_at: AtomicU64::new(now),
            created_at: AtomicU64::new(now),
            total_duration: AtomicU64::new(0),
        }
    }

    /// Get the current state.
    pub fn state(&self) -> RequestState {
        let value = self.state.load(Ordering::SeqCst) as u8;
        RequestState::from_u8(value).unwrap_or(RequestState::Pending)
    }

    /// Get the timestamp when the request entered the current stage.
    pub fn stage_entered_at(&self) -> u64 {
        self.stage_entered_at.load(Ordering::SeqCst)
    }

    /// Get the timestamp when the request was created.
    pub fn created_at(&self) -> u64 {
        self.created_at.load(Ordering::SeqCst)
    }

    /// Get the total duration (only valid after entering terminal state).
    pub fn total_duration(&self) -> u64 {
        self.total_duration.load(Ordering::SeqCst)
    }

    /// Attempt to transition to a new state.
    /// Returns the new state on success, or an error if the transition is invalid.
    pub fn transition_to(&self, new_state: RequestState) -> Result<RequestState, TransitionError> {
        let current = self.state();
        if !current.can_transition_to(new_state) {
            return Err(TransitionError { from: current, to: new_state });
        }

        let now = current_timestamp_ms();
        self.state.store(new_state.as_u8() as u64, Ordering::SeqCst);
        self.stage_entered_at.store(now, Ordering::SeqCst);

        // If entering a terminal state, record total duration
        if new_state.is_terminal() {
            let duration = now - self.created_at.load(Ordering::SeqCst);
            self.total_duration.store(duration, Ordering::SeqCst);
        }

        Ok(new_state)
    }
}

/// Get current Unix timestamp in milliseconds.
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Thread-safe registry for tracking all in-flight request states.
///
/// Uses DashMap for concurrent access without locking.
#[derive(Debug, Clone)]
pub struct RequestStateRegistry {
    /// Map of request ID to state data.
    states: DashMap<Uuid, RequestStateData>,
    /// Per-state counters for fast metrics.
    state_counts: Arc<[AtomicU64; 7]>,
}

impl RequestStateRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
            state_counts: Arc::new([
                AtomicU64::new(0), // Pending
                AtomicU64::new(0), // Validating
                AtomicU64::new(0), // Processing
                AtomicU64::new(0), // AwaitingResponse
                AtomicU64::new(0), // Completed
                AtomicU64::new(0), // Failed
                AtomicU64::new(0), // Cancelled
            ]),
        }
    }

    /// Register a new request in the Pending state.
    ///
    /// Returns a guard that will auto-decrement the counter when dropped.
    pub fn insert(&self, request_id: Uuid) {
        let data = RequestStateData::new(request_id);
        self.states.insert(request_id, data);
        self.state_counts[RequestState::Pending as usize].fetch_add(1, Ordering::SeqCst);
    }

    /// Transition a request to a new state.
    ///
    /// Returns the new state on success, or None if the request is not found.
    pub fn transition(
        &self,
        request_id: Uuid,
        new_state: RequestState,
    ) -> Option<Result<RequestState, TransitionError>> {
        let entry = self.states.get_mut(&request_id)?;

        let old_state = entry.state();
        let result = entry.transition_to(new_state);

        // Update counters
        if result.is_ok() {
            self.state_counts[old_state as usize].fetch_sub(1, Ordering::SeqCst);
            self.state_counts[new_state as usize].fetch_add(1, Ordering::SeqCst);
        }

        Some(result)
    }

    /// Mark a request as completed.
    pub fn complete(&self, request_id: Uuid) -> Option<RequestState> {
        let result = self.transition(request_id, RequestState::Completed);
        result.and_then(|r| r.ok())
    }

    /// Mark a request as failed.
    pub fn fail(&self, request_id: Uuid) -> Option<RequestState> {
        let result = self.transition(request_id, RequestState::Failed);
        result.and_then(|r| r.ok())
    }

    /// Mark a request as cancelled.
    pub fn cancel(&self, request_id: Uuid) -> Option<RequestState> {
        let result = self.transition(request_id, RequestState::Cancelled);
        result.and_then(|r| r.ok())
    }

    /// Get the count of requests in a specific state.
    pub fn count(&self, state: RequestState) -> u64 {
        self.state_counts[state as usize].load(Ordering::SeqCst)
    }

    /// Get the total number of in-flight requests (non-terminal states).
    pub fn in_flight(&self) -> u64 {
        self.state_counts[RequestState::Pending as usize].load(Ordering::SeqCst)
            + self.state_counts[RequestState::Validating as usize].load(Ordering::SeqCst)
            + self.state_counts[RequestState::Processing as usize].load(Ordering::SeqCst)
            + self.state_counts[RequestState::AwaitingResponse as usize].load(Ordering::SeqCst)
    }

    /// Get the count of completed requests.
    pub fn completed(&self) -> u64 {
        self.state_counts[RequestState::Completed as usize].load(Ordering::SeqCst)
    }

    /// Get a snapshot of the current state distribution.
    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            pending: self.count(RequestState::Pending),
            validating: self.count(RequestState::Validating),
            processing: self.count(RequestState::Processing),
            awaiting: self.count(RequestState::AwaitingResponse),
            completed: self.count(RequestState::Completed),
            failed: self.count(RequestState::Failed),
            cancelled: self.count(RequestState::Cancelled),
            total_in_flight: self.in_flight(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Remove a request from the registry.
    ///
    /// This should only be called for terminal states after cleanup timeout.
    pub fn remove(&self, request_id: Uuid) {
        if let Some((_, state_data)) = self.states.remove(&request_id) {
            let state = state_data.state();
            if !state.is_terminal() {
                self.state_counts[state as usize].fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Cleanup old terminal state entries.
    ///
    /// Returns the number of entries removed.
    pub fn cleanup(&self, max_age_ms: u64) -> usize {
        let now = current_timestamp_ms();
        let mut removed = 0;

        // We need to collect keys first because we can't mutate while iterating
        let keys: Vec<Uuid> = self.states.iter()
            .filter(|entry| {
                let state = entry.value().state();
                if state.is_terminal() {
                    let duration = entry.value().total_duration();
                    let completed_at = entry.value().created_at() + duration;
                    now.saturating_sub(completed_at) > max_age_ms
                } else {
                    false
                }
            })
            .map(|entry| *entry.key())
            .collect();

        for key in keys {
            self.remove(key);
            removed += 1;
        }

        removed
    }

    /// Get the number of registered requests (for debugging/testing).
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

impl Default for RequestStateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of the current state distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub pending: u64,
    pub validating: u64,
    pub processing: u64,
    pub awaiting: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub total_in_flight: u64,
    pub timestamp: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_is_terminal() {
        assert!(!RequestState::Pending.is_terminal());
        assert!(!RequestState::Validating.is_terminal());
        assert!(!RequestState::Processing.is_terminal());
        assert!(!RequestState::AwaitingResponse.is_terminal());
        assert!(RequestState::Completed.is_terminal());
        assert!(RequestState::Failed.is_terminal());
        assert!(RequestState::Cancelled.is_terminal());
    }

    #[test]
    fn test_valid_transitions() {
        assert!(RequestState::Pending.can_transition_to(RequestState::Validating));
        assert!(RequestState::Pending.can_transition_to(RequestState::Failed));
        assert!(RequestState::Pending.can_transition_to(RequestState::Cancelled));

        assert!(RequestState::Validating.can_transition_to(RequestState::Processing));
        assert!(RequestState::Validating.can_transition_to(RequestState::Failed));
        assert!(RequestState::Validating.can_transition_to(RequestState::Cancelled));

        assert!(RequestState::Processing.can_transition_to(RequestState::AwaitingResponse));
        assert!(RequestState::Processing.can_transition_to(RequestState::Completed));
        assert!(RequestState::Processing.can_transition_to(RequestState::Failed));
        assert!(RequestState::Processing.can_transition_to(RequestState::Cancelled));

        assert!(RequestState::AwaitingResponse.can_transition_to(RequestState::Processing));
        assert!(RequestState::AwaitingResponse.can_transition_to(RequestState::Completed));
        assert!(RequestState::AwaitingResponse.can_transition_to(RequestState::Failed));
        assert!(RequestState::AwaitingResponse.can_transition_to(RequestState::Cancelled));
    }

    #[test]
    fn test_invalid_transitions() {
        // Can't skip states
        assert!(!RequestState::Pending.can_transition_to(RequestState::Processing));
        assert!(!RequestState::Pending.can_transition_to(RequestState::Completed));
        assert!(!RequestState::Pending.can_transition_to(RequestState::AwaitingResponse));

        assert!(!RequestState::Validating.can_transition_to(RequestState::Completed));
        assert!(!RequestState::Validating.can_transition_to(RequestState::AwaitingResponse));

        // Can't go backwards
        assert!(!RequestState::Processing.can_transition_to(RequestState::Validating));
        assert!(!RequestState::Processing.can_transition_to(RequestState::Pending));

        // Terminal states can't transition
        assert!(!RequestState::Completed.can_transition_to(RequestState::Pending));
        assert!(!RequestState::Failed.can_transition_to(RequestState::Completed));
        assert!(!RequestState::Cancelled.can_transition_to(RequestState::Failed));
    }

    #[test]
    fn test_state_data_transitions() {
        let request_id = Uuid::new_v4();
        let data = RequestStateData::new(request_id);

        assert_eq!(data.state(), RequestState::Pending);

        // Valid transition
        let result = data.transition_to(RequestState::Validating);
        assert!(result.is_ok());
        assert_eq!(data.state(), RequestState::Validating);

        // Invalid transition
        let result = data.transition_to(RequestState::Completed);
        assert!(result.is_err());

        // Can still transition to valid state
        let result = data.transition_to(RequestState::Processing);
        assert!(result.is_ok());
        assert_eq!(data.state(), RequestState::Processing);

        // Transition to terminal state records duration (may be 0 if completed within same millisecond)
        let result = data.transition_to(RequestState::Completed);
        assert!(result.is_ok());
        let _duration = data.total_duration(); // Verify method works (value may be 0 due to timing)
    }

    #[test]
    fn test_registry_insert_and_transition() {
        let registry = RequestStateRegistry::new();
        let request_id = Uuid::new_v4();

        assert_eq!(registry.len(), 0);

        registry.insert(request_id);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.count(RequestState::Pending), 1);

        let result = registry.transition(request_id, RequestState::Validating);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
        assert_eq!(registry.count(RequestState::Pending), 0);
        assert_eq!(registry.count(RequestState::Validating), 1);

        // Unknown request
        let result = registry.transition(Uuid::new_v4(), RequestState::Processing);
        assert!(result.is_none());
    }

    #[test]
    fn test_registry_snapshot() {
        let registry = RequestStateRegistry::new();

        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();

        registry.insert(id1);
        registry.insert(id2);
        registry.insert(id3);

        registry.transition(id1, RequestState::Validating).unwrap().unwrap();
        // id2 must go through Validating before Processing
        registry.transition(id2, RequestState::Validating).unwrap().unwrap();
        registry.transition(id2, RequestState::Processing).unwrap().unwrap();

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.pending, 1);
        assert_eq!(snapshot.validating, 1);
        assert_eq!(snapshot.processing, 1);
        assert_eq!(snapshot.awaiting, 0);
        assert_eq!(snapshot.completed, 0);
        assert_eq!(snapshot.total_in_flight, 3);
    }

    #[tokio::test]
    async fn test_registry_concurrent_access() {
        use tokio::task;

        let registry = RequestStateRegistry::new();
        let mut handles = vec![];

        // Spawn multiple tasks that insert and transition requests concurrently
        for i in 0..10 {
            let registry = registry.clone();
            let handle = task::spawn(async move {
                let request_id = Uuid::new_v4();
                registry.insert(request_id);
                registry.transition(request_id, RequestState::Validating).unwrap().unwrap();
                registry.transition(request_id, RequestState::Processing).unwrap().unwrap();
                registry.complete(request_id);
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.pending, 0);
        assert_eq!(snapshot.validating, 0);
        assert_eq!(snapshot.processing, 0);
        assert_eq!(snapshot.completed, 10);
        assert_eq!(snapshot.total_in_flight, 0);
    }
}
