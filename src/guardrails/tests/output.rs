//! Stage 5a — Output guardrail integration tests. The harness-level callsite
//! integration test lives in `src/harness/tests/` once Task 4 lands; this file
//! covers the registry surface for output traits.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::ErrorClass;
use crate::guardrails::decision::GuardrailDecision;
use crate::guardrails::registry::GuardrailRegistry;
use crate::guardrails::traits::OutputGuardrail;

struct BlockOnLeak;
#[async_trait]
impl OutputGuardrail for BlockOnLeak {
    fn name(&self) -> &str {
        "block_on_leak"
    }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        if text.contains("leak") {
            GuardrailDecision::Block {
                reason: "leak detected".into(),
                class: ErrorClass::Fixable,
            }
        } else {
            GuardrailDecision::Allow
        }
    }
}

#[tokio::test]
async fn output_guardrail_blocks_on_leak() {
    let r = GuardrailRegistry::builder()
        .with_output(Arc::new(BlockOnLeak))
        .build();
    let d = r.evaluate_output("here is a leak").await;
    assert!(d.is_block());
    if let GuardrailDecision::Block { class, .. } = d {
        assert_eq!(class, ErrorClass::Fixable);
    }
}

#[tokio::test]
async fn output_guardrail_passes_clean_text() {
    let r = GuardrailRegistry::builder()
        .with_output(Arc::new(BlockOnLeak))
        .build();
    assert!(r.evaluate_output("clean response").await.is_allow());
}
