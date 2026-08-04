//! `VerifierChain` semantics: empty / first-veto-wins.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::verification::turn_verifier::{
    hash_tool_args, TurnVerifier, TurnVerifyContext, VerifierChain, VerifierVerdict,
};

struct AlwaysContinue;
#[async_trait]
impl TurnVerifier for AlwaysContinue {
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
    async fn verify(
        &self,
        _ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        VerifierVerdict::Veto {
            reason: self.0.to_string(),
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
        VerifierVerdict::Veto { reason } => assert_eq!(reason, "first"),
        other => panic!("expected first veto, got {other:?}"),
    }
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
