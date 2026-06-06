//! `ToolLoopVerifier` threshold + identical-call repetition semantics.

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
async fn thinking_text_does_not_rescue_identical_loop() {
    // Narration is now ENCOURAGED (guidelines rule 17), so it can no longer be
    // the signal that suppresses death-loop detection. Five identical
    // (name, args_hash) calls is a loop whether or not the model narrates —
    // the args_hash equality already excludes legitimate varied exploration.
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
    assert!(v.verify(&ctx, &cancel).await.is_veto());
}

#[tokio::test]
async fn text_present_still_vetoes_identical_loop() {
    // After removing the has_text escape, the presence of *any* final_text
    // (whitespace or substantive) does not change the verdict for an identical
    // (name, args_hash) run — it vetoes on repetition alone.
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

#[tokio::test]
async fn between_thresholds_still_vetoes_not_halts() {
    // Default new(): veto at 5, halt at 8. Six identical calls → past the veto
    // tier but short of the halt tier → still a (recoverable) Veto.
    let v = ToolLoopVerifier::new();
    let history = vec![make("read", 1); 6];
    let ctx = TurnVerifyContext {
        iterations: 6,
        tool_calls_made: 6,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_veto());
}

#[tokio::test]
async fn at_halt_threshold_halts() {
    // Eight identical calls (== TOOL_HISTORY_WINDOW, the default halt tier) →
    // the model has ignored several vetoes; cut the loop off with a Halt before
    // it runs into the provider rate limit.
    let v = ToolLoopVerifier::new();
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
    match v.verify(&ctx, &cancel).await {
        VerifierVerdict::Halt { reason, .. } => {
            assert!(reason.contains("read"));
            assert!(reason.contains("unproductive loop"));
        }
        other => panic!("expected Halt, got {other:?}"),
    }
}

#[tokio::test]
async fn tier2_same_name_varying_args_no_text_halts() {
    // A full window of the SAME tool name with all-different args (so Tier 1's
    // identical run never reaches the threshold) and no narration text → the
    // exploration-loop Tier 2 halts. This is the template.html/layouts.md/
    // themes.md thrash the identical-args check is blind to.
    let v = ToolLoopVerifier::new();
    let history: Vec<_> = (0..TOOL_HISTORY_WINDOW as u64)
        .map(|i| make("file_read", i)) // same name, every args_hash distinct
        .collect();
    let ctx = TurnVerifyContext {
        iterations: TOOL_HISTORY_WINDOW,
        tool_calls_made: TOOL_HISTORY_WINDOW,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    match v.verify(&ctx, &cancel).await {
        VerifierVerdict::Halt { reason, .. } => {
            assert!(reason.contains("file_read"));
            assert!(reason.contains("varying arguments"));
        }
        other => panic!("expected Tier-2 Halt, got {other:?}"),
    }
}

#[tokio::test]
async fn tier2_narration_rescues_varying_args_loop() {
    // The same full-window varying-args run, but WITH narration text. Tier 2 is
    // silence-gated (varying-args exploration with reasoning can be legitimate,
    // and the Tier-2 Halt is terminal), so this must NOT halt.
    let v = ToolLoopVerifier::new();
    let history: Vec<_> = (0..TOOL_HISTORY_WINDOW as u64)
        .map(|i| make("file_read", i))
        .collect();
    let ctx = TurnVerifyContext {
        iterations: TOOL_HISTORY_WINDOW,
        tool_calls_made: TOOL_HISTORY_WINDOW,
        final_text: Some("comparing the three layout references before composing"),
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn tier2_below_window_continues() {
    // One short of a full window of same-name varying-args calls, no text →
    // Tier 2 requires the entire window, so this still continues.
    let v = ToolLoopVerifier::new();
    let history: Vec<_> = (0..(TOOL_HISTORY_WINDOW as u64 - 1))
        .map(|i| make("file_read", i))
        .collect();
    let ctx = TurnVerifyContext {
        iterations: TOOL_HISTORY_WINDOW - 1,
        tool_calls_made: TOOL_HISTORY_WINDOW - 1,
        final_text: None,
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_continue());
}

#[tokio::test]
async fn halt_threshold_clamped_at_or_above_repeat() {
    // A halt threshold below the veto threshold would invert the tiers; it is
    // lifted to `repeat_threshold`.
    let v = ToolLoopVerifier::new()
        .with_threshold(5)
        .with_halt_threshold(2);
    assert_eq!(v.halt_threshold(), 5);
    // And it can never exceed the ring-buffer window (else it could never fire).
    let v = ToolLoopVerifier::new().with_halt_threshold(TOOL_HISTORY_WINDOW + 50);
    assert_eq!(v.halt_threshold(), TOOL_HISTORY_WINDOW);
}
