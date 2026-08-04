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
//!
//! Queued-but-unwritten is the third outcome, and it used to be the silent one:
//! the process exiting with records still in the channel lost them **and** the
//! count of them, because nothing had failed yet. [`flush`] is the barrier that
//! closes it — the queue is FIFO and there is exactly one writer, so an
//! acknowledged flush means everything enqueued before it is on disk. The boot
//! path awaits it during graceful shutdown.
//!
//! ## Key lifecycle is a ledger operation, not a caller's two-step
//!
//! Rotating and revoking a key are not "mutate the keystore, then mention it on
//! the chain". The mention is what makes the incoming key legitimate
//! ([`ChainFault::UndeclaredSigner`](super::verify::ChainFault::UndeclaredSigner)),
//! so a caller that performs the mutation and then *enqueues* the declaration
//! has a two-step protocol whose second step can fail silently — and whose
//! failure is permanent, because every record the new key goes on to sign
//! faults forever. That is what [`rotate_identity`] and [`revoke_identity`]
//! replace: both halves run **inside the writer**, in the order that survives a
//! failure (mint → declare → activate; declare → mark), and the caller awaits
//! the outcome instead of assuming it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use tokio::sync::{mpsc, oneshot};

use super::keystore::{AgentIdentityRow, AgentKeystore, KeyError};
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

    /// Records this installation failed to append. Non-zero means the chains
    /// have holes that verification **cannot** see.
    ///
    /// Reads the durable counter and returns the larger of it and this
    /// process's own — the two disagree only when the failure that lost the
    /// record also lost the count of it, and in that direction the bigger
    /// number is the honest one. A store that cannot be read at all falls back
    /// to the process counter rather than reporting a reassuring zero.
    #[must_use]
    pub fn lost(&self) -> u64 {
        let here = self.lost.load(Ordering::Relaxed);
        let durable = self
            .keys
            .store()
            .ledger_lost_total()
            .ok()
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0);
        here.max(durable)
    }

    /// Append one record: assign position, hash, sign, insert, advance anchor.
    ///
    /// A chain that is opening (position 1) gets a signed
    /// [`IdentityCreated`](super::record::LedgerAction::IdentityCreated) row
    /// first, so "this chain belongs to this agent and began under this key"
    /// is itself a chained, signed fact rather than a claim resting on the
    /// mutable `agent_identities` row. The follow-on position is known without
    /// re-querying: an empty chain's next two slots are 1 and 2.
    ///
    /// Call from the writer task only — see the module doc.
    pub fn append(&self, new: &NewRecord) -> Result<LedgerRecord, KeyError> {
        // `signing_identity`, not `ensure`: a revoked agent's actions must still
        // be recorded — see `AgentKeystore::signing_identity`.
        let identity = self.keys.signing_identity(&new.agent_id)?;
        self.append_signed_by(&identity.active_fingerprint, new)
    }

    /// [`Self::append`] with the signing key named outright.
    ///
    /// Only [`Self::perform_rotate`] needs this: the record that *declares* an
    /// incoming key has to be signed by that key while the agent's active key is
    /// still the outgoing one, which is exactly the window that lets a failed
    /// rotation leave nothing broken behind it.
    fn append_signed_by(&self, signer_fp: &str, new: &NewRecord) -> Result<LedgerRecord, KeyError> {
        let (seq, prev_hash) = self.keys.store().ledger_next_position(&new.agent_id)?;

        if seq == 1 {
            let genesis = self.write(
                signer_fp,
                &NewRecord::identity_created(&new.agent_id, signer_fp),
                1,
                None,
            )?;
            return self.write(signer_fp, new, 2, Some(genesis.hash));
        }
        self.write(signer_fp, new, seq, prev_hash)
    }

    /// Replace an agent's signing key and make its own chain say so, as one
    /// operation on the single writer.
    ///
    /// The order is the whole point. Minting first and **activating last** means
    /// every failure short of the final upsert leaves the outgoing key active
    /// and declared, and the unused new key inert. The reverse order — activate,
    /// then enqueue the declaration, which is what the `agent_identity` tool
    /// used to do — turns any lost declaration into a permanent
    /// [`UndeclaredSigner`](super::verify::ChainFault::UndeclaredSigner) fault on
    /// every record the new key subsequently signs, reported by a tool that
    /// already told the operator the rotation succeeded.
    ///
    /// Returns the new identity row and the fingerprint it replaced, so no
    /// caller has to read "what was active before" a second time and risk
    /// disagreeing with what actually happened.
    fn perform_rotate(&self, agent_id: &str) -> Result<Rotation, KeyError> {
        let previous = self.keys.identity(agent_id)?.map(|r| r.active_fingerprint);
        let fingerprint = self.keys.mint_key(agent_id)?;
        let declaration = NewRecord::identity_rotated(agent_id, &fingerprint, previous.as_deref());
        if let Err(e) = self.append_signed_by(&fingerprint, &declaration) {
            // A lifecycle statement that should be on the chain is not. Counted
            // for the same reason an ordinary lost append is: no chain check can
            // see a record that was never written.
            self.note_lost();
            return Err(e);
        }
        Ok(Rotation {
            identity: self.keys.activate(agent_id, &fingerprint)?,
            previous_fingerprint: previous,
        })
    }

    /// Revoke an identity and make its own chain say so.
    ///
    /// Declared **before** the mutable column is marked, the mirror of
    /// [`Self::perform_rotate`]'s ordering and for the same reason: if the two
    /// halves cannot both land, the survivor must be the one that is hard to
    /// erase. A chain that records a revocation the column has not caught up to
    /// is over-eager and visible; a column marked with nothing on the chain is
    /// exactly the state an adversary would manufacture by editing `revoked_at`
    /// back to NULL.
    ///
    /// Returns the fingerprint that was retired, or `None` when the agent had no
    /// live identity to revoke.
    fn perform_revoke(&self, agent_id: &str) -> Result<Option<String>, KeyError> {
        let Some(identity) = self.keys.identity(agent_id)? else {
            return Ok(None);
        };
        if identity.revoked_at.is_some() {
            return Ok(None);
        }
        let fingerprint = identity.active_fingerprint;
        // Signed by the key being revoked — the chain's last statement under it,
        // and the reason `signing_identity` tolerates a revoked agent.
        if let Err(e) = self.append_signed_by(
            &fingerprint,
            &NewRecord::identity_revoked(agent_id, &fingerprint),
        ) {
            self.note_lost();
            return Err(e);
        }
        self.keys.revoke(agent_id)?;
        Ok(Some(fingerprint))
    }

    /// Hash, sign, insert and advance the anchor for one record at a position
    /// its caller has already resolved.
    fn write(
        &self,
        signer_fp: &str,
        new: &NewRecord,
        seq: i64,
        prev_hash: Option<Vec<u8>>,
    ) -> Result<LedgerRecord, KeyError> {
        let store = self.keys.store();
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
            signer_fp,
            prev_hash: prev_hash.as_deref(),
        });
        let signature = self.keys.sign(signer_fp, &hash)?;

        let record = LedgerRecord {
            agent_id: new.agent_id.clone(),
            seq,
            prev_hash,
            hash: hash.to_vec(),
            signature: signature.to_vec(),
            signer_fp: signer_fp.to_string(),
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

    /// Verify every known chain. Used by the `agent_identity` tool and by the
    /// offline `aleph-server identity verify`.
    ///
    /// Enumerates the **union** of the identity table and the agent ids present
    /// in the ledger itself. Driving this off identities alone (what it did)
    /// meant deleting one mutable `agent_identities` row silently removed that
    /// agent's entire chain from the verdict — a clean "all chains OK" that had
    /// simply stopped looking at one of them.
    ///
    /// Deliberately **not** run at boot: a full pass reads every row of every
    /// chain and checks a signature per row, which is the wrong thing to put on
    /// the startup path, and a warning in a log the daemon itself wrote is not
    /// the evidence anyone would act on anyway. Verification belongs where it
    /// is asked for — and, for the case that matters, in the process that did
    /// not write the records.
    pub fn verify_all(&self) -> Result<Vec<ChainReport>, KeyError> {
        self.agent_ids()?
            .iter()
            .map(|id| verify_chain(&self.keys, id))
            .collect()
    }

    /// Every agent this installation has evidence about: one with an identity,
    /// one with records, or both. Sorted, so two runs report in the same order.
    pub fn agent_ids(&self) -> Result<Vec<String>, KeyError> {
        let mut ids: std::collections::BTreeSet<String> =
            self.keys.list()?.into_iter().map(|i| i.agent_id).collect();
        ids.extend(self.keys.store().ledger_agent_ids()?);
        Ok(ids.into_iter().collect())
    }

    /// Agents whose chain has records but **no identity row** — the anchor that
    /// would reveal a truncated tail is gone, and the agent is absent from
    /// every identity-driven listing. Empty in a healthy installation.
    pub fn orphan_chains(&self) -> Result<Vec<String>, KeyError> {
        let known: std::collections::BTreeSet<String> =
            self.keys.list()?.into_iter().map(|i| i.agent_id).collect();
        Ok(self
            .keys
            .store()
            .ledger_agent_ids()?
            .into_iter()
            .filter(|id| !known.contains(id))
            .collect())
    }

    fn note_lost(&self) {
        let n = self.lost.fetch_add(1, Ordering::Relaxed) + 1;
        // Persist so the offline verifier — run in another process, precisely
        // when this one is not trusted — can say the trail is incomplete.
        if let Err(e) = self.keys.store().ledger_note_lost() {
            tracing::warn!(error = %e, "could not persist the ledger loss counter");
        }
        if n == 1 || n.is_multiple_of(100) {
            tracing::error!(
                lost = n,
                "agent ledger append failed — the chain now has an undetectable hole"
            );
        }
    }
}

