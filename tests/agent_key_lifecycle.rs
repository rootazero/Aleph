//! Rotating and revoking a key are **ledger** operations, and they are awaited.
//!
//! They used to be a two-step protocol owned by the caller: mutate the keystore,
//! then enqueue the chain record and return success. The second step could fail
//! — a full queue drained by a writer that then died, a disk error, or simply
//! the process exiting before the record was written — and its failure was
//! silent *and* permanent: the incoming key was already active, the chain had
//! never declared it, and every record it went on to sign faulted as
//! `UndeclaredSigner` from then on. The tool had already told the operator the
//! rotation succeeded.
//!
//! Like its sibling `agent_ledger_gate_refusals.rs`, this lives in `tests/`
//! because [`identity::install`] takes a process-wide `OnceLock` **and** spawns
//! its writer on the installing runtime: one install, one `#[tokio::test]`, one
//! binary. A second test function here would either be refused the install or
//! be left waiting on a writer whose runtime had already shut down.

use std::time::{Duration, Instant};

use tempfile::TempDir;

use alephcore::gateway::security::shared_token::SharedTokenManager;
use alephcore::gateway::security::store::SecurityStore;
use alephcore::identity::{
    AgentKeystore, AgentLedger, LedgerAction, LedgerOutcome, NewRecord, Rotation,
};
use alephcore::sync_primitives::Arc;

fn call(agent: &str, target: &str) -> NewRecord {
    NewRecord {
        agent_id: agent.to_string(),
        action: LedgerAction::ToolCall,
        target: target.to_string(),
        outcome: LedgerOutcome::Ok,
        args_fp: Some("fp".into()),
        detail: format!("{target}: did a thing"),
    }
}

fn actions(store: &SecurityStore, agent: &str) -> Vec<LedgerAction> {
    store
        .ledger_chain(agent)
        .unwrap()
        .iter()
        .map(|r| r.action)
        .collect()
}

#[tokio::test]
async fn key_lifecycle_changes_are_on_the_chain_before_the_caller_is_told() {
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

    // ── rotating an agent that has never been seen ──────────────────────────
    let Rotation {
        identity,
        previous_fingerprint,
    } = alephcore::identity::rotate_identity("newcomer")
        .await
        .expect("rotate");
    assert_eq!(previous_fingerprint, None, "nothing to replace");
    assert_eq!(
        store.ledger_chain("newcomer").unwrap()[0].signer_fp,
        identity.active_fingerprint,
        "a chain opened by a rotation opens under the key it hands over to",
    );
    // No polling anywhere in this test: the whole point is that the record is
    // written by the time the call returns.
    assert_eq!(
        actions(&store, "newcomer"),
        vec![LedgerAction::IdentityCreated, LedgerAction::IdentityRotated],
    );
    assert!(ledger.verify("newcomer").unwrap().ok);

    // ── rotating an agent with history ──────────────────────────────────────
    alephcore::identity::record_action(call("main", "bash")).await;
    // `record_action` is the fire-and-forget path; the barrier is how a caller
    // that needs it on disk says so.
    assert!(alephcore::identity::flush().await, "flush must be served");
    assert_eq!(
        actions(&store, "main"),
        vec![LedgerAction::IdentityCreated, LedgerAction::ToolCall],
        "the barrier means everything enqueued before it is written"
    );
    let before = ledger
        .keys()
        .identity("main")
        .unwrap()
        .unwrap()
        .active_fingerprint;

    let rotation = alephcore::identity::rotate_identity("main")
        .await
        .expect("rotate");
    assert_eq!(
        rotation.previous_fingerprint.as_deref(),
        Some(before.as_str()),
        "reported from what actually happened, not read a second time"
    );
    assert_ne!(rotation.identity.active_fingerprint, before);
    assert_eq!(
        actions(&store, "main"),
        vec![
            LedgerAction::IdentityCreated,
            LedgerAction::ToolCall,
            LedgerAction::IdentityRotated,
        ],
    );

    // The rows the incoming key signs from here on are covered by that
    // declaration — which is the whole reason it may not be lost.
    alephcore::identity::record_action(call("main", "file_write")).await;
    assert!(alephcore::identity::flush().await);
    let report = ledger.verify("main").unwrap();
    assert!(report.ok, "{:?}", report.faults);
    let chain = store.ledger_chain("main").unwrap();
    assert_eq!(
        chain.last().unwrap().signer_fp,
        rotation.identity.active_fingerprint,
    );
    assert_eq!(chain[1].signer_fp, before, "history keeps its own signer");

    // ── revoking ────────────────────────────────────────────────────────────
    let retired = alephcore::identity::revoke_identity("main")
        .await
        .expect("revoke");
    assert_eq!(
        retired.as_deref(),
        Some(rotation.identity.active_fingerprint.as_str()),
    );
    let chain = store.ledger_chain("main").unwrap();
    assert_eq!(chain.last().unwrap().action, LedgerAction::IdentityRevoked);
    assert_eq!(
        chain.last().unwrap().signer_fp,
        rotation.identity.active_fingerprint,
        "signed by the key it retires — the chain's last statement under it",
    );
    assert!(ledger
        .keys()
        .identity("main")
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(ledger.verify("main").unwrap().ok);

    // Revoking again is not an error and does not add a second statement.
    let len = store.ledger_chain("main").unwrap().len();
    assert_eq!(
        alephcore::identity::revoke_identity("main").await.unwrap(),
        None
    );
    assert_eq!(store.ledger_chain("main").unwrap().len(), len);

    // The mutable column and the chain agree — and the report says so, which is
    // the only place that claim is ever actually checked.
    let report = ledger.verify("main").unwrap();
    assert_eq!(report.revoked_in_chain, Some(true));
    assert!(!report.revocation_disagrees());

    // ── the barrier under load ──────────────────────────────────────────────
    // Enough records that the writer cannot plausibly have drained them by the
    // time the last `record_action` returns; the barrier is what makes the
    // assertion below deterministic rather than a race the test usually wins.
    let started = Instant::now();
    for i in 0..200 {
        alephcore::identity::record_action(call("busy", &format!("t{i}"))).await;
    }
    assert!(alephcore::identity::flush().await);
    assert_eq!(
        store.ledger_chain("busy").unwrap().len(),
        201,
        "200 records plus the opening one, all on disk after the barrier"
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(ledger.verify("busy").unwrap().ok);
    assert_eq!(ledger.lost(), 0, "nothing was dropped along the way");
}
