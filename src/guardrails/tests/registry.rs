//! `GuardrailRegistry` semantics + `disable_all` kill-switch.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ErrorClass;
use crate::guardrails::decision::GuardrailDecision;
use crate::guardrails::registry::GuardrailRegistry;
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};

struct AllowAll;
#[async_trait]
impl InputGuardrail for AllowAll {
    fn name(&self) -> &str {
        "allow_all"
    }
    async fn evaluate_input(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Allow
    }
}
#[async_trait]
impl OutputGuardrail for AllowAll {
    fn name(&self) -> &str {
        "allow_all"
    }
    async fn evaluate_output(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Allow
    }
}
#[async_trait]
impl ToolCallGuardrail for AllowAll {
    fn name(&self) -> &str {
        "allow_all"
    }
    async fn evaluate_tool_call(&self, _name: &str, _args: &Value) -> GuardrailDecision {
        GuardrailDecision::Allow
    }
}

struct AlwaysBlock;
#[async_trait]
impl InputGuardrail for AlwaysBlock {
    fn name(&self) -> &str {
        "always_block"
    }
    async fn evaluate_input(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Block {
            reason: "blocked".into(),
            class: ErrorClass::Fixable,
        }
    }
}
#[async_trait]
impl OutputGuardrail for AlwaysBlock {
    fn name(&self) -> &str {
        "always_block"
    }
    async fn evaluate_output(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Block {
            reason: "blocked".into(),
            class: ErrorClass::Fixable,
        }
    }
}
#[async_trait]
impl ToolCallGuardrail for AlwaysBlock {
    fn name(&self) -> &str {
        "always_block"
    }
    async fn evaluate_tool_call(&self, _name: &str, _args: &Value) -> GuardrailDecision {
        GuardrailDecision::Block {
            reason: "blocked".into(),
            class: ErrorClass::Fixable,
        }
    }
}

#[tokio::test]
async fn empty_registry_allows_everything() {
    let r = GuardrailRegistry::empty();
    assert!(r.evaluate_input("anything").await.is_allow());
    assert!(r.evaluate_output("anything").await.is_allow());
    assert!(r
        .evaluate_tool_call("t", &serde_json::json!({}))
        .await
        .is_allow());
}

#[tokio::test]
async fn block_guardrail_blocks_all_three_surfaces() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AlwaysBlock))
        .with_output(Arc::new(AlwaysBlock))
        .with_tool_call(Arc::new(AlwaysBlock))
        .build();
    assert!(r.evaluate_input("x").await.is_block());
    assert!(r.evaluate_output("x").await.is_block());
    assert!(r
        .evaluate_tool_call("t", &serde_json::json!({}))
        .await
        .is_block());
}

#[tokio::test]
async fn disable_all_short_circuits_to_allow() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AlwaysBlock))
        .with_output(Arc::new(AlwaysBlock))
        .with_tool_call(Arc::new(AlwaysBlock))
        .build();
    r.disable_all();
    assert!(!r.is_enabled());
    assert!(r.evaluate_input("x").await.is_allow());
    assert!(r.evaluate_output("x").await.is_allow());
    assert!(r
        .evaluate_tool_call("t", &serde_json::json!({}))
        .await
        .is_allow());

    r.enable_all();
    assert!(r.is_enabled());
    assert!(r.evaluate_input("x").await.is_block());
}

#[tokio::test]
async fn first_non_allow_wins_when_multiple_registered() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AllowAll))
        .with_input(Arc::new(AlwaysBlock))
        .build();
    let d = r.evaluate_input("x").await;
    assert!(d.is_block());
}

#[tokio::test]
async fn counts_match_registered_guardrails() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AllowAll))
        .with_input(Arc::new(AlwaysBlock))
        .with_output(Arc::new(AllowAll))
        .with_tool_call(Arc::new(AllowAll))
        .with_tool_call(Arc::new(AllowAll))
        .with_tool_call(Arc::new(AllowAll))
        .build();
    assert_eq!(r.input_count(), 2);
    assert_eq!(r.output_count(), 1);
    assert_eq!(r.tool_call_count(), 3);
}

/// Concurrency hammer: spawn N tasks each calling `evaluate_tool_call` in a
/// loop while a sibling task flips `disable_all` / `enable_all`. The contract
/// being tested is the sequential consistency of the AtomicBool kill-switch:
/// every individual call must observe a coherent on/off state and produce
/// either an `Allow` (registry off OR no guardrails fired) or a `Block` (an
/// `AlwaysBlock` guardrail fired while registry was on). No call should
/// panic, deadlock, or return an inconsistent decision.
///
/// This stands in for a `loom` model test — the repository's loom feature
/// exists in `sync_primitives` but no `cfg(loom)` test infrastructure is
/// wired into `cargo test` yet. A tokio hammer with high task count gives
/// good practical coverage for a single-AtomicBool flag at minimal cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_evaluate_vs_disable_all_is_consistent() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let registry = Arc::new(
        GuardrailRegistry::builder()
            .with_tool_call(Arc::new(AlwaysBlock))
            .build(),
    );

    const READERS: usize = 16;
    const ITERS: usize = 200;
    let blocks = Arc::new(AtomicUsize::new(0));
    let allows = Arc::new(AtomicUsize::new(0));

    let toggler = {
        let registry = registry.clone();
        tokio::spawn(async move {
            for i in 0..(READERS * ITERS) {
                if i % 2 == 0 {
                    registry.disable_all();
                } else {
                    registry.enable_all();
                }
                // Yield so readers actually get scheduled between toggles —
                // without this the toggler can run to completion in one slot
                // and the test degenerates into sequential execution.
                if i % 8 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        })
    };

    let mut readers = Vec::with_capacity(READERS);
    for _ in 0..READERS {
        let registry = registry.clone();
        let blocks = blocks.clone();
        let allows = allows.clone();
        readers.push(tokio::spawn(async move {
            for _ in 0..ITERS {
                let d = registry
                    .evaluate_tool_call("any", &serde_json::json!({"x": 1}))
                    .await;
                match d {
                    GuardrailDecision::Block { .. } => {
                        blocks.fetch_add(1, Ordering::Relaxed);
                    }
                    GuardrailDecision::Allow => {
                        allows.fetch_add(1, Ordering::Relaxed);
                    }
                    other => panic!("unexpected decision under concurrency: {other:?}"),
                }
            }
        }));
    }

    for r in readers {
        r.await.expect("reader task");
    }
    toggler.await.expect("toggler task");

    let total = blocks.load(Ordering::Relaxed) + allows.load(Ordering::Relaxed);
    assert_eq!(total, READERS * ITERS, "every call must produce a decision");
    // The contract being tested is sequential consistency of the
    // AtomicBool — every call returned a coherent decision and no task
    // panicked or deadlocked. We do NOT assert that both arms fired
    // because the scheduler may run the toggler to completion before any
    // readers schedule (especially on a busy CI runner).
}
