//! `GuardrailRegistry` — aggregates all three guardrail surfaces behind
//! a single `Arc`-shareable handle. Constructed once at startup, held by
//! `HarnessDeps` as `Option<Arc<GuardrailRegistry>>`.
//!
//! Sequential evaluation per surface: stops at the first non-`Allow`
//! decision. `disable_all()` flips an `AtomicBool` so every evaluation
//! short-circuits to `Allow` — the high-risk runtime rollback knob from
//! master spec § Stage 5.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;

use crate::guardrails::decision::GuardrailDecision;
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};

pub struct GuardrailRegistry {
    input: Vec<Arc<dyn InputGuardrail>>,
    output: Vec<Arc<dyn OutputGuardrail>>,
    tool_call: Vec<Arc<dyn ToolCallGuardrail>>,
    enabled: AtomicBool,
}

impl GuardrailRegistry {
    pub fn builder() -> GuardrailRegistryBuilder {
        GuardrailRegistryBuilder::default()
    }

    pub fn empty() -> Self {
        Self {
            input: Vec::new(),
            output: Vec::new(),
            tool_call: Vec::new(),
            enabled: AtomicBool::new(true),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Runtime kill-switch — flips `enabled` to false. All three `evaluate_*`
    /// methods short-circuit to `Allow` until `enable_all()` is called.
    pub fn disable_all(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    pub fn enable_all(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub fn input_count(&self) -> usize {
        self.input.len()
    }
    pub fn output_count(&self) -> usize {
        self.output.len()
    }
    pub fn tool_call_count(&self) -> usize {
        self.tool_call.len()
    }

    pub async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        if !self.is_enabled() || self.input.is_empty() {
            return GuardrailDecision::Allow;
        }
        for g in &self.input {
            let d = g.evaluate_input(text).await;
            if !d.is_allow() {
                return d;
            }
        }
        GuardrailDecision::Allow
    }

    pub async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        if !self.is_enabled() || self.output.is_empty() {
            return GuardrailDecision::Allow;
        }
        for g in &self.output {
            let d = g.evaluate_output(text).await;
            if !d.is_allow() {
                return d;
            }
        }
        GuardrailDecision::Allow
    }

    pub async fn evaluate_tool_call(&self, tool_name: &str, args: &Value) -> GuardrailDecision {
        if !self.is_enabled() || self.tool_call.is_empty() {
            return GuardrailDecision::Allow;
        }
        for g in &self.tool_call {
            let d = g.evaluate_tool_call(tool_name, args).await;
            if !d.is_allow() {
                return d;
            }
        }
        GuardrailDecision::Allow
    }
}

#[derive(Default)]
pub struct GuardrailRegistryBuilder {
    input: Vec<Arc<dyn InputGuardrail>>,
    output: Vec<Arc<dyn OutputGuardrail>>,
    tool_call: Vec<Arc<dyn ToolCallGuardrail>>,
}

impl GuardrailRegistryBuilder {
    pub fn with_input(mut self, g: Arc<dyn InputGuardrail>) -> Self {
        self.input.push(g);
        self
    }
    pub fn with_output(mut self, g: Arc<dyn OutputGuardrail>) -> Self {
        self.output.push(g);
        self
    }
    pub fn with_tool_call(mut self, g: Arc<dyn ToolCallGuardrail>) -> Self {
        self.tool_call.push(g);
        self
    }
    pub fn build(self) -> GuardrailRegistry {
        GuardrailRegistry {
            input: self.input,
            output: self.output,
            tool_call: self.tool_call,
            enabled: AtomicBool::new(true),
        }
    }
}

#[cfg(test)]
fn _assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<GuardrailRegistry>();
    check::<Arc<GuardrailRegistry>>();
}
