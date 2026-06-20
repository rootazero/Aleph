//! `VerifierChain` semantics: empty / first-veto-wins / kill-switch /
//! concurrency hammer.

use crate::sync_primitives::{Arc, AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::ErrorClass;
use crate::verification::turn_verifier::{
    hash_tool_args, TurnVerifier, TurnVerifyContext, VerifierChain, VerifierVerdict,
};

struct AlwaysContinue;
#[async_trait]
impl TurnVerifier for AlwaysContinue {
    fn name(&self) -> &str {
        "continue"
    }
    async fn verify(
        &self,
        _ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        VerifierVerdict::Continue
    }
}

struct AlwaysVeto(&'static str);
#[async_trait]
impl TurnVerifier for AlwaysVeto {
    fn name(&self) -> &str {
        self.0
    }
    async fn verify(
        &self,
        _ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        VerifierVerdict::Veto {
            reason: self.0.to_string(),
            class: ErrorClass::Recoverable,
        }
    }
}

fn ctx() -> TurnVerifyContext<'static> {
    TurnVerifyContext {
        iterations: 0,
        tool_calls_made: 0,
        final_text: None,
        recent_tool_calls: &[],
        stop_reason: None,
        session_id: None,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
    }
}

#[tokio::test]
async fn empty_chain_returns_continue() {
    let chain = VerifierChain::empty();
    let cancel = CancellationToken::new();
    assert!(chain.verify(&ctx(), &cancel).await.is_continue());
}

#[tokio::test]
async fn first_veto_short_circuits_subsequent_verifiers() {
    let chain = VerifierChain::builder()
        .with(Arc::new(AlwaysContinue))
        .with(Arc::new(AlwaysVeto("first")))
        .with(Arc::new(AlwaysVeto("second")))
        .build();
    let cancel = CancellationToken::new();
    let verdict = chain.verify(&ctx(), &cancel).await;
    match verdict {
        VerifierVerdict::Veto { reason, .. } => assert_eq!(reason, "first"),
        other => panic!("expected first veto, got {other:?}"),
    }
}

#[tokio::test]
async fn disable_all_short_circuits_to_continue() {
    let chain = VerifierChain::builder()
        .with(Arc::new(AlwaysVeto("would_block")))
        .build();
    let cancel = CancellationToken::new();
    chain.disable_all();
    assert!(!chain.is_enabled());
    assert!(chain.verify(&ctx(), &cancel).await.is_continue());

    chain.enable_all();
    assert!(chain.is_enabled());
    assert!(chain.verify(&ctx(), &cancel).await.is_veto());
}

/// Concurrency hammer: spawn N reader tasks calling `verify` against a
/// chain whose `disable_all` / `enable_all` gets toggled by a sibling
/// task. Sequential consistency of the AtomicBool kill-switch means
/// every individual call must produce a coherent verdict (Continue when
/// disabled, Veto when enabled). No call may panic or hang. Sister of
/// the Stage 5b guardrails registry hammer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_verify_vs_disable_all_is_consistent() {
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(AlwaysVeto("any")))
            .build(),
    );
    const READERS: usize = 16;
    const ITERS: usize = 200;
    let blocks = Arc::new(AtomicUsize::new(0));
    let allows = Arc::new(AtomicUsize::new(0));

    let toggler = {
        let chain = chain.clone();
        tokio::spawn(async move {
            for i in 0..(READERS * ITERS) {
                if i % 2 == 0 {
                    chain.disable_all();
                } else {
                    chain.enable_all();
                }
                if i % 8 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        })
    };

    let mut readers = Vec::with_capacity(READERS);
    for _ in 0..READERS {
        let chain = chain.clone();
        let blocks = blocks.clone();
        let allows = allows.clone();
        readers.push(tokio::spawn(async move {
            let cancel = CancellationToken::new();
            let c = TurnVerifyContext {
                iterations: 0,
                tool_calls_made: 0,
                final_text: None,
                recent_tool_calls: &[],
                stop_reason: None,
                session_id: None,
                robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
            };
            for _ in 0..ITERS {
                match chain.verify(&c, &cancel).await {
                    VerifierVerdict::Continue => {
                        allows.fetch_add(1, Ordering::Relaxed);
                    }
                    VerifierVerdict::Veto { .. } | VerifierVerdict::Halt { .. } => {
                        blocks.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    for r in readers {
        r.await.expect("reader task");
    }
    toggler.await.expect("toggler task");

    let total = blocks.load(Ordering::Relaxed) + allows.load(Ordering::Relaxed);
    assert_eq!(total, READERS * ITERS);
}

#[test]
fn hash_tool_args_is_deterministic() {
    let args = serde_json::json!({"path": "/tmp/test", "limit": 10});
    let h1 = hash_tool_args(&args);
    let h2 = hash_tool_args(&args);
    assert_eq!(h1, h2);
}

#[test]
fn hash_tool_args_different_inputs_produce_different_hashes() {
    let a = serde_json::json!({"a": 1});
    let b = serde_json::json!({"a": 2});
    assert_ne!(hash_tool_args(&a), hash_tool_args(&b));
}
