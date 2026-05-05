//! Real `PiiSecretsGuardrail` impl wrapping `PiiEngine` + `SecretLeakDetector`.
//!
//! Order of evaluation: secret leak detector first (must Block, never Sanitize
//! — a redacted secret is still a leak signal), then PII engine (may Sanitize).
//! Both come from existing `src/pii` and `src/secrets` modules — this file is
//! a trait adapter, not a re-implementation.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ErrorClass;
use crate::guardrails::decision::{GuardrailDecision, Replacement};
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
use crate::pii::engine::PiiEngine;
use crate::secrets::leak_detector::{LeakDecision, LeakDetector as SecretLeakDetector};
use crate::sync_primitives::RwLock;

const NAME: &str = "pii_secrets";

pub struct PiiSecretsGuardrail {
    pii: Option<Arc<RwLock<PiiEngine>>>,
    secrets: Arc<SecretLeakDetector>,
}

impl PiiSecretsGuardrail {
    pub fn new(pii: Option<Arc<RwLock<PiiEngine>>>, secrets: Arc<SecretLeakDetector>) -> Self {
        Self { pii, secrets }
    }

    /// Construct a guardrail using the global PII engine snapshot (if any) and
    /// a fresh secret leak detector. Convenience for the boot path.
    pub fn from_globals() -> Self {
        Self::new(PiiEngine::global(), Arc::new(SecretLeakDetector::new()))
    }

    fn evaluate(&self, text: &str) -> GuardrailDecision {
        match self.secrets.scan_outbound(text) {
            LeakDecision::Allow => {}
            LeakDecision::Block { reason, .. } => {
                return GuardrailDecision::Block {
                    reason,
                    class: ErrorClass::Fixable,
                };
            }
        }
        if let Some(engine) = self.pii.as_ref() {
            let guard = match engine.read() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let result = guard.filter(text);
            if result.has_detections() && result.text != text {
                let total = result.blocked_count + result.warned_count;
                return GuardrailDecision::Sanitize(Replacement {
                    text: result.text,
                    source: format!("pii ({total} detection(s))"),
                });
            }
        }
        GuardrailDecision::Allow
    }
}

#[async_trait]
impl InputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        self.evaluate(text)
    }
}

#[async_trait]
impl OutputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        self.evaluate(text)
    }
}

#[async_trait]
impl ToolCallGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_tool_call(&self, _tool_name: &str, args: &Value) -> GuardrailDecision {
        let serialized = match serde_json::to_string(args) {
            Ok(s) => s,
            Err(_) => return GuardrailDecision::Allow,
        };
        self.evaluate(&serialized)
    }
}
