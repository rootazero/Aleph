//! Async User Input Flow for Agent Loop
//!
//! This module implements non-blocking user input collection for AgentLoop:
//!
//! - Stores pending input requests with oneshot channels
//! - Swift receives `on_user_input_request` callback
//! - Swift calls `respond_to_user_input(request_id, response)` when user responds
//! - `on_user_input_required()` awaits oneshot and returns response
//!
//! # Architecture
//!
//! ```text
//! AgentLoop needs user input
//!       ↓
//! ┌───────────────────────────────────────┐
//! │      User Input Request Flow          │
//! │                                       │
//! │  1. Generate unique request_id        │
//! │  2. Create oneshot channel            │
//! │  3. Store in PENDING_USER_INPUTS      │
//! │  4. Call handler.on_user_input_request│
//! │  5. await oneshot.recv()              │
//! │  6. Return user response              │
//! └───────────────────────────────────────┘
//!       ↓
//! Swift shows input dialog/prompt
//!       ↓
//! User types response or selects option
//!       ↓
//! Swift calls AetherCore.respond_to_user_input(request_id, response)
//!       ↓
//! Oneshot sender sends response
//!       ↓
//! on_user_input_required() returns
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{info, warn};

/// Default timeout for user input requests
/// Duration::ZERO means no timeout - wait indefinitely
const DEFAULT_TIMEOUT: Duration = Duration::ZERO;

/// Counter for generating unique request IDs
static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request ID
pub fn generate_request_id() -> String {
    let id = REQUEST_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("user_input_{}", id)
}

/// A pending user input request awaiting response
pub struct PendingUserInput {
    /// The oneshot sender to deliver the user's response
    pub sender: oneshot::Sender<String>,
    /// When this request was created
    pub created_at: Instant,
    /// The question being asked (stored for debugging/logging)
    #[allow(dead_code)]
    pub question: String,
    /// Optional list of choices (stored for debugging/logging)
    #[allow(dead_code)]
    pub options: Option<Vec<String>>,
}

impl PendingUserInput {
    /// Create a new pending user input request
    pub fn new(question: String, options: Option<Vec<String>>) -> (Self, oneshot::Receiver<String>) {
        let (sender, receiver) = oneshot::channel();
        let pending = Self {
            sender,
            created_at: Instant::now(),
            question,
            options,
        };
        (pending, receiver)
    }

    /// Check if this request has expired
    /// Returns false if DEFAULT_TIMEOUT is zero (no timeout)
    pub fn is_expired(&self) -> bool {
        if DEFAULT_TIMEOUT == Duration::ZERO {
            return false;
        }
        self.created_at.elapsed() > DEFAULT_TIMEOUT
    }
}

/// Global store for pending user input requests
///
/// This stores requests that are awaiting user response,
/// allowing the FFI function `respond_to_user_input` to find and complete them.
static PENDING_USER_INPUTS: std::sync::LazyLock<RwLock<HashMap<String, PendingUserInput>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Store a pending user input request
///
/// Returns the oneshot receiver to wait on for the response and the request ID.
pub fn store_pending_input(
    question: String,
    options: Option<Vec<String>>,
) -> (String, oneshot::Receiver<String>) {
    let request_id = generate_request_id();
    let (pending, receiver) = PendingUserInput::new(question, options);

    let mut store = PENDING_USER_INPUTS.write().unwrap();

    // Clean up expired requests first
    store.retain(|_, p| !p.is_expired());

    // Store the new request
    store.insert(request_id.clone(), pending);

    info!(request_id = %request_id, "Stored pending user input request");

    (request_id, receiver)
}

/// Complete a pending user input request with the user's response
///
/// Called from `AetherCore::respond_to_user_input()` FFI function.
///
/// # Arguments
/// * `request_id` - The request ID to respond to
/// * `response` - The user's response
///
/// # Returns
/// `true` if the request was found and completed, `false` otherwise.
pub fn complete_pending_input(request_id: &str, response: String) -> bool {
    let mut store = PENDING_USER_INPUTS.write().unwrap();

    if let Some(pending) = store.remove(request_id) {
        if pending.is_expired() {
            warn!(request_id = %request_id, "User input request expired");
            // Send empty response for expired requests
            let _ = pending.sender.send(String::new());
            return false;
        }

        info!(request_id = %request_id, response_len = response.len(), "Completing user input request");
        let _ = pending.sender.send(response);
        true
    } else {
        warn!(request_id = %request_id, "User input request not found");
        false
    }
}

/// Cancel all pending user input requests
///
/// Called on operation cancellation to clean up.
pub fn cancel_all_pending_inputs() {
    let mut store = PENDING_USER_INPUTS.write().unwrap();
    for (request_id, pending) in store.drain() {
        info!(request_id = %request_id, "Cancelling pending user input request");
        let _ = pending.sender.send(String::new()); // Send empty response
    }
}

/// Get the count of pending input requests (used in tests)
#[allow(dead_code)]
pub fn pending_input_count() -> usize {
    let store = PENDING_USER_INPUTS.read().unwrap();
    store.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_complete_input() {
        let (request_id, receiver) =
            store_pending_input("What is your name?".to_string(), None);

        // Complete it
        assert!(complete_pending_input(&request_id, "Alice".to_string()));

        // Should receive the response
        let response = receiver.await.unwrap();
        assert_eq!(response, "Alice");
    }

    #[tokio::test]
    async fn test_store_with_options() {
        let options = Some(vec![
            "Option A".to_string(),
            "Option B".to_string(),
            "Option C".to_string(),
        ]);
        let (request_id, receiver) =
            store_pending_input("Choose one:".to_string(), options);

        assert!(complete_pending_input(&request_id, "Option B".to_string()));

        let response = receiver.await.unwrap();
        assert_eq!(response, "Option B");
    }

    #[test]
    fn test_complete_not_found() {
        assert!(!complete_pending_input("nonexistent", "response".to_string()));
    }

    #[test]
    fn test_cancel_all() {
        let (_id1, _recv1) = store_pending_input("Question 1".to_string(), None);
        let (_id2, _recv2) = store_pending_input("Question 2".to_string(), None);

        assert_eq!(pending_input_count(), 2);

        cancel_all_pending_inputs();

        assert_eq!(pending_input_count(), 0);
    }

    #[test]
    fn test_unique_request_ids() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        let id3 = generate_request_id();

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }
}
