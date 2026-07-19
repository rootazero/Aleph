//! `PiiSecretsGuardrail` — thin trait adapter delegating to `RuntimeSecurityGuard`.
//!
//! Maps the three guardrail surfaces (input / output / `tool_call`) onto the
//! orchestrator's two methods:
//! - `evaluate_input`  → `process_outbound(None resolver)` — user → LLM
//! - `evaluate_output` → `process_inbound`                 — LLM → user
//! - `evaluate_tool_call` → `process_outbound(Some(resolver))` — LLM → tool
//!
//! Placeholder substitution (`{{secret:NAME}}`) happens **only** at the
//! `tool_call` surface — the unique location where the next consumer is a
//! tool runtime (not the user) and plaintext is appropriate.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ErrorClass;
use crate::guardrails::decision::{GuardrailDecision, Replacement};
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
use crate::secrets::injection::AsyncSecretResolver;
use crate::security::runtime_guard::{
    GuardResult, RuntimeSecurityGuard, SecurityContext, SecurityGuardError,
};

const NAME: &str = "pii_secrets";

pub struct PiiSecretsGuardrail {
    guard: Arc<RuntimeSecurityGuard>,
    resolver: Option<Arc<dyn AsyncSecretResolver>>,
}

impl PiiSecretsGuardrail {
    /// Construct over an existing orchestrator with no resolver. Placeholder
    /// substitution at the `tool_call` surface will be inert.
    #[must_use]
    pub fn new(guard: Arc<RuntimeSecurityGuard>) -> Self {
        Self {
            guard,
            resolver: None,
        }
    }

    /// Construct over an existing orchestrator with a resolver wired in.
    #[must_use]
    pub fn with_guard_and_resolver(
        guard: Arc<RuntimeSecurityGuard>,
        resolver: Option<Arc<dyn AsyncSecretResolver>>,
    ) -> Self {
        Self { guard, resolver }
    }

    /// Construct with a default orchestrator and an optional resolver.
    /// Convenience for the boot path. Audit channel from the orchestrator
    /// is dropped here — callers that want audit drainage should construct
    /// via `with_guard_and_resolver` after spawning their own drain.
    #[must_use]
    pub fn with_resolver(resolver: Option<Arc<dyn AsyncSecretResolver>>) -> Self {
        let guard = Arc::new(RuntimeSecurityGuard::default_guard());
        Self { guard, resolver }
    }

    fn map_outbound(
        original: &str,
        result: Result<GuardResult, SecurityGuardError>,
    ) -> GuardrailDecision {
        match result {
            Ok(GuardResult::Clean { .. }) => GuardrailDecision::Allow,
            Ok(GuardResult::Warned { text, warnings }) => {
                // `Warned.text` carries security-relevant sanitisation applied
                // at warn severity: invisible/bidi-char stripping, LLM
                // tokenizer-marker scrubbing, inbound PII warn-masking, and
                // resolved placeholders. If it differs from the input we MUST
                // surface it as `Sanitize` so the caller swaps the cleaned text
                // in — mapping to `Warn` (no caller-visible mutation) would
                // discard the scrubbed text and let the original through,
                // defeating the very defense that fired.
                if text != original {
                    GuardrailDecision::Sanitize(Replacement {
                        text,
                        source: format!("pii_secrets (warn: {})", warnings.join("; ")),
                    })
                } else {
                    GuardrailDecision::Warn {
                        reason: warnings.join("; "),
                    }
                }
            }
            Ok(GuardResult::Redacted { text, reasons }) => {
                GuardrailDecision::Sanitize(Replacement {
                    text,
                    source: format!("pii_secrets ({})", reasons.join("; ")),
                })
            }
            Ok(GuardResult::Blocked { reason, .. }) => GuardrailDecision::Block {
                reason,
                class: ErrorClass::Fixable,
            },
            Err(SecurityGuardError::SecretResolutionFailed(e)) => GuardrailDecision::Block {
                reason: format!("Secret resolution failed: {e}"),
                class: ErrorClass::Unexpected,
            },
            Err(e) => {
                tracing::error!(error = %e, "RuntimeSecurityGuard error; failing closed");
                GuardrailDecision::Block {
                    reason: format!("Security check unavailable: {e}"),
                    class: ErrorClass::Unexpected,
                }
            }
        }
    }

    /// For `tool_call` we always want a fresh, rendered args payload back to
    /// the caller, even if the orchestrator returned `Clean { text }` where
    /// the only change was placeholder substitution. So we wrap `Clean`'s
    /// text as `Sanitize` iff the text differs from the original.
    fn map_tool_call(
        original: &str,
        result: Result<GuardResult, SecurityGuardError>,
    ) -> GuardrailDecision {
        match result {
            Ok(GuardResult::Clean { text }) if text != original => {
                GuardrailDecision::Sanitize(Replacement {
                    text,
                    source: "pii_secrets (placeholder substitution)".to_string(),
                })
            }
            other => Self::map_outbound(original, other),
        }
    }
}

#[async_trait]
impl InputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        let ctx = SecurityContext::default();
        let r = self.guard.process_outbound(text, None, ctx).await;
        Self::map_outbound(text, r)
    }
}

#[async_trait]
impl OutputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        let ctx = SecurityContext::default();
        let r = self.guard.process_inbound(text, &ctx).await;
        Self::map_outbound(text, r)
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
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize tool args for guardrail scan; failing closed");
                return GuardrailDecision::Block {
                    reason: format!("Guardrail serialization failed: {e}"),
                    class: ErrorClass::Unexpected,
                };
            }
        };
        let ctx = SecurityContext::default();
        let resolver_ref = self
            .resolver
            .as_ref()
            .map(|a| a.as_ref() as &dyn AsyncSecretResolver);
        let r = self
            .guard
            .process_outbound(&serialized, resolver_ref, ctx)
            .await;
        Self::map_tool_call(&serialized, r)
    }
}