/// What [`AgentLedger::perform_rotate`] did, so the caller does not have to
/// re-read "which key was active before" and risk reporting a different answer
/// than the one the chain recorded.
#[derive(Debug, Clone)]
pub struct Rotation {
    pub identity: AgentIdentityRow,
    pub previous_fingerprint: Option<String>,
}

/// One unit of work for the single writer.
///
/// Everything that touches a chain goes through this queue, including the key
/// lifecycle: `(read head → hash → sign → insert → advance anchor)` and
/// `(mint → declare → activate)` must both be uninterleaved, and one writer is
/// how that is bought. The acknowledged variants exist because their failure is
/// not something a caller may assume away — see the module doc.
enum LedgerJob {
    /// Fire-and-forget append from a hot path.
    Append(NewRecord),
    Rotate {
        agent_id: String,
        ack: oneshot::Sender<Result<Rotation, KeyError>>,
    },
    Revoke {
        agent_id: String,
        ack: oneshot::Sender<Result<Option<String>, KeyError>>,
    },
    /// A barrier: acknowledged once every job queued ahead of it is written.
    Flush(oneshot::Sender<()>),
}

/// Why a ledger command could not be carried out.
///
/// Distinguishes "nothing is recording" from "the write failed", because they
/// call for opposite responses: the first means this build has no ledger wired
/// at all, the second means the chain has a hole.
#[derive(Debug, thiserror::Error)]
pub enum LedgerCommandError {
    #[error(
        "the agent identity ledger is not installed in this process, so key lifecycle \
         changes cannot be recorded. It is installed by `aleph-server start`."
    )]
    NotInstalled,
    #[error("the agent ledger writer is no longer running — nothing was changed")]
    WriterGone,
    #[error("{0}")]
    Key(#[from] KeyError),
}

