//! `ToolLoopVerifier` threshold + thinking-text guard semantics.

use tokio_util::sync::CancellationToken;

use crate::verification::turn_verifier::{
    ToolCallSummary, TurnVerifier, TurnVerifyContext, VerifierVerdict, TOOL_HISTORY_WINDOW,
};
use crate::verification::ToolLoopVerifier;

fn make(name: &str, args_hash: u64) -> ToolCallSummary {
    ToolCallSummary {
        name: name.to_string(),
        args_hash,
    }
}

#[tokio::test]
async fn below_threshold_allows() {
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![make("read", 1); 4];
    let ctx = TurnVerifyContext {
        iterations: 4,
        tool_calls_made: 4,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn at_threshold_with_no_text_vetoes() {
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![make("read", 1); 5];
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 5,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    match v.verify(&ctx, &cancel).await {
        VerifierVerdict::Veto { reason, .. } => {
            assert!(reason.contains("read"));
            assert!(reason.contains("5"));
        }
        other => panic!("expected Veto, got {other:?}"),
    }
}

#[tokio::test]
async fn at_threshold_with_thinking_text_allows() {
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![make("read", 1); 5];
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 5,
        final_text: Some("hmm, let me reconsider"),
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn whitespace_only_text_treated_as_no_text() {
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![make("read", 1); 5];
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 5,
        final_text: Some("   \n\t  "),
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_veto());
}

#[tokio::test]
async fn different_args_hash_breaks_repetition() {
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![
        make("read", 1),
        make("read", 1),
        make("read", 2), // different args — sequence broken
        make("read", 1),
        make("read", 1),
    ];
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 5,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn threshold_minimum_is_two() {
    // with_threshold(0) and with_threshold(1) should clamp up to 2 to
    // avoid pathological "single call ⇒ instant veto" behavior.
    let v = ToolLoopVerifier::new().with_threshold(0);
    assert_eq!(v.threshold(), 2);
    let v = ToolLoopVerifier::new().with_threshold(1);
    assert_eq!(v.threshold(), 2);
}

#[tokio::test]
async fn stop_turn_never_vetoes_on_stale_history() {
    // A stop turn (`stop_reason.is_some()`) with a buffer full of identical
    // calls and empty answer text (e.g. a thinking-only finish) must NOT veto:
    // the death loop is a mid-turn concern and re-judging stale history here
    // would flip a clean Done into a Continue.
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![make("read", 1); 5];
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 5,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: Some("end_turn"),
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn threshold_clamped_to_history_window() {
    // A threshold above the ring-buffer capacity could never be satisfied
    // (`recent_tool_calls.len()` is bounded by the window), silently disabling
    // detection. `with_threshold` clamps to the window so it can still fire.
    let v = ToolLoopVerifier::new().with_threshold(TOOL_HISTORY_WINDOW + 100);
    assert_eq!(v.threshold(), TOOL_HISTORY_WINDOW);

    let history = vec![make("read", 1); TOOL_HISTORY_WINDOW];
    let ctx = TurnVerifyContext {
        iterations: TOOL_HISTORY_WINDOW,
        tool_calls_made: TOOL_HISTORY_WINDOW,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_veto());
}

#[tokio::test]
async fn threshold_two_vetoes_at_exactly_two() {
    let v = ToolLoopVerifier::new().with_threshold(2);
    let history = vec![make("read", 1), make("read", 1)];
    let ctx = TurnVerifyContext {
        iterations: 2,
        tool_calls_made: 2,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_veto());
}
