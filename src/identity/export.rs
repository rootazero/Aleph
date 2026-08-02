//! A portable, self-contained chain export — and its off-box verifier.
//!
//! Three surfaces already told the operator (and the model, through the tool's
//! `DESCRIPTION`) that public fingerprints could be exported and pinned, and
//! that an exported chain segment was checkable by someone who does not trust
//! the machine it came from. Nothing implemented it: there was no export, and
//! the only verifier read `security.db` directly, so it could only ever run
//! where the database — and therefore the adversary of the threat model — is.
//! This module is that claim's producer.
//!
//! ## What an export is
//!
//! One JSON document carrying an agent's whole chain, every public key that
//! chain names, and the anchor. No private key, no arguments (records store a
//! fingerprint of those, never the values), and nothing that needs Aleph to
//! read: [`verify_export`] runs against the document alone, on a machine with
//! no daemon, no database and no vault.
//!
//! ## What verifying one proves — and what it does not
//!
//! [`verify_export`] runs **the same walk as the live verifier**
//! ([`walk_chain`]), differing only in where public keys come from. So a clean
//! report means what a clean `verify` means: no row was edited, reordered,
//! deleted from the middle or signed by a key the chain does not itself
//! introduce.
//!
//! It does **not**, on its own, prove the chain is the real one. Whoever
//! produced the document also chose the keys inside it, so an adversary who
//! owns the machine can mint a fresh key and sign a fabricated chain that
//! verifies perfectly. Two out-of-band pins turn that around, and both are one
//! value copied once:
//!
//! * **Root fingerprint** (`pins`) — the key the chain opened under. Pin it the
//!   first time and no later export can substitute a different lineage:
//!   continuing the chain under a new key requires a recorded rotation
//!   *signed by the key it replaces*.
//! * **Head** (`last_seq` / `last_hash` in the report) — pin the previous
//!   export's head and the next one must extend it. This is the only thing that
//!   catches a **truncated tail**, because the anchor travels inside the
//!   document and an adversary edits it as freely as the rows.
//!
//! Stated plainly because the alternative is an export that reads like proof
//! and is not one.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::keystore::KeyError;
use super::ledger::AgentLedger;
use super::record::{LedgerAction, LedgerOutcome, LedgerRecord};
use super::verify::{check_against, walk_chain, Anchor, ChainFault, Signer, SignerSource};

/// Format tag. Bumping it is how a future reader knows the shape changed;
/// [`verify_export`] refuses anything else rather than guessing.
pub const EXPORT_FORMAT: &str = "aleph-agent-chain-v1";

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("not an Aleph agent chain export (format = {0:?}, expected {EXPORT_FORMAT:?})")]
    UnknownFormat(String),
    #[error("export names no agent")]
    NoAgent,
    #[error("malformed {field} in the export: {detail}")]
    Malformed { field: String, detail: String },
}

impl ExportError {
    fn malformed(field: &str, detail: impl std::fmt::Display) -> Self {
        Self::Malformed {
            field: field.to_string(),
            detail: detail.to_string(),
        }
    }
}

/// A public key the exported chain names. Retired keys are included — records
/// signed before a rotation can only be checked against the key that signed
/// them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedKey {
    pub fingerprint: String,
    /// Ed25519 public key, hex.
    pub public_key: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<i64>,
}

/// One chain row, wire form.
///
/// Carries no `agent_id`: the document names the agent once, and every row is
/// reconstructed under **that** name. A row lifted from another agent's chain
/// therefore fails the hash check on arrival — the agent id leads the preimage
/// — instead of needing a rule of its own.
///
/// Hand-written rather than `#[derive(Deserialize)]` on [`LedgerRecord`] so
/// that nothing which can be appended is ever deserializable. The provenance
/// fence on [`NewRecord`](super::record::NewRecord) is the same idea; keeping
/// the two forms distinct is what stops a supplied blob from drifting into the
/// writer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedRecord {
    pub seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
    pub hash: String,
    pub signature: String,
    pub signer_fp: String,
    pub action: String,
    pub target: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_fp: Option<String>,
    pub detail: String,
    pub at_ms: i64,
}