static LEDGER: OnceLock<Arc<AgentLedger>> = OnceLock::new();
static WRITER: OnceLock<mpsc::Sender<LedgerJob>> = OnceLock::new();

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
    let (tx, mut rx) = mpsc::channel::<LedgerJob>(LEDGER_QUEUE);
    if LEDGER.set(ledger.clone()).is_err() || WRITER.set(tx).is_err() {
        return None;
    }
    Some(tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            match job {
                LedgerJob::Append(new) => {
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
                // The acknowledged pair. `note_lost` is raised inside
                // `perform_*`, at the exact step that failed, so a rotation
                // whose declaration landed but whose activation did not is not
                // miscounted as a lost record.
                LedgerJob::Rotate { agent_id, ack } => {
                    let _ = ack.send(ledger.perform_rotate(&agent_id));
                }
                LedgerJob::Revoke { agent_id, ack } => {
                    let _ = ack.send(ledger.perform_revoke(&agent_id));
                }
                LedgerJob::Flush(ack) => {
                    let _ = ack.send(());
                }
            }
        }
    }))
}

/// Hand one job to the writer and wait for its verdict.
async fn submit<T>(
    job: impl FnOnce(oneshot::Sender<Result<T, KeyError>>) -> LedgerJob,
) -> Result<T, LedgerCommandError> {
    let tx = WRITER.get().ok_or(LedgerCommandError::NotInstalled)?;
    let (ack, done) = oneshot::channel();
    tx.send(job(ack))
        .await
        .map_err(|_| LedgerCommandError::WriterGone)?;
    done.await
        .map_err(|_| LedgerCommandError::WriterGone)?
        .map_err(LedgerCommandError::Key)
}

