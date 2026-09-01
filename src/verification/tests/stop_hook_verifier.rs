//! `StopHookVerifier` adapter behaviour: stop_reason guard +
//! pre-6a parity (block exit-2 → Veto).

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::verification::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};
use crate::verification::turn_verifier::{TurnVerifier, TurnVerifyContext, VerifierVerdict};
use crate::verification::StopHookVerifier;

struct InProcessHook {
    block_reason: Option<String>,
    halt_reason: Option<String>,
}
#[async_trait]
impl StopHookHandler for InProcessHook {
    fn name(&self) -> &str {
        "in_process"
    }
    async fn evaluate(
        &self,
        _ctx: &StopHookContext,
        _cancel: &CancellationToken,
    ) -> StopHookVerdict {
        if let Some(reason) = &self.halt_reason {
            return StopHookVerdict::Halt {
                reason: reason.clone(),
            };
        }
        match &self.block_reason {
            Some(reason) => StopHookVerdict::Block {
                reason: reason.clone(),
            },
            None => StopHookVerdict::Allow,
        }
    }
}

fn build(block_reason: Option<&str>) -> StopHookVerifier {
    let hooks: Arc<Vec<Arc<dyn StopHookHandler>>> = Arc::new(vec![Arc::new(InProcessHook {
        block_reason: block_reason.map(|s| s.to_string()),
        halt_reason: None,
    })]);
    StopHookVerifier::new(hooks)
}

/// Build a verifier with one Block hook and one Halt hook. The order in the
/// vector is preserved by the aggregator; the verifier must check halt_reason
/// before blocking_reason so the Halt hook always surfaces even when it
/// appears after a Block hook in the same aggregate.
fn build_with_block_and_halt(block_reason: &str, halt_reason: &str) -> StopHookVerifier {
    let hooks: Arc<Vec<Arc<dyn StopHookHandler>>> = Arc::new(vec![
        Arc::new(InProcessHook {
            block_reason: Some(block_reason.to_string()),
            halt_reason: None,
        }),
        Arc::new(InProcessHook {
            block_reason: None,
            halt_reason: Some(halt_reason.to_string()),
        }),
    ]);
    StopHookVerifier::new(hooks)
}

#[tokio::test]
async fn skips_when_stop_reason_is_none() {
    // Mid-turn (stop_reason: None) — verifier should short-circuit
    // even if the hook would have blocked, because it's not the right
    // moment to consult stop hooks.
    let verifier = build(Some("would block"));
    let ctx = TurnVerifyContext {
        iterations: 0,
        tool_calls_made: 0,
        final_text: None,
        recent_tool_calls: &[],
        stop_reason: None,
        session_id: None,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
    };
    let cancel = CancellationToken::new();
    assert!(verifier.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn fires_when_stop_reason_is_some_and_hook_blocks() {
    let verifier = build(Some("tests not passing"));
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 3,
        final_text: Some("done"),
        recent_tool_calls: &[],
        stop_reason: Some("end_turn"),
        session_id: None,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
    };
    let cancel = CancellationToken::new();
    match verifier.verify(&ctx, &cancel).await {
        VerifierVerdict::Veto { reason, .. } => {
            assert_eq!(reason, "tests not passing");
        }
        other => panic!("expected Veto with hook reason, got {other:?}"),
    }
}

#[tokio::test]
async fn allows_when_hook_passes() {
    let verifier = build(None);
    let ctx = TurnVerifyContext {
        iterations: 1,
        tool_calls_made: 0,
        final_text: Some("done"),
        recent_tool_calls: &[],
        stop_reason: Some("end_turn"),
        session_id: None,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
    };
    let cancel = CancellationToken::new();
    assert!(verifier.verify(&ctx, &cancel).await.is_continue());
}

/// Halt must outrank Block: when both fire in the same aggregate (e.g. one
/// hook exits 3 and another exits 2), the verifier must surface `Halt` —
/// not `Veto` — so the loop exits instead of retrying. Regression guard for
/// the priority check at `stop_hook_verifier.rs:71-84`.
#[tokio::test]
async fn halt_outranks_block_in_the_same_aggregate() {
    let verifier = build_with_block_and_halt("tests not passing", "fatal: stop now");
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 3,
        final_text: Some("done"),
        recent_tool_calls: &[],
        stop_reason: Some("end_turn"),
        session_id: None,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
    };
    let cancel = CancellationToken::new();
    match verifier.verify(&ctx, &cancel).await {
        VerifierVerdict::Halt { reason } => {
            assert_eq!(reason, "fatal: stop now");
        }
        other => panic!("expected Halt with halt-hook reason, got {other:?}"),
    }
}
