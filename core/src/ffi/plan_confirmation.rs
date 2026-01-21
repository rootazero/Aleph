//! Async Plan Confirmation Flow for DAG Scheduler
//!
//! This module implements non-blocking confirmation flow for DAG task execution:
//!
//! - Stores pending confirmations with oneshot channels
//! - Swift receives `on_plan_confirmation_required` callback
//! - Swift calls `confirm_task_plan(plan_id, decision)` to respond
//! - `on_confirmation_required()` waits on oneshot and returns decision
//!
//! # Architecture
//!
//! ```text
//! DAG Scheduler detects high-risk tasks
//!       ↓
//! ┌───────────────────────────────────────┐
//! │      Plan Confirmation Flow           │
//! │                                       │
//! │  1. Create pending with oneshot       │
//! │  2. Store in PENDING_PLAN_CONFIRMATIONS │
//! │  3. Call handler.on_plan_confirmation │
//! │  4. await oneshot.recv()              │
//! │  5. Return UserDecision               │
//! └───────────────────────────────────────┘
//!       ↓
//! Swift shows confirmation dialog
//!       ↓
//! User clicks Confirm/Cancel
//!       ↓
//! Swift calls AetherCore.confirm_task_plan(plan_id, decision)
//!       ↓
//! Oneshot sender sends decision
//!       ↓
//! on_confirmation_required() returns
//! ```

use crate::dispatcher::{DagTaskPlan, UserDecision};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{info, warn};

/// Default timeout for plan confirmations (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A pending plan confirmation awaiting user decision
pub struct PendingPlanConfirmation {
    /// The oneshot sender to deliver the user's decision
    pub sender: oneshot::Sender<UserDecision>,
    /// When this confirmation was created
    pub created_at: Instant,
    /// The plan that needs confirmation
    pub plan: DagTaskPlan,
}

impl PendingPlanConfirmation {
    /// Create a new pending confirmation
    pub fn new(plan: DagTaskPlan) -> (Self, oneshot::Receiver<UserDecision>) {
        let (sender, receiver) = oneshot::channel();
        let pending = Self {
            sender,
            created_at: Instant::now(),
            plan,
        };
        (pending, receiver)
    }

    /// Check if this confirmation has expired
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > DEFAULT_TIMEOUT
    }
}

/// Global store for pending plan confirmations
///
/// This stores confirmations that are awaiting user decision,
/// allowing the FFI function `confirm_task_plan` to find and complete them.
static PENDING_PLAN_CONFIRMATIONS: std::sync::LazyLock<RwLock<HashMap<String, PendingPlanConfirmation>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Store a pending plan confirmation
///
/// Returns the oneshot receiver to wait on for the decision.
pub fn store_pending_confirmation(
    plan_id: String,
    plan: DagTaskPlan,
) -> oneshot::Receiver<UserDecision> {
    let (pending, receiver) = PendingPlanConfirmation::new(plan);

    let mut store = PENDING_PLAN_CONFIRMATIONS.write().unwrap();

    // Clean up expired confirmations first
    store.retain(|_, p| !p.is_expired());

    // Store the new confirmation
    store.insert(plan_id.clone(), pending);

    info!(plan_id = %plan_id, "Stored pending plan confirmation");

    receiver
}

/// Complete a pending confirmation with the user's decision
///
/// Called from `AetherCore::confirm_task_plan()` FFI function.
///
/// # Arguments
/// * `plan_id` - The plan ID to confirm
/// * `decision` - The user's decision (Confirmed or Cancelled)
///
/// # Returns
/// `true` if the confirmation was found and completed, `false` otherwise.
pub fn complete_pending_confirmation(plan_id: &str, decision: UserDecision) -> bool {
    let mut store = PENDING_PLAN_CONFIRMATIONS.write().unwrap();

    if let Some(pending) = store.remove(plan_id) {
        if pending.is_expired() {
            warn!(plan_id = %plan_id, "Plan confirmation expired");
            // Try to send anyway (receiver will get error if dropped)
            let _ = pending.sender.send(UserDecision::Cancelled);
            return false;
        }

        info!(plan_id = %plan_id, decision = ?decision, "Completing plan confirmation");
        let _ = pending.sender.send(decision);
        true
    } else {
        warn!(plan_id = %plan_id, "Plan confirmation not found");
        false
    }
}

/// Cancel all pending confirmations
///
/// Called on cancellation to clean up.
pub fn cancel_all_pending_confirmations() {
    let mut store = PENDING_PLAN_CONFIRMATIONS.write().unwrap();
    for (plan_id, pending) in store.drain() {
        info!(plan_id = %plan_id, "Cancelling pending plan confirmation");
        let _ = pending.sender.send(UserDecision::Cancelled);
    }
}

/// Get the count of pending confirmations
pub fn pending_confirmation_count() -> usize {
    let store = PENDING_PLAN_CONFIRMATIONS.read().unwrap();
    store.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::{DagTaskDisplayStatus, DagTaskInfo, DagTaskPlan};

    fn create_test_plan() -> DagTaskPlan {
        DagTaskPlan {
            id: "test_plan_1".to_string(),
            title: "Test Plan".to_string(),
            tasks: vec![DagTaskInfo {
                id: "task_1".to_string(),
                name: "Test Task".to_string(),
                status: DagTaskDisplayStatus::Pending,
                risk_level: "high".to_string(),
                dependencies: vec![],
            }],
            requires_confirmation: true,
        }
    }

    #[tokio::test]
    async fn test_store_and_complete_confirmation() {
        let plan = create_test_plan();
        let plan_id = plan.id.clone();

        // Store confirmation
        let receiver = store_pending_confirmation(plan_id.clone(), plan);

        // Complete it
        assert!(complete_pending_confirmation(&plan_id, UserDecision::Confirmed));

        // Should receive the decision
        let decision = receiver.await.unwrap();
        assert_eq!(decision, UserDecision::Confirmed);
    }

    #[tokio::test]
    async fn test_complete_cancelled() {
        let plan = create_test_plan();
        let plan_id = plan.id.clone();

        let receiver = store_pending_confirmation(plan_id.clone(), plan);

        assert!(complete_pending_confirmation(&plan_id, UserDecision::Cancelled));

        let decision = receiver.await.unwrap();
        assert_eq!(decision, UserDecision::Cancelled);
    }

    #[test]
    fn test_complete_not_found() {
        assert!(!complete_pending_confirmation("nonexistent", UserDecision::Confirmed));
    }

    #[test]
    fn test_cancel_all() {
        let plan1 = create_test_plan();
        let mut plan2 = create_test_plan();
        plan2.id = "test_plan_2".to_string();

        let _recv1 = store_pending_confirmation(plan1.id.clone(), plan1);
        let _recv2 = store_pending_confirmation(plan2.id.clone(), plan2);

        assert_eq!(pending_confirmation_count(), 2);

        cancel_all_pending_confirmations();

        assert_eq!(pending_confirmation_count(), 0);
    }
}
