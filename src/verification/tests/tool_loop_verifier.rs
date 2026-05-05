//! `ToolLoopVerifier` threshold + thinking-text guard semantics.

use tokio_util::sync::CancellationToken;

use crate::verification::turn_verifier::{
    ToolCallSummary, TurnVerifier, TurnVerifyContext, VerifierVerdict,
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
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn threshold_minimum_is_two() {
    // with_threshold(0) and with_threshold(1) should clamp up to 2 to
    // avoid pathological "single call ⇒ instant veto" behavior.
    let v = ToolLoopVerifier::new().with_threshold(0);
    assert_eq!(v.repeat_threshold, 2);
    let v = ToolLoopVerifier::new().with_threshold(1);
    assert_eq!(v.repeat_threshold, 2);
}
