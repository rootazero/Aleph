//! DAG Plan Confirmation FFI methods
//!
//! Contains task plan confirmation:
//! - confirm_task_plan

use crate::ffi::AetherCore;
use tracing::info;

impl AetherCore {
    // =========================================================================
    // DAG Plan Confirmation (Plan Confirmation Flow)
    // =========================================================================

    /// Confirm or cancel a pending DAG task plan
    ///
    /// This method is called by Swift after displaying a confirmation dialog
    /// to the user. It completes the pending confirmation and allows the
    /// DAG scheduler to proceed (or cancel).
    ///
    /// # Arguments
    ///
    /// * `plan_id` - The plan ID from `on_plan_confirmation_required` callback
    /// * `confirmed` - `true` to confirm execution, `false` to cancel
    ///
    /// # Returns
    ///
    /// `true` if the confirmation was found and completed, `false` if expired or not found.
    pub fn confirm_task_plan(&self, plan_id: String, confirmed: bool) -> bool {
        let decision = if confirmed {
            crate::dispatcher::UserDecision::Confirmed
        } else {
            crate::dispatcher::UserDecision::Cancelled
        };

        info!(
            plan_id = %plan_id,
            decision = ?decision,
            "Confirming task plan from FFI"
        );

        crate::ffi::plan_confirmation::complete_pending_confirmation(&plan_id, decision)
    }
}
