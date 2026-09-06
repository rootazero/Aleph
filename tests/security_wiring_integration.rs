//! End-to-end integration: tool-call placeholder substitution wired through
//! PiiSecretsGuardrail → RuntimeSecurityGuard → AuditDrain.

use std::sync::Arc;

use alephcore::gateway::security::store::SecurityStore;
use alephcore::guardrails::decision::GuardrailDecision;
use alephcore::guardrails::traits::ToolCallGuardrail;
use alephcore::guardrails::PiiSecretsGuardrail;
use alephcore::secrets::injection::AsyncSecretResolver;
use alephcore::secrets::types::{DecryptedSecret, SecretError};
use alephcore::security::spawn_audit_drain;
use alephcore::security::{RuntimeSecurityGuard, SecurityGuardConfig};
use async_trait::async_trait;
use serde_json::json;

struct StubResolver;

#[async_trait]
impl AsyncSecretResolver for StubResolver {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        Ok(DecryptedSecret::new(format!("VAL-{name}")))
    }
}

#[tokio::test]
async fn tool_call_placeholder_resolves_end_to_end() {
    let (guard, audit_rx) = RuntimeSecurityGuard::new_with_audit(SecurityGuardConfig::default());
    let guard = Arc::new(guard);
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let drain = spawn_audit_drain(audit_rx, store.clone(), guard.audit_dropped_counter());

    let resolver: Option<Arc<dyn AsyncSecretResolver>> = Some(Arc::new(StubResolver));
    let pii = PiiSecretsGuardrail::with_guard_and_resolver(guard.clone(), resolver);

    let args = json!({ "command": "echo {{secret:openai_main}}" });
    let dec = pii.evaluate_tool_call("bash_exec", &args).await;

    let rep = match dec {
        GuardrailDecision::Sanitize(rep) => rep,
        other => panic!("expected Sanitize, got {other:?}"),
    };
    assert!(rep.text.contains("VAL-openai_main"), "got `{}`", rep.text);
    assert!(!rep.text.contains("{{secret:"));

    // Drop the guard's audit_log Sender (via dropping guard) so drain exits.
    drop(guard);
    drop(pii);
    tokio::time::timeout(std::time::Duration::from_secs(2), drain)
        .await
        .expect("drain should exit on sender drop")
        .unwrap();
}
