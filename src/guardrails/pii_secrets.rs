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
use futures::future::BoxFuture;
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
        }
    }

    /// Scan one string leaf of the tool args through the orchestrator and
    /// return the text to put back into the rebuilt `Value`. `Err` carries
    /// the decision the whole tool call must return (a `Block`). A `Clean`
    /// result whose text differs from the leaf carries a resolved
    /// placeholder — the caller always wants the rendered args back, so it
    /// counts as a changed leaf (the old `map_tool_call` special case).
    async fn scan_tool_arg_leaf(
        &self,
        leaf: &str,
        resolver_ref: Option<&dyn AsyncSecretResolver>,
        warnings: &mut Vec<String>,
        sources: &mut Vec<String>,
    ) -> Result<String, GuardrailDecision> {
        let ctx = SecurityContext::default();
        let r = self.guard.process_outbound(leaf, resolver_ref, ctx).await;
        if let Ok(GuardResult::Clean { text }) = &r {
            if text != leaf {
                sources.push("pii_secrets (placeholder substitution)".to_string());
                return Ok(text.clone());
            }
        }
        match Self::map_outbound(leaf, r) {
            GuardrailDecision::Allow => Ok(leaf.to_string()),
            GuardrailDecision::Warn { reason } => {
                warnings.push(reason);
                Ok(leaf.to_string())
            }
            GuardrailDecision::Sanitize(rep) => {
                sources.push(rep.source);
                Ok(rep.text)
            }
            block => Err(block),
        }
    }

    /// Walk string leaves (and object keys) of `args`, scanning each through
    /// the orchestrator, and rebuild the value with the results.
    fn scan_tool_args<'a>(
        &'a self,
        value: &'a Value,
        resolver_ref: Option<&'a dyn AsyncSecretResolver>,
        warnings: &'a mut Vec<String>,
        sources: &'a mut Vec<String>,
    ) -> BoxFuture<'a, Result<Value, GuardrailDecision>> {
        Box::pin(async move {
            match value {
                Value::String(s) => Ok(Value::String(
                    self.scan_tool_arg_leaf(s, resolver_ref, warnings, sources)
                        .await?,
                )),
                Value::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        out.push(
                            self.scan_tool_args(item, resolver_ref, warnings, sources)
                                .await?,
                        );
                    }
                    Ok(Value::Array(out))
                }
                Value::Object(map) => {
                    let mut out = serde_json::Map::with_capacity(map.len());
                    for (key, val) in map {
                        let new_key = self
                            .scan_tool_arg_leaf(key, resolver_ref, warnings, sources)
                            .await?;
                        let new_val = self
                            .scan_tool_args(val, resolver_ref, warnings, sources)
                            .await?;
                        out.insert(new_key, new_val);
                    }
                    Ok(Value::Object(out))
                }
                _ => Ok(value.clone()),
            }
        })
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
        // Scan and rebuild the args leaf by leaf instead of substituting
        // into the serialized JSON text: `RuntimeSecurityGuard` performs raw
        // string replacement, so a secret containing `"`, `\` or a newline
        // would corrupt the payload — the caller's reparse then fails and
        // silently falls back to the ORIGINAL args, delivering the
        // unresolved `{{secret:NAME}}` placeholder to the tool. Rebuilding
        // the `Value` keeps it structured, so re-serialization escapes the
        // secret correctly. Each leaf is scanned with the placeholder still
        // in place (the orchestrator substitutes after its own scan), so
        // injected secrets are registered and leak detection behaves exactly
        // as the single-shot path did.
        let resolver_ref = self
            .resolver
            .as_ref()
            .map(|a| a.as_ref() as &dyn AsyncSecretResolver);
        let mut warnings = Vec::new();
        let mut sources = Vec::new();
        let resolved = match self
            .scan_tool_args(args, resolver_ref, &mut warnings, &mut sources)
            .await
        {
            Ok(v) => v,
            Err(decision) => return decision,
        };
        if resolved != *args {
            match serde_json::to_string(&resolved) {
                Ok(text) => {
                    let mut source = sources.join("; ");
                    if !source.is_empty() && !warnings.is_empty() {
                        source.push_str("; ");
                    }
                    source.push_str(&warnings.join("; "));
                    GuardrailDecision::Sanitize(Replacement { text, source })
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to serialize rebuilt tool args; failing closed");
                    GuardrailDecision::Block {
                        reason: format!("Guardrail serialization failed: {e}"),
                        class: ErrorClass::Unexpected,
                    }
                }
            }
        } else if !warnings.is_empty() {
            GuardrailDecision::Warn {
                reason: warnings.join("; "),
            }
        } else {
            GuardrailDecision::Allow
        }
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

    /// Secret value carrying JSON metacharacters — quote, backslash, newline.
    const SPECIAL_SECRET: &str = "line1\n\"quoted\"\\tail";

    #[async_trait]
    impl AsyncSecretResolver for StubResolver {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            match name {
                "test_key" => Ok(DecryptedSecret::new("resolved-VAL".to_string())),
                "special_key" => Ok(DecryptedSecret::new(SPECIAL_SECRET.to_string())),
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

    /// Regression: a secret containing JSON metacharacters (`"`, `\`,
    /// newline) must survive the tool-call surface. Substitution used to
    /// happen on the serialized JSON text; such a secret corrupted the
    /// payload, the caller's reparse failed, and it silently fell back to
    /// the ORIGINAL args — passing the unresolved placeholder to the tool.
    #[tokio::test]
    async fn tool_call_secret_with_json_metacharacters_round_trips() {
        let g = guard(true);
        let args = serde_json::json!({ "command": "echo {{secret:special_key}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Sanitize(rep) => {
                let parsed: Value =
                    serde_json::from_str(&rep.text).expect("sanitized args must reparse as JSON");
                assert_eq!(
                    parsed["command"].as_str(),
                    Some(format!("echo {SPECIAL_SECRET}").as_str()),
                );
            }
            other => panic!("expected Sanitize, got {:?}", other),
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
