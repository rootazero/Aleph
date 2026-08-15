//! A sub-agent's allowlist refusal must land on the sub-agent's OWN chain.
//!
//! `AllowlistToolService` refuses disallowed calls **above** the
//! `ScopedToolService` chokepoint, so the chokepoint's refusal recording never
//! fired for them: a denied sub-agent used to leave no trace on any chain and
//! was indistinguishable from an idle one. The wrapper now files a
//! `ToolDenied` record itself, under the delegated role's identity — the same
//! attribution its allowed calls get.
//!
//! Separate binary from `agent_ledger_gate_refusals.rs` for the same reason
//! that file gives: [`identity::install`] is a process-wide `OnceLock`, so
//! each installer needs its own integration-test process.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

use alephcore::agents::allowlist_tool_service::AllowlistToolService;
use alephcore::agents::{AgentDef, AgentMode};
use alephcore::gateway::security::shared_token::SharedTokenManager;
use alephcore::gateway::security::store::SecurityStore;
use alephcore::identity::{AgentKeystore, AgentLedger, LedgerAction, LedgerOutcome};
use alephcore::session::events::{ToolOutput, ToolOutputMetadata};
use alephcore::sync_primitives::Arc;
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService};

struct StubTools;

#[async_trait::async_trait]
impl ToolService for StubTools {
    async fn execute(&self, name: &str, _: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: json!({ "tool": name }),
            metadata: ToolOutputMetadata::default(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        vec![]
    }
    async fn describe(&self, _: &str) -> Option<ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> std::sync::Arc<[alephcore::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from(Vec::new())
    }
}

/// The ledger writer is a task draining a channel; poll instead of sleeping a
/// guessed amount. (Same pattern as `agent_ledger_gate_refusals.rs`.)
async fn wait_for_chain(store: &Arc<SecurityStore>, agent: &str, rows: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let chain = store.ledger_chain(agent).expect("chain readable");
        if chain.len() >= rows || Instant::now() > deadline {
            return chain
                .iter()
                .map(|r| format!("{}/{}", r.action, r.outcome))
                .collect();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_allowlist_refusal_is_signed_onto_the_subagents_own_chain() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let vault = Arc::new(SharedTokenManager::new(
        store.clone(),
        dir.path().join("t.vault"),
    ));
    vault.generate_token().expect("vault master key");
    let ledger = Arc::new(AgentLedger::new(Arc::new(AgentKeystore::new(
        store.clone(),
        vault,
    ))));
    let _writer = alephcore::identity::install(ledger.clone())
        .expect("this test binary installs the ledger exactly once");

    // A sub-agent allowed nothing at all, calling anything. (`AgentDef::new`
    // defaults to `["*"]` — the wildcard — so the empty list must be set
    // explicitly, as the wrapper's own unit tests do.)
    let mut def = AgentDef::new("delegate-1", AgentMode::SubAgent);
    def.allowed_tools = Vec::new();
    let svc = AllowlistToolService::new(Arc::new(StubTools), Arc::new(def));

    let err = svc
        .execute("exec", json!({ "command": "rm -rf /" }))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ToolError::PermissionDenied { .. }),
        "the refusal itself is unchanged: {err:?}"
    );

    let chain = wait_for_chain(&store, "delegate-1", 2).await;
    assert_eq!(
        chain,
        vec!["identity_created/ok", "tool_denied/denied"],
        "the refusal must open the sub-agent's own chain and be recorded on it"
    );

    let last = ledger.recent(Some("delegate-1"), 1).unwrap();
    assert_eq!(last[0].action, LedgerAction::ToolDenied);
    assert_eq!(last[0].outcome, LedgerOutcome::Denied);
    assert_eq!(last[0].target, "exec");
    assert!(
        last[0].args_fp.is_some(),
        "keyed on the call that was refused, so it correlates with the grant surfaces"
    );
    assert!(
        last[0].principal.is_none(),
        "no person is driving inside this test — absent, not guessed"
    );

    let report = ledger.verify("delegate-1").unwrap();
    assert!(report.ok, "{:?}", report.faults);
}
