//! The append-only, hash-chained, signed operation ledger.
//!
//! ## One writer
//!
//! Appends are funnelled through a single task draining a bounded channel, so
//! `(read head → hash → sign → insert → advance anchor)` is never interleaved.
//! The reference implementation buys the same property with a per-tenant
//! Postgres advisory lock; a single-process daemon gets it structurally, and
//! `PRIMARY KEY (agent_id, seq)` remains the backstop if a second writer ever
//! appears.
//!
//! ## Backpressure, not silent drops
//!
//! [`record`] uses `send().await`, unlike the sibling
//! [`SecurityAuditLog`](crate::security::audit::SecurityAuditLog), which
//! `try_send`s and discards on overflow. A metrics channel may drop; an
//! accountability record may not — and the chain cannot detect a record that
//! was never written, so a dropped one is invisible forever. Appends that fail
//! anyway (a disk error, a dead writer) are counted in [`AgentLedger::lost`]
//! and surfaced by the read tool, so "the ledger looks quiet" can be
//! distinguished from "the ledger stopped working".

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use tokio::sync::mpsc;

use super::keystore::{AgentKeystore, KeyError};
use super::record::{LedgerRecord, NewRecord};
use super::verify::{verify_chain, ChainReport};
use crate::sync_primitives::Arc;

/// Buffered appends before producers start waiting. Sized like the audit
/// channel; the writer's per-record work is a signature plus one insert.
const LEDGER_QUEUE: usize = 1024;

/// Reads, appends to and verifies agent chains.
pub struct AgentLedger {
    keys: Arc<AgentKeystore>,
    lost: AtomicU64,
}

