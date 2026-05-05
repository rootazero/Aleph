//! Stage 5a — Input guardrail integration tests are wired in Task 4
//! once the harness callsite lands. This file currently only exercises
//! the registry surface for input traits.

use std::sync::Arc;

use async_trait::async_trait;

use crate::guardrails::decision::{GuardrailDecision, Replacement};
use crate::guardrails::registry::GuardrailRegistry;
use crate::guardrails::traits::InputGuardrail;

struct SanitizeOnSecret;
#[async_trait]
impl InputGuardrail for SanitizeOnSecret {
    fn name(&self) -> &str {
        "sanitize_on_secret"
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        if text.contains("SECRET") {
            GuardrailDecision::Sanitize(Replacement {
                text: text.replace("SECRET", "[REDACTED]"),
                source: "test_sanitize".into(),
            })
        } else {
            GuardrailDecision::Allow
        }
    }
}

#[tokio::test]
async fn input_guardrail_sanitizes_secret() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(SanitizeOnSecret))
        .build();
    let d = r.evaluate_input("the password is SECRET, ok?").await;
    match d {
        GuardrailDecision::Sanitize(rep) => {
            assert!(rep.text.contains("[REDACTED]"));
            assert!(!rep.text.contains("SECRET"));
        }
        other => panic!("expected Sanitize, got {other:?}"),
    }
}

#[tokio::test]
async fn input_guardrail_passes_clean_text() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(SanitizeOnSecret))
        .build();
    assert!(r.evaluate_input("benign text").await.is_allow());
}