/// Replace an agent's signing key **and** record that on its own chain, or fail
/// saying so.
///
/// Awaited rather than enqueued: the declaration is what keeps the incoming key
/// from faulting every record it signs, so "probably written" is not a state a
/// caller may report success from.
pub async fn rotate_identity(agent_id: &str) -> Result<Rotation, LedgerCommandError> {
    let agent_id = agent_id.to_string();
    submit(|ack| LedgerJob::Rotate { agent_id, ack }).await
}

/// Revoke an agent's identity **and** record that on its own chain.
///
/// Resolves to the retired fingerprint, or `None` when there was no live
/// identity to revoke.
pub async fn revoke_identity(agent_id: &str) -> Result<Option<String>, LedgerCommandError> {
    let agent_id = agent_id.to_string();
    submit(|ack| LedgerJob::Revoke { agent_id, ack }).await
}

/// Wait until every record enqueued before this call has been written.
///
/// The queue is FIFO and there is exactly one writer, so acknowledging this
/// barrier means everything ahead of it is on disk. Called on the graceful
/// shutdown path: exiting with records still queued loses them *and* the count
/// of them, because nothing failed — it simply never ran, which is the one hole
/// no chain check and no loss counter can reveal.
///
/// `false` when no ledger is installed or the writer is already gone; callers
/// impose their own deadline rather than one being chosen here, because how long
/// a shutdown may wait is not this module's decision.
pub async fn flush() -> bool {
    let Some(tx) = WRITER.get() else {
        return false;
    };
    let (ack, done) = oneshot::channel();
    tx.send(LedgerJob::Flush(ack)).await.is_ok() && done.await.is_ok()
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
    if let Err(e) = tx.send(LedgerJob::Append(new)).await {
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

    /// Every chain opens with a signed `identity_created` row, so a caller's
    /// Nth append lands at seq N+1. Named so the offset reads as the deliberate
    /// thing it is rather than an off-by-one.
    const GENESIS_ROWS: usize = 1;

    #[test]
    fn a_new_chain_opens_with_a_signed_identity_record() {
        // `agent_keys.retired_at` / `agent_identities.revoked_at` are ordinary
        // mutable columns. Stating the owning identity and its key inside the
        // chain is what makes that history tamper-evident on the same terms as
        // everything else in it.
        let (l, _s, _d) = ledger();
        let first_action = l.append(&entry("main", "bash")).unwrap();
        assert_eq!(first_action.seq, 2, "the caller's record follows genesis");

        let chain = l.keys().store().ledger_chain("main").unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].seq, 1);
        assert_eq!(chain[0].action, LedgerAction::IdentityCreated);
        assert!(chain[0].prev_hash.is_none());
        assert_eq!(chain[0].target, chain[0].signer_fp, "names the opening key");
        assert!(l.verify("main").unwrap().ok);
    }

    #[test]
    fn genesis_is_written_once_per_chain() {
        let (l, _s, _d) = ledger();
        for i in 0..3 {
            l.append(&entry("main", &format!("t{i}"))).unwrap();
        }
        let opens = l
            .keys()
            .store()
            .ledger_chain("main")
            .unwrap()
            .into_iter()
            .filter(|r| r.action == LedgerAction::IdentityCreated)
            .count();
        assert_eq!(opens, 1);
    }

    #[test]
    fn appends_form_a_verifiable_chain() {
        let (l, _s, _d) = ledger();
        for i in 0..5 {
            l.append(&entry("main", &format!("tool{i}"))).unwrap();
        }
        let report = l.verify("main").unwrap();
        assert!(report.ok, "clean chain must verify: {:?}", report.faults);
        assert_eq!(report.records, 5 + GENESIS_ROWS);
        assert_eq!(report.anchor_seq, 6);
        assert_eq!(report.last_seq, 6);
    }

    #[test]
    fn chains_are_per_agent_and_independent() {
        let (l, _s, _d) = ledger();
        l.append(&entry("main", "a")).unwrap();
        l.append(&entry("trader", "b")).unwrap();
        let second = l.append(&entry("main", "c")).unwrap();
        assert_eq!(second.seq, 3, "trader's append must not advance main's seq");
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
                "UPDATE agent_ledger SET target = 'harmless' WHERE agent_id='main' AND seq=2",
                [],
            )
            .unwrap();
        }

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::HashMismatch { seq: 2 }));
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
                "DELETE FROM agent_ledger WHERE agent_id='main' AND seq >= 4",
                [],
            )
            .unwrap();
        }

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::TailTruncated {
            anchor_seq: 5,
            last_seq: 3
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
                "DELETE FROM agent_ledger WHERE agent_id='main' AND seq >= 4",
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
                expected: 4,
                found: 6
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
            .contains(&ChainFault::ChainWiped { anchor_seq: 2 }));
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
                "UPDATE agent_ledger SET agent_id='main', seq=3, prev_hash=
                   (SELECT hash FROM agent_ledger WHERE agent_id='main' AND seq=2)
                 WHERE agent_id='trader' AND seq=2",
                [],
            )
            .unwrap();
        }
        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::HashMismatch { seq: 3 }));
        // And independently: the row names a key that was never main's.
        assert!(report
            .faults
            .iter()
            .any(|f| matches!(f, ChainFault::ForeignSigner { seq: 3, .. })));
    }

    #[test]
    fn a_row_signed_by_another_agents_key_is_detected() {
        // The narrow version of the transplant: the row is otherwise perfect —
        // correct agent, correct position, hash recomputed over the substituted
        // signer, signature genuinely produced by the named key. Only the fact
        // that the key belongs to `trader` is wrong. Without the ownership
        // check, "forging a record needs THIS agent's private key" would mean
        // "needs SOME agent's private key".
        let (l, store, _d) = ledger();
        l.append(&entry("main", "a")).unwrap();
        let trader = l.keys().ensure("trader").unwrap().active_fingerprint;

        let chain = store.ledger_chain("main").unwrap();
        let hash = super::super::hash::compute_hash(&super::super::hash::Preimage {
            signer_fp: &trader,
            ..super::super::verify::preimage_of(&chain[1])
        });
        let signature = l.keys().sign(&trader, &hash).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE agent_ledger SET signer_fp=?1, hash=?2, signature=?3
                 WHERE agent_id='main' AND seq=2",
                rusqlite::params![trader, hash.to_vec(), signature.to_vec()],
            )
            .unwrap();
        }

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert!(
            report.faults.iter().any(|f| matches!(
                f,
                ChainFault::ForeignSigner { seq: 2, owner, .. } if owner == "trader"
            )),
            "expected a foreign-signer fault, got {:?}",
            report.faults
        );
        assert!(
            !report.faults.contains(&ChainFault::BadSignature { seq: 2 }),
            "the signature itself is valid — only its owner is wrong"
        );
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
            seq: 3,
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
        assert!(report.faults.contains(&ChainFault::BadSignature { seq: 3 }));
    }

    #[test]
    fn records_signed_before_a_rotation_still_verify() {
        // Rotation as the only production path performs it: mint the key AND
        // make the chain declare it, in one writer-side operation. Both halves
        // are load-bearing — the declaration is what keeps `UndeclaredSigner`
        // off the ordinary rows the incoming key goes on to sign, which is the
        // case below and the one the neighbouring test does not reach (its last
        // row IS the rotation).
        let (l, _s, _d) = ledger();
        l.append(&entry("main", "before")).unwrap();
        let old = l
            .keys()
            .identity("main")
            .unwrap()
            .unwrap()
            .active_fingerprint;
        let rotation = l.perform_rotate("main").unwrap();
        assert_eq!(rotation.previous_fingerprint.as_deref(), Some(old.as_str()));
        l.append(&entry("main", "after")).unwrap();

        let report = l.verify("main").unwrap();
        assert!(
            report.ok,
            "rotation must not invalidate history: {:?}",
            report.faults
        );
        assert_eq!(report.records, 3 + GENESIS_ROWS);
    }

    #[test]
    fn a_key_the_chain_never_declares_faults_only_the_rows_it_signed() {
        // The tamper `UndeclaredSigner` exists for: `agent_identities` is a
        // single mutable row, so deleting it makes the next append mint a fresh
        // key and continue the chain under it with every link and every
        // signature intact. A rotation nobody records has the same shape and is
        // the cheapest way to write it down.
        //
        // The second half of the name is the part worth pinning: the undeclared
        // key indicts its own rows and nothing else. A check that also faulted
        // the history would tell an operator the whole chain is untrustworthy
        // when in fact everything up to the substitution still holds.
        let (l, _s, _d) = ledger();
        l.append(&entry("main", "before")).unwrap();
        // Mint and activate with nothing in between — the shape a caller
        // produces when it swaps the key and only then tries to say so.
        let new = l.keys().mint_key("main").unwrap();
        l.keys().activate("main", &new).unwrap();
        l.append(&entry("main", "after")).unwrap();

        let report = l.verify("main").unwrap();
        assert!(!report.ok);
        assert_eq!(
            report.faults,
            vec![ChainFault::UndeclaredSigner {
                seq: 3,
                fingerprint: new
            }],
            "the substitution is the only fault: rows 1-2 predate it"
        );
    }

    #[test]
    fn a_rotation_recorded_in_the_chain_verifies_under_the_new_key() {
        // The lifecycle record `perform_rotate` writes. It is signed by the
        // incoming key — the retired one no longer makes statements — and it
        // must not disturb the rows the outgoing key signed.
        let (l, _s, _d) = ledger();
        l.append(&entry("main", "before")).unwrap();
        let old = l
            .keys()
            .identity("main")
            .unwrap()
            .unwrap()
            .active_fingerprint;
        let new = l
            .perform_rotate("main")
            .unwrap()
            .identity
            .active_fingerprint;

        let report = l.verify("main").unwrap();
        assert!(report.ok, "{:?}", report.faults);
        let chain = l.keys().store().ledger_chain("main").unwrap();
        let rotation = chain.last().unwrap();
        assert_eq!(rotation.action, LedgerAction::IdentityRotated);
        assert_eq!(rotation.signer_fp, new);
        assert_eq!(chain[1].signer_fp, old, "history keeps its own signer");
    }

    #[test]
    fn a_revocation_is_recorded_and_signed_by_the_key_it_retires() {
        // The reason `signing_identity` tolerates a revoked agent: refusing to
        // sign here would delete the record of the revocation itself.
        let (l, _s, _d) = ledger();
        let fp = l.keys().ensure("main").unwrap().active_fingerprint;
        assert_eq!(
            l.perform_revoke("main").unwrap().as_deref(),
            Some(fp.as_str())
        );
        assert!(l
            .keys()
            .identity("main")
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some());

        let report = l.verify("main").unwrap();
        assert!(report.ok, "{:?}", report.faults);
        let chain = l.keys().store().ledger_chain("main").unwrap();
        assert_eq!(chain.last().unwrap().action, LedgerAction::IdentityRevoked);
        assert_eq!(chain.last().unwrap().signer_fp, fp);
        // Revoking again changes nothing and adds no second statement.
        assert_eq!(l.perform_revoke("main").unwrap(), None);
        assert_eq!(
            l.keys().store().ledger_chain("main").unwrap().len(),
            chain.len()
        );
    }

    #[test]
    fn a_rotation_that_cannot_be_declared_leaves_the_outgoing_key_in_charge() {
        // The failure the mint → declare → activate order exists for. With the
        // ledger table gone, the declaration cannot be written; the rotation
        // must then leave NOTHING behind that faults, rather than an active key
        // the chain has never heard of.
        let (l, store, _d) = ledger();
        l.append(&entry("main", "before")).unwrap();
        let before = l.keys().identity("main").unwrap().unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute("DROP TABLE agent_ledger", []).unwrap();
        }

        assert!(l.perform_rotate("main").is_err());

        let after = l.keys().identity("main").unwrap().unwrap();
        assert_eq!(
            after.active_fingerprint, before.active_fingerprint,
            "an undeclarable key must never become the active one"
        );
        assert_eq!(l.lost(), 1, "the missing declaration is counted");
    }

    #[test]
    fn the_loss_counter_outlives_the_process_that_lost_the_record() {
        // The offline verifier runs in a different process — precisely when the
        // writing one is not trusted — so a process-local counter would always
        // read zero exactly where it matters most. It also survives a restart.
        let (l, store, dir) = ledger();
        assert_eq!(l.lost(), 0);
        store.ledger_note_lost().unwrap();
        store.ledger_note_lost().unwrap();
        assert_eq!(l.lost(), 2);

        let reopened = AgentLedger::new(Arc::new(AgentKeystore::new(
            store.clone(),
            Arc::new(SharedTokenManager::new(
                store.clone(),
                dir.path().join("t.vault"),
            )),
        )));
        assert_eq!(
            reopened.lost(),
            2,
            "a fresh reader must still see the holes"
        );
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