#[cfg(test)]
mod map_outbound_tests {
    use super::*;

    /// Regression: `Warned` carrying a *mutated* text (e.g. invisible-char
    /// stripping / tokenizer-marker scrubbing applied at warn severity) must
    /// surface as `Sanitize` so the caller swaps the cleaned text in. Mapping
    /// it to `Warn` would discard the scrub and let the original text through.
    #[test]
    fn warned_with_mutated_text_sanitizes() {
        let dec = PiiSecretsGuardrail::map_outbound(
            "raw\u{200b}text",
            Ok(GuardResult::Warned {
                text: "rawtext".to_string(),
                warnings: vec!["stripped 1 invisible char".to_string()],
            }),
        );
        match dec {
            GuardrailDecision::Sanitize(rep) => {
                assert_eq!(rep.text, "rawtext");
                assert!(rep.source.contains("warn"));
            }
            other => panic!("expected Sanitize for mutated Warned, got {other:?}"),
        }
    }

    /// `Warned` whose text equals the input is a pure advisory — stays `Warn`.
    #[test]
    fn warned_with_unchanged_text_warns() {
        let dec = PiiSecretsGuardrail::map_outbound(
            "same",
            Ok(GuardResult::Warned {
                text: "same".to_string(),
                warnings: vec!["leak detector advisory".to_string()],
            }),
        );
        assert!(matches!(dec, GuardrailDecision::Warn { .. }));
    }
}

#[cfg(test)]
mod delegation_tests {
    use super::*;
    use crate::guardrails::decision::GuardrailDecision;
    use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
    use crate::secrets::injection::AsyncSecretResolver;
    use crate::secrets::types::{DecryptedSecret, SecretError};
    use crate::sync_primitives::Arc;
    use async_trait::async_trait;

    struct StubResolver;

    #[async_trait]
    impl AsyncSecretResolver for StubResolver {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            match name {
                "test_key" => Ok(DecryptedSecret::new("resolved-VAL".to_string())),
                _ => Err(SecretError::NotFound(name.to_string())),
            }
        }
    }

    fn guard(with_resolver: bool) -> PiiSecretsGuardrail {
        let resolver: Option<Arc<dyn AsyncSecretResolver>> = if with_resolver {
            Some(Arc::new(StubResolver))
        } else {
            None
        };
        PiiSecretsGuardrail::with_resolver(resolver)
    }

    #[tokio::test]
    async fn input_does_not_resolve_placeholder() {
        let g = guard(true);
        let dec = g.evaluate_input("hello {{secret:test_key}}").await;
        match dec {
            GuardrailDecision::Allow | GuardrailDecision::Warn { .. } => {}
            GuardrailDecision::Sanitize(rep) => {
                assert!(
                    !rep.text.contains("resolved-VAL"),
                    "input must never expose plaintext secret"
                );
            }
            GuardrailDecision::Block { .. } => panic!("input should not block this benign text"),
        }
    }

    #[tokio::test]
    async fn output_does_not_resolve_placeholder() {
        let g = guard(true);
        let dec = g.evaluate_output("LLM said {{secret:test_key}}").await;
        if let GuardrailDecision::Sanitize(rep) = dec {
            assert!(
                !rep.text.contains("resolved-VAL"),
                "output must never expose plaintext secret"
            );
        }
    }

    #[tokio::test]
    async fn tool_call_resolves_placeholder() {
        let g = guard(true);
        let args = serde_json::json!({ "command": "echo {{secret:test_key}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Sanitize(rep) => {
                assert!(
                    rep.text.contains("resolved-VAL"),
                    "tool_call must resolve placeholder; got `{}`",
                    rep.text
                );
                assert!(!rep.text.contains("{{secret:"));
            }
            other => panic!("expected Sanitize, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tool_call_without_resolver_passes_placeholder_through() {
        let g = guard(false);
        let args = serde_json::json!({ "command": "echo {{secret:test_key}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Allow => {}
            GuardrailDecision::Sanitize(rep) => {
                assert!(rep.text.contains("{{secret:test_key}}"));
                assert!(!rep.text.contains("resolved-VAL"));
            }
            other => panic!("expected Allow or pass-through Sanitize, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tool_call_unknown_secret_blocks() {
        let g = guard(true);
        let args = serde_json::json!({ "command": "echo {{secret:ghost}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Block { reason, .. } => {
                assert!(
                    reason.contains("ghost"),
                    "reason must name the missing secret"
                );
            }
            other => panic!("expected Block, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod input_blocking_tests {
    use super::*;
    use crate::guardrails::decision::GuardrailDecision;
    use crate::guardrails::traits::InputGuardrail;

    fn pasted(prefix: &str) -> String {
        format!("My key is {}{}", prefix, "A".repeat(40))
    }

    #[tokio::test]
    async fn input_blocks_pasted_api_keys() {
        let g = PiiSecretsGuardrail::with_resolver(None);
        let cases = ["sk-proj-", "sk-ant-", "AKIA", "ghp_", "glpat-"];
        for prefix in cases {
            let text = pasted(prefix);
            let dec = g.evaluate_input(&text).await;
            match dec {
                GuardrailDecision::Block { reason, .. } => {
                    assert!(
                        reason.to_lowercase().contains("leak")
                            || reason.to_lowercase().contains("secret")
                            || reason.to_lowercase().contains("api"),
                        "Block reason should mention leak/secret/api; got `{reason}` for prefix `{prefix}`"
                    );
                }
                other => panic!("prefix `{prefix}` was NOT blocked; got {other:?}"),
            }
        }
    }
}