impl ExportedRecord {
    fn from_record(r: &LedgerRecord) -> Self {
        Self {
            seq: r.seq,
            prev_hash: r.prev_hash.as_deref().map(hex::encode),
            hash: hex::encode(&r.hash),
            signature: hex::encode(&r.signature),
            signer_fp: r.signer_fp.clone(),
            action: r.action.as_str().to_string(),
            target: r.target.clone(),
            outcome: r.outcome.as_str().to_string(),
            args_fp: r.args_fp.clone(),
            detail: r.detail.clone(),
            at_ms: r.at_ms,
        }
    }

    /// Rebuild the row exactly as the signer saw it. An unparseable action or
    /// outcome is an error, never a lenient default: a row the verifier cannot
    /// reconstruct byte for byte must not be reconstructed as something else,
    /// which would turn a corrupted row into a passing one.
    fn to_record(&self, agent_id: &str) -> Result<LedgerRecord, ExportError> {
        let de = |field: &str, s: &str| {
            hex::decode(s).map_err(|e| ExportError::malformed(field, format!("{s:?}: {e}")))
        };
        Ok(LedgerRecord {
            agent_id: agent_id.to_string(),
            seq: self.seq,
            prev_hash: self
                .prev_hash
                .as_deref()
                .map(|h| de("prev_hash", h))
                .transpose()?,
            hash: de("hash", &self.hash)?,
            signature: de("signature", &self.signature)?,
            signer_fp: self.signer_fp.clone(),
            action: self
                .action
                .parse::<LedgerAction>()
                .map_err(|e| ExportError::malformed("action", e))?,
            target: self.target.clone(),
            outcome: self
                .outcome
                .parse::<LedgerOutcome>()
                .map_err(|e| ExportError::malformed("outcome", e))?,
            args_fp: self.args_fp.clone(),
            detail: self.detail.clone(),
            at_ms: self.at_ms,
        })
    }
}

/// One agent's chain, its keys and its anchor, in a form that needs nothing
/// from this installation to check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainExport {
    pub format: String,
    pub agent_id: String,
    pub exported_at_ms: i64,
    /// The anchor as this installation held it. Travels for completeness, and
    /// is **not** evidence on its own — see the module doc on pinning the head.
    pub anchor_seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
    pub keys: Vec<ExportedKey>,
    pub records: Vec<ExportedRecord>,
    /// Appends this installation is known to have **lost**. Travels with the
    /// document because a chain says nothing about records that were never
    /// written, so a clean verdict must never be read alone.
    pub failed_appends: u64,
}

/// Build the export for one agent.
///
/// Includes the whole chain, not a window: the prefix and the opening
/// `IdentityCreated` are what let a verifier see where the chain began and
/// which key it began under, and a segment starting mid-chain can prove
/// neither.
pub fn export_chain(ledger: &AgentLedger, agent_id: &str) -> Result<ChainExport, KeyError> {
    let keys = ledger.keys();
    let identity = keys.identity(agent_id)?;
    let records = keys.store().ledger_chain(agent_id)?;
    if identity.is_none() && records.is_empty() {
        return Err(KeyError::UnknownAgent(agent_id.to_string()));
    }

    Ok(ChainExport {
        format: EXPORT_FORMAT.to_string(),
        agent_id: agent_id.to_string(),
        exported_at_ms: crate::session::events::now_ms(),
        anchor_seq: identity.as_ref().map_or(0, |i| i.head_seq),
        anchor_hash: identity
            .as_ref()
            .and_then(|i| i.head_hash.as_deref())
            .map(hex::encode),
        revoked_at: identity.as_ref().and_then(|i| i.revoked_at),
        keys: keys
            .keys_of(agent_id)?
            .iter()
            .map(|k| ExportedKey {
                fingerprint: k.fingerprint.clone(),
                public_key: hex::encode(&k.public_key),
                created_at: k.created_at,
                retired_at: k.retired_at,
            })
            .collect(),
        records: records.iter().map(ExportedRecord::from_record).collect(),
        failed_appends: ledger.lost(),
    })
}

/// Public keys resolved from a document instead of from `security.db`.
///
/// Every key in the export belongs to the agent the export names, so there is
/// no "foreign signer" to distinguish here — a fingerprint the document does
/// not carry is simply [`Signer::Unknown`], which is exactly the right verdict
/// off-box: the verifier has no way to learn whose it was, and a row it cannot
/// resolve a key for is unverifiable either way.
pub(super) struct ExportKeyring {
    keys: HashMap<String, Vec<u8>>,
}

