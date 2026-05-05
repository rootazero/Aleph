//! ContextRuleLayer — rewrite/deny by context.
//!
//! Phase 2 Task 6: relocate ContextRule evaluation into the middleware layer.
//!
//! Note: at the time of this change no concrete `ContextRule` type existed in
//! the codebase (only `ContextRuleConfig` in smart_flow, which governs LLM
//! agent matching rather than tool dispatch). To establish the layer surface
//! for future rules, this file defines a placeholder `ContextRule` trait and
//! action enum locally. The layer holds an `Arc<ArcSwap<Vec<Arc<dyn ContextRule>>>>`
//! snapshot and evaluates each rule in order. The production rule set is empty
//! today; rules can be appended via `set_rules` once a real policy exists.

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

/// Action a rule takes on an incoming `(name, input)` pair.
#[derive(Debug, Clone)]
pub enum ContextRuleAction {
    /// Allow the call through unchanged.
    Allow,
    /// Short-circuit with `ToolError::PermissionDenied`. `reason` is surfaced to the caller.
    Deny(String),
    /// Forward to the inner service with `new_input` in place of the original.
    Rewrite(Value),
}

/// A policy rule evaluated during tool dispatch.
///
/// Rules see the tool name and the current input value and return an action.
/// Evaluation order is the order of the `Vec` snapshot — the first non-`Allow`
/// action wins.
pub trait ContextRule: Send + Sync {
    fn evaluate(&self, name: &str, input: &Value) -> ContextRuleAction;
}

pub struct ContextRuleLayer {
    inner: Arc<dyn ToolService>,
    rules: Arc<ArcSwap<Vec<Arc<dyn ContextRule>>>>,
}

impl ContextRuleLayer {
    /// Construct a layer with an empty rule set. Wire rules in later via
    /// `set_rules` once a concrete policy exists.
    pub fn new(inner: Arc<dyn ToolService>) -> Self {
        Self {
            inner,
            rules: Arc::new(ArcSwap::from_pointee(Vec::new())),
        }
    }

    /// Construct a layer with an explicit rule set and ArcSwap handle.
    pub fn with_rules(
        inner: Arc<dyn ToolService>,
        rules: Arc<ArcSwap<Vec<Arc<dyn ContextRule>>>>,
    ) -> Self {
        Self { inner, rules }
    }

    /// Replace the current rule set atomically.
    pub fn set_rules(&self, rules: Vec<Arc<dyn ContextRule>>) {
        self.rules.store(Arc::new(rules));
    }

    /// Evaluate all rules against `(name, input)` and return the first
    /// non-`Allow` action. If every rule allows, returns `Allow`.
    fn evaluate(&self, name: &str, input: &Value) -> ContextRuleAction {
        let snapshot = self.rules.load();
        for rule in snapshot.iter() {
            match rule.evaluate(name, input) {
                ContextRuleAction::Allow => continue,
                other => return other,
            }
        }
        ContextRuleAction::Allow
    }
}

#[async_trait]
impl ToolService for ContextRuleLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        match self.evaluate(name, &input) {
            ContextRuleAction::Allow => self.inner.execute(name, input).await,
            ContextRuleAction::Deny(reason) => Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason,
            }),
            ContextRuleAction::Rewrite(new_input) => self.inner.execute(name, new_input).await,
        }
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner.list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.inner.describe(name).await
    }

    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
        self.inner.dispatcher_schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;

    use crate::session::events::{ToolOutput, ToolOutputMetadata};

    /// Inner service that records the last `(name, input)` it saw.
    struct RecordingInner {
        last: Mutex<Option<(String, Value)>>,
    }

    impl RecordingInner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last: Mutex::new(None),
            })
        }

        fn last(&self) -> Option<(String, Value)> {
            self.last.lock().expect("mutex poisoned").clone()
        }
    }

    #[async_trait]
    impl ToolService for RecordingInner {
        async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
            *self.last.lock().expect("mutex poisoned") = Some((name.to_string(), input.clone()));
            Ok(ToolOutput {
                value: serde_json::json!({"ran": name}),
                metadata: ToolOutputMetadata::default(),
            })
        }

        async fn list(&self) -> Vec<ToolDefinition> {
            Vec::new()
        }

        async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
            None
        }
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    struct ConstantRule(ContextRuleAction);

    impl ContextRule for ConstantRule {
        fn evaluate(&self, _name: &str, _input: &Value) -> ContextRuleAction {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn empty_rules_allow_through() {
        let inner = RecordingInner::new();
        let layer = ContextRuleLayer::new(inner.clone());

        let input = serde_json::json!({"k": "v"});
        let out = layer
            .execute("echo", input.clone())
            .await
            .expect("allow should forward");
        assert_eq!(out.value, serde_json::json!({"ran": "echo"}));

        let (name, seen) = inner.last().expect("inner must have been called");
        assert_eq!(name, "echo");
        assert_eq!(seen, input);
    }

    #[tokio::test]
    async fn rule_deny_short_circuits() {
        let inner = RecordingInner::new();
        let layer = ContextRuleLayer::new(inner.clone());
        layer.set_rules(vec![Arc::new(ConstantRule(ContextRuleAction::Deny(
            "nope".into(),
        )))]);

        let err = layer
            .execute("echo", serde_json::json!({"k": "v"}))
            .await
            .expect_err("deny should short-circuit");

        match err {
            ToolError::PermissionDenied { name, reason } => {
                assert_eq!(name, "echo");
                assert_eq!(reason, "nope");
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert!(
            inner.last().is_none(),
            "inner must not be called when denied"
        );
    }

    #[tokio::test]
    async fn rule_rewrite_passes_new_input() {
        let inner = RecordingInner::new();
        let layer = ContextRuleLayer::new(inner.clone());

        let rewritten = serde_json::json!({"k": "rewritten"});
        layer.set_rules(vec![Arc::new(ConstantRule(ContextRuleAction::Rewrite(
            rewritten.clone(),
        )))]);

        layer
            .execute("echo", serde_json::json!({"k": "original"}))
            .await
            .expect("rewrite should forward");

        let (name, seen) = inner.last().expect("inner must have been called");
        assert_eq!(name, "echo");
        assert_eq!(seen, rewritten, "inner must see rewritten input");
    }
}