impl AgentLedger {
    #[must_use]
    pub const fn new(keys: Arc<AgentKeystore>) -> Self {
        Self {
            keys,
            lost: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub const fn keys(&self) -> &Arc<AgentKeystore> {
        &self.keys
    }

    /// Records this process failed to append. Non-zero means the chain has
    /// holes that verification **cannot** see.
    #[must_use]
    pub fn lost(&self) -> u64 {
        self.lost.load(Ordering::Relaxed)
    }

    /// Append one record: assign position, hash, sign, insert, advance anchor.
    ///
    /// Call from the writer task only — see the module doc.
    pub fn append(&self, new: &NewRecord) -> Result<LedgerRecord, KeyError> {
        // `signing_identity`, not `ensure`: a revoked agent's actions must still
        // be recorded — see `AgentKeystore::signing_identity`.
        let identity = self.keys.signing_identity(&new.agent_id)?;
        let store = self.keys.store();
        let (seq, prev_hash) = store.ledger_next_position(&new.agent_id)?;
        let at_ms = crate::session::events::now_ms();

        let hash = super::hash::compute_hash(&super::hash::Preimage {
            agent_id: &new.agent_id,
            seq,
            at_ms,
            action: new.action,
            outcome: new.outcome,
            target: &new.target,
            args_fp: new.args_fp.as_deref(),
            detail: &new.detail,
            signer_fp: &identity.active_fingerprint,
            prev_hash: prev_hash.as_deref(),
        });
        let signature = self.keys.sign(&identity.active_fingerprint, &hash)?;

        let record = LedgerRecord {
            agent_id: new.agent_id.clone(),
            seq,
            prev_hash,
            hash: hash.to_vec(),
            signature: signature.to_vec(),
            signer_fp: identity.active_fingerprint,
            action: new.action,
            target: new.target.clone(),
            outcome: new.outcome,
            args_fp: new.args_fp.clone(),
            detail: new.detail.clone(),
            at_ms,
        };
        store.ledger_insert(&record)?;
        Ok(record)
    }

    /// Most recent records, newest first. `agent_id = None` spans every agent.
    pub fn recent(
        &self,
        agent_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<LedgerRecord>, KeyError> {
        Ok(self.keys.store().ledger_recent(agent_id, limit)?)
    }

    pub fn verify(&self, agent_id: &str) -> Result<ChainReport, KeyError> {
        verify_chain(&self.keys, agent_id)
    }

    /// Verify every known chain. Used by the read tool and by the boot check.
    pub fn verify_all(&self) -> Result<Vec<ChainReport>, KeyError> {
        self.keys
            .list()?
            .into_iter()
            .map(|id| verify_chain(&self.keys, &id.agent_id))
            .collect()
    }

    fn note_lost(&self) {
        let n = self.lost.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n.is_multiple_of(100) {
            tracing::error!(
                lost = n,
                "agent ledger append failed — the chain now has an undetectable hole"
            );
        }
    }
}

static LEDGER: OnceLock<Arc<AgentLedger>> = OnceLock::new();
static WRITER: OnceLock<mpsc::Sender<NewRecord>> = OnceLock::new();

/// Install the process-wide ledger and start its writer task.
///
/// Idempotent: a second call is ignored, mirroring
/// [`set_global_session_service`](crate::session::service::set_global_session_service).
/// Returns the writer's `JoinHandle` so a caller that wants to observe the task
/// can; the boot path detaches it for the process lifetime.
pub fn install(ledger: Arc<AgentLedger>) -> Option<tokio::task::JoinHandle<()>> {
    if LEDGER.get().is_some() {
        return None;
    }
    let (tx, mut rx) = mpsc::channel::<NewRecord>(LEDGER_QUEUE);
    if LEDGER.set(ledger.clone()).is_err() || WRITER.set(tx).is_err() {
        return None;
    }
    Some(tokio::spawn(async move {
        while let Some(new) = rx.recv().await {
            if let Err(e) = ledger.append(&new) {
                ledger.note_lost();
                tracing::warn!(
                    agent_id = %new.agent_id,
                    action = %new.action,
                    target = %new.target,
                    error = %e,
                    "failed to append agent ledger record"
                );
            }
        }
    }))
}

/// The process-wide ledger, if one has been installed.
#[must_use]
pub fn global() -> Option<Arc<AgentLedger>> {
    LEDGER.get().cloned()
}

/// Enqueue a record from a hot path.
///
/// A no-op when no ledger is installed (unit tests, pre-boot, embedded uses) —
/// the chokepoint must never fail a tool call because accounting is not wired.
pub async fn record(new: NewRecord) {
    let Some(tx) = WRITER.get() else {
        return;
    };
    let agent_id = new.agent_id.clone();
    if let Err(e) = tx.send(new).await {
        if let Some(l) = LEDGER.get() {
            l.note_lost();
        }
        tracing::warn!(agent_id = %agent_id, error = %e, "agent ledger writer is gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::shared_token::SharedTokenManager;
    use crate::gateway::security::store::SecurityStore;
    use crate::identity::record::{LedgerAction, LedgerOutcome};
    use crate::identity::verify::ChainFault;
    use tempfile::TempDir;

    fn ledger() -> (AgentLedger, Arc<SecurityStore>, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store.clone(),
            dir.path().join("t.vault"),
        ));
        let _ = vault.generate_token();
        let keys = Arc::new(AgentKeystore::new(store.clone(), vault));
        (AgentLedger::new(keys), store, dir)
    }

    fn entry(agent: &str, target: &str) -> NewRecord {
        NewRecord {
            agent_id: agent.to_string(),
            action: LedgerAction::ToolCall,
            target: target.to_string(),
            outcome: LedgerOutcome::Ok,
            args_fp: Some("fp".into()),
            detail: format!("{target}: did a thing"),
        }
    }

    #[test]
    fn appends_form_a_verifiable_chain() {
        let (l, _s, _d) = ledger();
        for i in 0..5 {
            l.append(&entry("main", &format!("tool{i}"))).unwrap();
        }
        let report = l.verify("main").unwrap();
        assert!(report.ok, "clean chain must verify: {:?}", report.faults);
        assert_eq!(report.records, 5);
        assert_eq!(report.anchor_seq, 5);
        assert_eq!(report.last_seq, 5);
    }

    #[test]
    fn chains_are_per_agent_and_independent() {
        let (l, _s, _d) = ledger();
        l.append(&entry("main", "a")).unwrap();
        l.append(&entry("trader", "b")).unwrap();
        let second = l.append(&entry("main", "c")).unwrap();
        assert_eq!(second.seq, 2, "trader's append must not advance main's seq");
        assert!(l.verify("main").unwrap().ok);
        assert!(l.verify("trader").unwrap().ok);
    }

    #[test]
    fn editing_a_row_is_detected() {
        let (l, store, _d) = ledger();
        l.append(&entry("main", "bash")).unwrap();
        l.append(&entry("main", "file_ops")).unwrap();

        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE agent_ledger SET target = 'harmless' WHERE agent_id='main' AND seq=1",
                [],
            )
            .unwrap();
        }

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::HashMismatch { seq: 1 }));
    }

    #[test]
    fn deleting_the_tail_is_detected() {
        // The failure the reference implementation is structurally blind to:
        // a truncated chain is internally consistent, so only the anchor can
        // reveal it.
        let (l, store, _d) = ledger();
        for i in 0..4 {
            l.append(&entry("main", &format!("t{i}"))).unwrap();
        }
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM agent_ledger WHERE agent_id='main' AND seq >= 3",
                [],
            )
            .unwrap();
        }

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::TailTruncated {
            anchor_seq: 4,
            last_seq: 2
        }));
    }

    #[test]
    fn a_truncated_tail_stays_visible_after_further_appends() {
        // Numbering the next append above the ANCHOR (not above the surviving
        // last row) is what stops a later write from healing the evidence.
        let (l, store, _d) = ledger();
        for i in 0..4 {
            l.append(&entry("main", &format!("t{i}"))).unwrap();
        }
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM agent_ledger WHERE agent_id='main' AND seq >= 3",
                [],
            )
            .unwrap();
        }
        l.append(&entry("main", "after")).unwrap();

        let report = l.verify("main").unwrap();
        assert!(!report.ok, "the hole must survive later appends");
        assert!(report.faults.iter().any(|f| matches!(
            f,
            ChainFault::SeqGap {
                expected: 3,
                found: 5
            }
        )));
    }

    #[test]
    fn deleting_the_prefix_is_detected() {
        let (l, store, _d) = ledger();
        for i in 0..3 {
            l.append(&entry("main", &format!("t{i}"))).unwrap();
        }
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM agent_ledger WHERE agent_id='main' AND seq = 1",
                [],
            )
            .unwrap();
        }

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report
            .faults
            .contains(&ChainFault::PrefixMissing { first_seq: 2 }));
    }

    #[test]
    fn wiping_the_whole_chain_is_detected() {
        let (l, store, _d) = ledger();
        l.append(&entry("main", "t")).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute("DELETE FROM agent_ledger WHERE agent_id='main'", [])
                .unwrap();
        }
        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report
            .faults
            .contains(&ChainFault::ChainWiped { anchor_seq: 1 }));
    }

    #[test]
    fn a_row_transplanted_from_another_agent_is_detected() {
        // The identity leads the preimage, so a row cannot be re-homed even
        // with its hash and signature copied verbatim.
        let (l, store, _d) = ledger();
        l.append(&entry("main", "a")).unwrap();
        l.append(&entry("trader", "b")).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE agent_ledger SET agent_id='main', seq=2, prev_hash=
                   (SELECT hash FROM agent_ledger WHERE agent_id='main' AND seq=1)
                 WHERE agent_id='trader'",
                [],
            )
            .unwrap();
        }
        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::HashMismatch { seq: 2 }));
    }

    #[test]
    fn a_forged_row_without_the_key_fails_the_signature() {
        // An attacker with write access who recomputes the hash correctly
        // still cannot produce a signature — this is what the keyless
        // reference chain cannot offer at all.
        let (l, store, _d) = ledger();
        let first = l.append(&entry("main", "a")).unwrap();

        let forged = LedgerRecord {
            agent_id: "main".into(),
            seq: 2,
            prev_hash: Some(first.hash.clone()),
            hash: vec![0u8; 32],
            signature: vec![0u8; 64],
            signer_fp: first.signer_fp.clone(),
            action: LedgerAction::ToolCall,
            target: "rm -rf".into(),
            outcome: LedgerOutcome::Ok,
            args_fp: None,
            detail: "forged".into(),
            at_ms: 99,
        };
        let real_hash = super::super::hash::compute_hash(&super::super::hash::Preimage {
            agent_id: &forged.agent_id,
            seq: forged.seq,
            at_ms: forged.at_ms,
            action: forged.action,
            outcome: forged.outcome,
            target: &forged.target,
            args_fp: None,
            detail: &forged.detail,
            signer_fp: &forged.signer_fp,
            prev_hash: forged.prev_hash.as_deref(),
        });
        let forged = LedgerRecord {
            hash: real_hash.to_vec(),
            ..forged
        };
        store.ledger_insert(&forged).unwrap();

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::BadSignature { seq: 2 }));
    }

    #[test]
    fn records_signed_before_a_rotation_still_verify() {
        let (l, _s, _d) = ledger();
        l.append(&entry("main", "before")).unwrap();
        l.keys().rotate("main").unwrap();
        l.append(&entry("main", "after")).unwrap();

        let report = l.verify("main").unwrap();
        assert!(
            report.ok,
            "rotation must not invalidate history: {:?}",
            report.faults
        );
        assert_eq!(report.records, 2);
    }

    #[test]
    fn recording_without_an_installed_ledger_is_a_noop() {
        // The chokepoint must never fail a tool call because accounting is
        // unwired (unit tests, embedded use).
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            record(entry("main", "t")).await;
        });
    }
}
