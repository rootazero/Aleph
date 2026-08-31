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

/// Maximum recursion depth for [`PiiSecretsGuardrail::scan_tool_args`]. See
/// the doc comment on the function for the DoS rationale.
const MAX_SCAN_DEPTH: u32 = 32;
/// Branching-width cap on tool-arg scans. Bounds the number of JSON nodes
/// visited per scan so a pathologically shallow-but-wide input (e.g. a
/// single object holding a 1M-element array of short strings) cannot
/// exhaust memory or starve the orchestrator. Mirrors `MAX_SCAN_DEPTH`'s
/// rationale (cap rather than scan a partial tree) but on a different
/// axis: depth vs total node count. Legitimate tool args stay well under
/// this bound; an attacker would need to construct a payload whose
/// legitimate use-case is indistinguishable from the attack to bypass it.
const MAX_SCAN_NODES: u32 = 65_536;

pub struct PiiSecretsGuardrail {
    guard: Arc<RuntimeSecurityGuard>,
    resolver: Option<Arc<dyn AsyncSecretResolver>>,
}

impl PiiSecretsGuardrail {
    /// Construct over an existing orchestrator with a resolver wired in.
    #[must_use]
    pub fn with_guard_and_resolver(
        guard: Arc<RuntimeSecurityGuard>,
        resolver: Option<Arc<dyn AsyncSecretResolver>>,
    ) -> Self {
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
                // Post-substitution leak blocks ("...in resolved outbound content")
                // are a security-infra failure, not a content-policy violation
                // the model could self-correct — mapping them to `Fixable` would
                // let the orchestrator's class-based retry loop re-run a request
                // whose root cause is "we just leaked a resolved secret". Any
                // other `Blocked` reason from `process_outbound` IS model-correctable
                // (model emitted a secret into a tool arg / output) and stays
                // `Fixable`.
                class: if reason.contains("resolved outbound") {
                    ErrorClass::Unexpected
                } else {
                    ErrorClass::Fixable
                },
                reason,
            },
            Err(SecurityGuardError::SecretResolutionFailed(e)) => {
                // Strip the requested secret name from `reason` before it
                // reaches the model / log / UI: `SecretError::NotFound` and
                // `SecretError::InvalidPlaceholder` echo the user-supplied
                // name verbatim in their `Display` impls, and that echo was
                // exposed via `Block.reason` (see review/guardrails-statics
                // C1) — an attacker who guessed vault keys could iterate
                // `{{secret:NAME}}` and use the differential between "name
                // echoed back" (vault hit) and "name NOT echoed" (vault miss)
                // to enumerate the vault namespace. Logging the name into a
                // dedicated audit field instead keeps triage intact.
                let variant = match &e {
                    crate::secrets::types::SecretError::NotFound(_) => "not_found",
                    crate::secrets::types::SecretError::InvalidPlaceholder(_) => {
                        "invalid_placeholder"
                    }
                    _ => "resolution_failed",
                };
                // `error = %e` was deliberately dropped: `SecretError`'s
                // `Display` impl echoes the user-supplied placeholder name
                // verbatim, and the Block-reason arm above was hardened to
                // strip the name to prevent vault-namespace enumeration. The
                // audit field below carries the pre-classified variant only;
                // the raw error (with name) is logged via a separate
                // elevated-access debug! path that operators must opt into.
                tracing::warn!(
                    secret_kind = "vault",
                    variant = variant,
                    "secret resolution failed during guardrail evaluation"
                );
                GuardrailDecision::Block {
                    reason: "secret resolution failed".to_string(),
                    class: ErrorClass::Unexpected,
                }
            }
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
    ///
    /// **Depth bound**: each recursion level heap-allocates a `BoxFuture`, so
    /// a pathologically deep payload (a 10k-nested `Value::Array` pasted into
    /// a tool arg) allocated linearly with depth — a memory-exhaustion DoS on
    /// attacker-supplied tool args on the tool-dispatch hot path. Recursion
    /// is capped at [`MAX_SCAN_DEPTH`]; exceeding it fails closed with a
    /// `Block { class: Unexpected }` rather than scanning a partial tree
    /// (which would let a deep leaf evade inspection). Legitimate tool args
    /// nest <10 levels; 32 leaves generous headroom.
    fn scan_tool_args<'a>(
        &'a self,
        value: &'a Value,
        resolver_ref: Option<&'a dyn AsyncSecretResolver>,
        warnings: &'a mut Vec<String>,
        sources: &'a mut Vec<String>,
    ) -> BoxFuture<'a, Result<Value, GuardrailDecision>> {
        let mut nodes: u32 = 0;
        self.scan_tool_args_at_depth(value, resolver_ref, warnings, sources, 0, &mut nodes)
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_tool_args_at_depth<'a>(
        &'a self,
        value: &'a Value,
        resolver_ref: Option<&'a dyn AsyncSecretResolver>,
        warnings: &'a mut Vec<String>,
        sources: &'a mut Vec<String>,
        depth: u32,
        nodes: &'a mut u32,
    ) -> BoxFuture<'a, Result<Value, GuardrailDecision>> {
        Box::pin(async move {
            if depth > MAX_SCAN_DEPTH {
                tracing::warn!(
                    subsystem = "guardrails",
                    depth = depth,
                    max = MAX_SCAN_DEPTH,
                    "tool-arg scan depth exceeded; failing closed"
                );
                return Err(GuardrailDecision::Block {
                    reason: format!(
                        "tool call arguments nested deeper than {MAX_SCAN_DEPTH} levels"
                    ),
                    class: ErrorClass::Unexpected,
                });
            }
            // The depth cap does NOT bound branching width: a single
            // `{ "items": [<1M short strings>] }` payload recurses 2 levels
            // deep (object -> array -> string) and processes every leaf.
            // Count visited nodes at every entry so a wide-but-shallow
            // adversarial payload cannot exhaust memory on the tool
            // dispatch hot path. The counter increments once per recursion
            // entry; legitimate tool args are well under the bound.
            *nodes = nodes.saturating_add(1);
            if *nodes > MAX_SCAN_NODES {
                tracing::warn!(
                    subsystem = "guardrails",
                    nodes = *nodes,
                    max = MAX_SCAN_NODES,
                    "tool-arg scan node budget exceeded; failing closed"
                );
                return Err(GuardrailDecision::Block {
                    reason: format!(
                        "tool call arguments contain more than {MAX_SCAN_NODES} JSON nodes"
                    ),
                    class: ErrorClass::Unexpected,
                });
            }
            match value {
                Value::String(s) => Ok(Value::String(
                    self.scan_tool_arg_leaf(s, resolver_ref, warnings, sources)
                        .await?,
                )),
                Value::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        out.push(
                            self.scan_tool_args_at_depth(
                                item,
                                resolver_ref,
                                warnings,
                                sources,
                                depth + 1,
                                nodes,
                            )
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
                            .scan_tool_args_at_depth(
                                val,
                                resolver_ref,
                                warnings,
                                sources,
                                depth + 1,
                                nodes,
                            )
                            .await?;
                        out.insert(new_key, new_val);
                    }
                    Ok(Value::Object(out))
                }
                // Numbers are NOT a safe bypass: an 11-digit phone number or
                // a 16-digit bank-card number can be expressed as a JSON
                // number, and the PII rules operate on strings. Convert
                // number leaves to their string form for scanning; on a hit
                // we redact the string form and re-parse it back to a number
                // when possible (preserving the wire shape for well-formed
                // values), or fall back to a redacted string when the
                // guardrail mangled it beyond numeric recognition (e.g.
                // `13812345678` -> `[REDACTED]` can't be re-parsed as a
                // number, so we return a string rather than guessing).
                Value::Number(n) => {
                    let text = n.to_string();
                    let scanned = self
                        .scan_tool_arg_leaf(&text, resolver_ref, warnings, sources)
                        .await?;
                    if scanned == text {
                        Ok(value.clone())
                    } else if let Ok(num) = scanned.parse::<serde_json::Number>() {
                        Ok(Value::Number(num))
                    } else {
                        // The wire shape changed from Number to String: a
                        // downstream tool that type-validates its args (u64
                        // schema) will now see a String. Log the delta so the
                        // audit trail records the shape change — silently
                        // coercing here would make a redacted phone number
                        // indistinguishable from a legitimate string arg.
                        tracing::warn!(
                            subsystem = "guardrails",
                            original_type = "number",
                            resulting_type = "string",
                            "number leaf redaction changed JSON wire type"
                        );
                        Ok(Value::String(scanned))
                    }
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
        // `_tool_name` is INTENTIONALLY unused — do not wire it to
        // `security::dangerous_tools` here. That list is an *untrusted-
        // surface hard floor* (its own doc: "Owner / local-trusted callers
        // are never restricted here"), and it is already enforced at the
        // untrusted execution entry points (`handlers/tools_invoke.rs`,
        // `execution_engine/slash_command.rs`, `tasks/heartbeat/probe.rs`).
        // This guardrail runs inside the agent loop (`harness/agent/act.rs`),
        // i.e. in the OWNER's context — an owner-driven agent legitimately
        // uses `bash` / `file_write` / `self_config` to do its job. Blocking
        // those by name at THIS layer would break the owner's agent, which is
        // exactly the caller class the dangerous-tools design exempts. The
        // parameter stays in the trait signature so a future guardrail that
        // has a *per-tool* content policy (not an identity-based deny floor)
        // can consume it without a trait change.
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
        let guard = Arc::new(RuntimeSecurityGuard::default_guard());
        PiiSecretsGuardrail::with_guard_and_resolver(guard, resolver)
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
        // The Block reason deliberately does NOT echo the requested
        // secret name (see the SecretResolutionFailed branch above —
        // stripping the name closes a vault-namespace enumeration side
        // channel exposed via the model-visible reason string). The
        // invariant this test pins is "missing secret -> Block", not
        // "missing secret -> reason contains name".
        let g = guard(true);
        let args = serde_json::json!({ "command": "echo {{secret:ghost}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Block { reason, .. } => {
                assert!(
                    !reason.is_empty(),
                    "Block reason must carry enough info to triage; \
                     got empty string"
                );
                assert!(
                    reason.to_lowercase().contains("secret"),
                    "Block reason must mention the secret pipeline so \
                     the operator can route to vault config; got {reason:?}"
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
        let guard = Arc::new(RuntimeSecurityGuard::default_guard());
        let g = PiiSecretsGuardrail::with_guard_and_resolver(guard, None);
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