impl ExportKeyring {
    fn load(doc: &ChainExport) -> Result<Self, ExportError> {
        let mut keys = HashMap::new();
        for k in &doc.keys {
            let bytes = hex::decode(&k.public_key).map_err(|e| {
                ExportError::malformed("public_key", format!("{}: {e}", k.fingerprint))
            })?;
            keys.insert(k.fingerprint.clone(), bytes);
        }
        Ok(Self { keys })
    }
}

impl SignerSource for ExportKeyring {
    fn check(
        &mut self,
        fingerprint: &str,
        message: &[u8],
        signature: &[u8],
    ) -> Result<Signer, KeyError> {
        Ok(self
            .keys
            .get(fingerprint)
            .map_or(Signer::Unknown, |pk| check_against(pk, message, signature)))
    }
}

/// The verdict on an exported chain.
#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub agent_id: String,
    /// `true` only when there are no faults **and** no supplied pin failed.
    pub ok: bool,
    pub records: usize,
    pub first_seq: i64,
    pub last_seq: i64,
    /// Hash of the last row, hex. Pin it and the next export must extend it —
    /// the only way an exported document can reveal a truncated tail.
    pub last_hash: Option<String>,
    /// The key the chain opened under: its root of trust. Pin this once and no
    /// later export can present a different lineage as this agent's.
    pub root_fingerprint: Option<String>,
    /// Every fingerprint carried by the document.
    pub keys: Vec<String>,
    pub faults: Vec<ChainFault>,
    /// `None` when no pin was supplied — in which case a clean report proves
    /// internal consistency only. `Some(false)` means the root is not one of
    /// the pinned fingerprints, i.e. this is not the chain you pinned.
    pub root_pinned: Option<bool>,
    /// Carried through from the document — records lost before they were ever
    /// written are invisible to every chain check.
    pub failed_appends: u64,
}

/// Verify an exported chain using nothing but the document.
///
/// `pins` are fingerprints previously taken off-box; empty means no pin check.
/// See the module doc for what each pin buys.
pub fn verify_export(doc: &ChainExport, pins: &[String]) -> Result<ExportReport, ExportError> {
    if doc.format != EXPORT_FORMAT {
        return Err(ExportError::UnknownFormat(doc.format.clone()));
    }
    if doc.agent_id.trim().is_empty() {
        return Err(ExportError::NoAgent);
    }

    let rows = doc
        .records
        .iter()
        .map(|r| r.to_record(&doc.agent_id))
        .collect::<Result<Vec<_>, _>>()?;
    let anchor_hash = doc
        .anchor_hash
        .as_deref()
        .map(|h| hex::decode(h).map_err(|e| ExportError::malformed("anchor_hash", e)))
        .transpose()?;

    let mut keyring = ExportKeyring::load(doc)?;
    // `walk_chain` only touches the database through its `SignerSource`, and
    // this one never does — so the `KeyError` arm is unreachable here rather
    // than merely unlikely. Mapped rather than unwrapped all the same.
    let faults = walk_chain(
        &rows,
        Some(Anchor {
            seq: doc.anchor_seq,
            hash: anchor_hash.as_deref(),
        }),
        &mut keyring,
    )
    .map_err(|e| ExportError::malformed("records", e))?;

    let root_fingerprint = rows.first().map(|r| r.signer_fp.clone());
    let root_pinned = if pins.is_empty() {
        None
    } else {
        Some(
            root_fingerprint
                .as_deref()
                .is_some_and(|root| pins.iter().any(|p| p == root)),
        )
    };

    Ok(ExportReport {
        agent_id: doc.agent_id.clone(),
        ok: faults.is_empty() && root_pinned != Some(false),
        records: rows.len(),
        first_seq: rows.first().map_or(0, |r| r.seq),
        last_seq: rows.last().map_or(0, |r| r.seq),
        last_hash: rows.last().map(|r| hex::encode(&r.hash)),
        root_fingerprint,
        keys: doc.keys.iter().map(|k| k.fingerprint.clone()).collect(),
        faults,
        root_pinned,
        failed_appends: doc.failed_appends,
    })
}
