//! A gate that refuses **without asking anyone** must still leave a signed
//! record.
//!
//! Two branches used to refuse and record nothing: the operator gate and the
//! confirmation gate, each when no approval channel is wired at all. Both are
//! fail-closed paths — exactly the situation an accountability trail is for —
//! and both returned their error above `confirm_with_memory`, which is where
//! every other refusal files its decision.
//!
//! This lives in `tests/` rather than beside the unit tests because
//! [`identity::install`] takes a process-wide `OnceLock`: an integration test
//! binary is one process, so it can install a real ledger and then read the
//! chain the dispatch actually wrote. The unit tests in `src/tools/scoped/`
//! run with no ledger installed and can only assert the returned error.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use alephcore::gateway::security::shared_token::SharedTokenManager;
use alephcore::gateway::security::store::SecurityStore;
use alephcore::identity::{AgentKeystore, AgentLedger, LedgerAction, LedgerOutcome};
use alephcore::routing::session_key::SessionKey;
use alephcore::sync_primitives::Arc;
use alephcore::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
use alephcore::tools::scoped::ScopedToolService;
use alephcore::tools::service::ToolService;
use alephcore::tools::turn_context::TurnContext;

/// A tool that declares it needs confirmation, so the confirmation gate fires
/// on its own declaration rather than on a permission override.
struct NeedsConfirmation;

#[async_trait::async_trait]
impl LoopTool for NeedsConfirmation {
    fn name(&self) -> &str {
        "needs_confirmation"
    }
    fn description(&self) -> &str {
        "test stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
        ToolResult::Success { output: json!({}) }
    }
}

/// A config-mutating tool, so the operator gate fires for a chat-tier caller.
struct ConfigTool;

#[async_trait::async_trait]
impl LoopTool for ConfigTool {
    fn name(&self) -> &str {
        "cron_manage"
    }
    fn description(&self) -> &str {
        "test stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
        ToolResult::Success { output: json!({}) }
    }
}

fn chat_tier_turn(agent: &str) -> TurnContext {
    TurnContext {
        session_key: SessionKey::main(agent),
        run_id: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        // Not an operator: what makes the config gate applicable at all.
        caller_role: Some("guest".to_string()),
        channel_tool_permissions: None,
        unattended: false,
    }
}

/// The ledger writer is a task draining a channel, so a record is not on disk
/// the instant `execute` returns. Poll instead of sleeping a guessed amount.
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
async fn a_gate_that_refuses_without_an_approval_channel_still_signs_a_record() {
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

    // ── the operator gate, with no config-approval channel ──────────────────
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(ConfigTool));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(chat_tier_turn("gate-operator"));

    let err = svc.execute("cron_manage", json!({})).await.unwrap_err();
    assert!(
        err.to_string().contains("operator"),
        "expected the fail-closed operator refusal, got {err:?}"
    );

    let chain = wait_for_chain(&store, "gate-operator", 2).await;
    assert_eq!(
        chain,
        vec!["identity_created/ok", "approval_denied/denied"],
        "a refusal nobody was asked about must still open the chain and be recorded"
    );

    // ── the confirmation gate, with no approval channel ─────────────────────
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(NeedsConfirmation));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(chat_tier_turn("gate-confirm"));

    let err = svc
        .execute("needs_confirmation", json!({}))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no approval"),
        "expected the fail-closed confirmation refusal, got {err:?}"
    );

    let chain = wait_for_chain(&store, "gate-confirm", 2).await;
    assert_eq!(chain, vec!["identity_created/ok", "approval_denied/denied"]);

    // Both chains verify: a refusal record is an ordinary signed row.
    for agent in ["gate-operator", "gate-confirm"] {
        let report = ledger.verify(agent).unwrap();
        assert!(report.ok, "{agent}: {:?}", report.faults);
    }

    // And the decision is filed as an approval denial, not as a tool call —
    // the authority to run was withheld, which is a different fact from the
    // call itself.
    let last = ledger.recent(Some("gate-operator"), 1).unwrap();
    assert_eq!(last[0].action, LedgerAction::ApprovalDenied);
    assert_eq!(last[0].outcome, LedgerOutcome::Denied);
    assert!(
        last[0].args_fp.is_some(),
        "keyed on the call it refused, so it correlates with a later approval"
    );
}
