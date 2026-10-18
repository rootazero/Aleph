//! Re-export shim — error classification lives in `aleph-slot-api`.
//!
//! As of P0 Task 2, the pure error classifier — `ToolErrorKind`,
//! `classify_error_str`, `classify_tool_error`, `kind_from_head`,
//! `has_status`, `contains_word`, `is_word_char`, and the
//! `CROSS_BATCH_REFUSED_CAUSE` constant — moved to
//! `aleph_slot_api::error_kind`. This file re-exports them so every
//! existing `crate::tools::error_kind::*` path keeps compiling.
//!
//! ## Single definition of `CROSS_BATCH_REFUSED_CAUSE`
//!
//! `aleph_slot_api::error_kind::CROSS_BATCH_REFUSED_CAUSE` is the
//! only definition in the crate graph. The pre-migration Core path
//! in `src/thinker/nudges.rs` re-exports it for backwards
//! compat.

pub use aleph_slot_api::error_kind::*;

// =============================================================================
// Core-coupled tests — these exercise `crate::tools::fallback_registry`,
// which is Core runtime machinery and cannot move to the API crate. They
// keep their original names and bodies verbatim from the pre-migration
// `src/tools/error_kind.rs`.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Pressing stop must not ban the call that was stopped.
    ///
    /// The harness's cross-batch failure memo enters a call whose error is not
    /// retryable (`agent/act.rs`: "Only a NON-retryable failure enters the
    /// cross-batch memo"), and an identical re-issue is then refused for the
    /// rest of the run. A cancellation used to arrive as
    /// `ToolError::Execution { cause: "tool X cancelled" }`, so one `/stop`
    /// permanently banned that exact call — and the model was handed a
    /// persistence hint telling it to climb the tool ladder, about a failure
    /// that never happened. Aleph already made this argument for
    /// `ApprovalExpired` ("nobody said no — nobody said anything"); a
    /// cancellation says even less about the call.
    #[test]
    fn a_cancelled_call_is_not_a_verdict_on_the_call() {
        use crate::tools::service::ToolError;

        let cancelled = ToolError::Cancelled {
            name: "bash".into(),
        };
        assert!(
            cancelled.is_retryable(),
            "the cross-batch memo keys off !is_retryable — a cancelled call must \
             not be banned for the rest of the run"
        );
        assert_eq!(cancelled.kind(), ToolErrorKind::Cancelled);
        assert!(cancelled.kind().is_transient());
        assert_eq!(
            crate::tools::fallback_registry::render_persistence_hint(&cancelled, "bash"),
            "",
            "a stopped call is not a rung of any ladder"
        );
        // …while a genuine execution failure still is.
        let failed = ToolError::Execution {
            name: "bash".into(),
            cause: "exit 1".into(),
        };
        assert!(!failed.is_retryable());
        assert!(
            !crate::tools::fallback_registry::render_persistence_hint(&failed, "bash").is_empty()
        );
    }
}
