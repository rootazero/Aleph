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
