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
//! * **Head** ([`PinnedHead`], reported as [`HeadPin`]) — pin the previous
//!   export's `last_seq` / `last_hash` and the next document must extend it.
//!   This is the only thing that catches a **truncated tail**, because the
//!   anchor travels inside the document and an adversary edits it as freely as
//!   the rows. It is *checked*, not merely printed: naming the one defence
//!   against truncation and then leaving an auditor to compare two hex strings
//!   by eye is how a documented guarantee turns out never to have been
//!   exercised.
//!
//! Stated plainly because the alternative is an export that reads like proof
//! and is not one.
//!
//! ## The document's own signature
//!
//! Pins are values copied off-box by a diligent operator; nothing forced that
//! to happen, and until it does the document carried no integrity of its own.
//! So the document now also carries a signature over itself
//! ([`ExportSignature`]), made by the agent's active key at export time —
//! buzz's ref-state events are signed for the same reason. It proves the
//! document's *content* has not been altered since a process holding the
//! agent's private key emitted it: edit any record, key or anchor and the
//! envelope no longer verifies.
//!
//! It does **not** replace the pins, and the report keeps the two verdicts
//! separate. Whoever owns the machine holds the private key, so a local
//! adversary can re-sign a fabricated document — the envelope is
//! tamper-evidence for the document *in transit*, not trust in its origin.
//! What it adds over an unsigned export is that forgery now requires the
//! private key, not just write access to the file; combined with a pinned root
//! fingerprint, an off-box auditor can reject a fabricated chain without
//! trusting anything but the pin. And an unsigned document is not an error:
//! exports written before signing existed still verify, with the absence
//! reported plainly.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::keystore::KeyError;
use super::ledger::AgentLedger;
use super::record::{LedgerAction, LedgerOutcome, LedgerRecord};
use super::verify::{
    check_against, revoked_per_chain, walk_chain, Anchor, ChainFault, Signer, SignerSource,
};

/// Format tag. Bumping it is how a future reader knows the shape changed;
/// [`verify_export`] refuses anything else rather than guessing.
pub const EXPORT_FORMAT: &str = "aleph-agent-chain-v1";

/// Domain separator leading the export-signature preimage, in the style of
/// [`super::hash`]'s `DOMAIN`: a signature made here must never verify against
/// a coincidentally-identical byte run produced by another subsystem. Bump the
/// suffix only alongside a format migration — changing it invalidates every
/// signed export in flight.
const EXPORT_SIGNATURE_DOMAIN: &[u8] = b"aleph-agent-chain-export-v1";

/// The only signature scheme this module emits and accepts. Named in the
/// document so a second scheme later is an explicit format decision, not a
/// silent reinterpretation of existing bytes.
const EXPORT_SIGNATURE_SCHEME: &str = "ed25519";

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("not an Aleph agent chain export (format = {0:?}, expected {EXPORT_FORMAT:?})")]
    UnknownFormat(String),
    #[error("export names no agent")]
    NoAgent,
    #[error("malformed {field} in the export: {detail}")]
    Malformed { field: String, detail: String },
    #[error("a pinned head must be written as <seq>:<hash>, e.g. 42:9f3c… — got {0:?}")]
    BadHeadPin(String),
    #[error("{0}")]
    Key(#[from] KeyError),
}

/// A chain head taken off-box from a previous export: "at sequence N the chain
/// hashed to H".
///
/// The document's own anchor cannot do this job. It travels *inside* the file,
/// so whoever produced the file edits it as freely as the rows — which is why
/// tail truncation is invisible to an export checked on its own terms. A head
/// copied out at the time of the previous export is the one value an adversary
/// cannot reach, and pinning it is the only thing that makes "this export
/// extends the one I saw last time" a checkable statement rather than an
/// instruction to compare two hex strings by eye.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedHead {
    pub seq: i64,
    /// Hex, compared case-insensitively.
    pub hash: String,
}

impl PinnedHead {
    /// Parse the `<seq>:<hash>` form the CLI accepts and the tool emits.
    ///
    /// Rejects rather than guesses: a head pin silently misread as "no pin" is
    /// the failure this whole mechanism exists to prevent, and it would look
    /// exactly like success.
    pub fn parse(s: &str) -> Result<Self, ExportError> {
        let bad = || ExportError::BadHeadPin(s.to_string());
        let (seq, hash) = s.split_once(':').ok_or_else(bad)?;
        let seq: i64 = seq.trim().parse().map_err(|_| bad())?;
        let hash = hash.trim();
        if seq < 1 || hash.is_empty() || hex::decode(hash).is_err() {
            return Err(bad());
        }
        Ok(Self {
            seq,
            hash: hash.to_string(),
        })
    }
}

/// The out-of-band values an auditor brings to a verification. Empty means
/// "check internal consistency only", which is worth saying out loud every time
/// — see the module doc.
#[derive(Debug, Clone, Default)]
pub struct ExportPins {
    /// Root fingerprints previously taken off-box. Any match counts.
    pub roots: Vec<String>,
    /// The head of the previous export.
    pub head: Option<PinnedHead>,
}

impl ExportPins {
    /// Root fingerprints only — the common shape, and the only one with more
    /// than one caller.
    #[must_use]
    pub fn roots(roots: Vec<String>) -> Self {
        Self { roots, head: None }
    }
}

/// What a pinned head found. Every variant names the pinned sequence, so an
/// operator can go straight to the row that should have been there.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HeadPin {
    /// The pinned row is present at its pinned hash: this document is the same
    /// chain, carried forward by `added` further records.
    Extends { pinned_seq: i64, added: usize },
    /// The document ends below the pinned head — records that demonstrably
    /// existed are gone. **This is the truncation an export cannot otherwise
    /// reveal.**
    Truncated { pinned_seq: i64, last_seq: i64 },
    /// The row at the pinned sequence carries a different hash: history at or
    /// below the pin was rewritten, or this is a different chain entirely.
    Diverged { pinned_seq: i64 },
}

impl HeadPin {
    #[must_use]
    pub const fn holds(&self) -> bool {
        matches!(self, Self::Extends { .. })
    }

    fn check(rows: &[LedgerRecord], pin: &PinnedHead) -> Self {
        let last_seq = rows.last().map_or(0, |r| r.seq);
        let Some(row) = rows.iter().find(|r| r.seq == pin.seq) else {
            return Self::Truncated {
                pinned_seq: pin.seq,
                last_seq,
            };
        };
        if !hex::encode(&row.hash).eq_ignore_ascii_case(&pin.hash) {
            return Self::Diverged {
                pinned_seq: pin.seq,
            };
        }
        Self::Extends {
            pinned_seq: pin.seq,
            added: usize::try_from(last_seq - pin.seq).unwrap_or(0),
        }
    }
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
    /// The person the row names, when it names one. `#[serde(default)]` is
    /// what lets a document exported before this field existed still parse —
    /// and it lands as `None`, which is also how the preimage treats it, so an
    /// older export verifies with the same digests it was signed under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
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
            principal: r.principal.clone(),
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
            principal: self.principal.clone(),
        })
    }
}

/// The document's signature over itself — see the module doc.
///
/// Covers the canonical JSON of the document **with this field removed**
/// ([`export_preimage`]), so the envelope never has to sign its own bytes.
/// `default` + `skip_serializing_if` keeps documents exported before signing
/// existed parseable — they land as `None` and verify with the same verdicts
/// they always did, the absence reported rather than smoothed over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSignature {
    /// Always [`EXPORT_SIGNATURE_SCHEME`] today; anything else is malformed,
    /// not a negotiation.
    pub scheme: String,
    /// Fingerprint of the agent's active key at export time. Resolved against
    /// the keys embedded in the document — a fingerprint the document does not
    /// carry cannot verify.
    pub signer_fp: String,
    /// Ed25519 signature over the preimage, hex.
    pub sig: String,
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
    /// The producing installation had records for this agent but **no
    /// `agent_identities` row**.
    ///
    /// Carried because the live verifier reports that as
    /// [`ChainFault::IdentityMissing`] and an export must not be the one surface
    /// where it disappears: with no identity row there is no anchor, the
    /// `anchor_seq` below is a zero standing in for a value nobody holds, and a
    /// document that simply omitted the fact verified clean — laundering, in the
    /// act of exporting, exactly the tamper that removes an agent from every
    /// listing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub identity_row_missing: bool,
    pub keys: Vec<ExportedKey>,
    pub records: Vec<ExportedRecord>,
    /// Appends this installation is known to have **lost**. Travels with the
    /// document because a chain says nothing about records that were never
    /// written, so a clean verdict must never be read alone.
    pub failed_appends: u64,
    /// The document's signature over everything above. `None` for documents
    /// exported before signing existed and for chains whose identity row is
    /// gone (there is no active key to sign with, and minting one at export
    /// time would recreate the very row whose deletion is the attack).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<ExportSignature>,
}

/// The bytes the export signature commits to: the domain tag, then the
/// canonical JSON of the document with the `signature` field removed.
///
/// Canonical here means *parse, drop the envelope, re-serialize*: this
/// workspace's `serde_json` has no `preserve_order`, so the map is a BTreeMap
/// and the compact `to_vec` of a parsed document is byte-deterministic
/// regardless of the file's key order or whitespace. The signature therefore
/// covers the document's **content**, not its formatting — a re-prettified or
/// key-reordered file still verifies, and only a changed value moves the
/// preimage.
fn export_preimage(doc: &ChainExport) -> Result<Vec<u8>, ExportError> {
    let mut value = serde_json::to_value(doc)
        .map_err(|e| ExportError::malformed("signature", format!("cannot render: {e}")))?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("signature");
    }
    let canonical = serde_json::to_vec(&value)
        .map_err(|e| ExportError::malformed("signature", format!("cannot render: {e}")))?;
    let mut preimage = Vec::with_capacity(EXPORT_SIGNATURE_DOMAIN.len() + canonical.len());
    preimage.extend_from_slice(EXPORT_SIGNATURE_DOMAIN);
    preimage.extend_from_slice(&canonical);
    Ok(preimage)
}

/// Build the export for one agent.
///
/// Includes the whole chain, not a window: the prefix and the opening
/// `IdentityCreated` are what let a verifier see where the chain began and
/// which key it began under, and a segment starting mid-chain can prove
/// neither.
///
/// The document is signed with the agent's active key when there is one. An
/// orphan chain — records but no identity row — exports with
/// `signature: None`: there is no active key to name, and resolving one
/// through `signing_identity` would **mint a fresh identity row**, recreating
/// at export time the very row whose deletion is the tamper the export exists
/// to reveal. A signing failure (vault unreadable, key material gone) fails
/// the export outright rather than silently degrading it to unsigned — the
/// difference between "cannot sign" and "nothing to sign with" is exactly the
/// kind of fact that must not be laundered.
pub fn export_chain(ledger: &AgentLedger, agent_id: &str) -> Result<ChainExport, ExportError> {
    let keys = ledger.keys();
    let identity = keys.identity(agent_id)?;
    let records = keys
        .store()
        .ledger_chain(agent_id)
        .map_err(KeyError::from)?;
    if identity.is_none() && records.is_empty() {
        return Err(KeyError::UnknownAgent(agent_id.to_string()).into());
    }

    let mut doc = ChainExport {
        format: EXPORT_FORMAT.to_string(),
        agent_id: agent_id.to_string(),
        exported_at_ms: crate::session::events::now_ms(),
        anchor_seq: identity.as_ref().map_or(0, |i| i.head_seq),
        anchor_hash: identity
            .as_ref()
            .and_then(|i| i.head_hash.as_deref())
            .map(hex::encode),
        revoked_at: identity.as_ref().and_then(|i| i.revoked_at),
        identity_row_missing: identity.is_none(),
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
        signature: None,
    };

    if let Some(identity) = &identity {
        // A revoked agent signs with its retired key: `sign` loads by
        // fingerprint and retired keys stay decryptable, which is the point of
        // keeping them.
        let sig = keys.sign(&identity.active_fingerprint, &export_preimage(&doc)?)?;
        doc.signature = Some(ExportSignature {
            scheme: EXPORT_SIGNATURE_SCHEME.to_string(),
            signer_fp: identity.active_fingerprint.clone(),
            sig: hex::encode(sig),
        });
    }
    Ok(doc)
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
    /// `None` when no head was pinned. Anything but
    /// [`HeadPin::Extends`] means this document is not a continuation of the one
    /// the head came from — the only way an export can be caught having lost its
    /// tail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_pin: Option<HeadPin>,
    /// `revoked_at` as the producing installation's mutable identity row held
    /// it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
    /// The same question answered by the chain's own lifecycle records. `None`
    /// when the chain makes no such statement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_in_chain: Option<bool>,
    /// Carried through from the document — records lost before they were ever
    /// written are invisible to every chain check.
    pub failed_appends: u64,
    /// The verdict on the document's own signature envelope:
    ///
    /// * `None` — the document is **unsigned** (everything exported before
    ///   signing existed, and every orphan-chain export). `ok` tolerates this
    ///   for backward compatibility, but the absence is stated here rather
    ///   than smoothed over, and the CLI prints it.
    /// * `Some(true)` — an envelope was present and verified against a key the
    ///   document itself carries.
    /// * `Some(false)` — an envelope was present and did NOT verify (bad
    ///   signature, or a signer fingerprint the document does not carry). This
    ///   flips `ok` to false.
    ///
    /// Deliberately a field of the report rather than a [`ChainFault`]: faults
    /// describe chain rows; this describes the wrapper around them, and
    /// shoehorning it in would fault a perfectly intact chain carried in a
    /// tampered envelope. Serialized even when `None` — an absent key would
    /// read, to whoever scans the JSON, like a check that was not run.
    pub signature: Option<bool>,
}

impl ExportReport {
    /// The document's mutable identity row and its chain disagree about
    /// revocation. See
    /// [`ChainReport::revocation_disagrees`](super::verify::ChainReport::revocation_disagrees)
    /// — same predicate, same reason it is reported rather than faulted.
    #[must_use]
    pub fn revocation_disagrees(&self) -> bool {
        self.revoked_in_chain
            .is_some_and(|in_chain| in_chain != self.revoked_at.is_some())
    }
}

const fn is_false(b: &bool) -> bool {
    !*b
}

/// Verify an exported chain using nothing but the document.
///
/// `pins` are the values previously taken off-box; empty means no pin check at
/// all. See the module doc for what each pin buys.
pub fn verify_export(doc: &ChainExport, pins: &ExportPins) -> Result<ExportReport, ExportError> {
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

    // No identity row means no anchor — the same `None` the live verifier walks
    // with, rather than a zero that would quietly satisfy every anchor check.
    let anchor = (!doc.identity_row_missing).then_some(Anchor {
        seq: doc.anchor_seq,
        hash: anchor_hash.as_deref(),
    });

    let mut faults = Vec::new();
    if doc.identity_row_missing {
        faults.push(ChainFault::IdentityMissing);
    }
    let mut keyring = ExportKeyring::load(doc)?;
    // `walk_chain` only touches the database through its `SignerSource`, and
    // this one never does — so the `KeyError` arm is unreachable here rather
    // than merely unlikely. Mapped rather than unwrapped all the same.
    faults.extend(
        walk_chain(&rows, anchor, &mut keyring)
            .map_err(|e| ExportError::malformed("records", e))?,
    );

    let root_fingerprint = rows.first().map(|r| r.signer_fp.clone());
    let root_pinned = (!pins.roots.is_empty()).then(|| {
        root_fingerprint
            .as_deref()
            .is_some_and(|root| pins.roots.iter().any(|p| p == root))
    });
    let head_pin = pins.head.as_ref().map(|h| HeadPin::check(&rows, h));

    // The document's own envelope, checked against the keys it carries — the
    // same `check_against` the row walk uses, so "valid" means one thing on
    // every surface. A signer the document does not carry is `Some(false)`:
    // off-box there is no way to learn whose key it was, which is precisely
    // what `Signer::Unknown` already means for rows.
    let signature = match &doc.signature {
        None => None,
        Some(envelope) => Some(check_envelope(doc, envelope, &keyring)?),
    };

    Ok(ExportReport {
        agent_id: doc.agent_id.clone(),
        ok: faults.is_empty()
            && root_pinned != Some(false)
            && head_pin.as_ref().is_none_or(HeadPin::holds)
            && signature != Some(false),
        records: rows.len(),
        first_seq: rows.first().map_or(0, |r| r.seq),
        last_seq: rows.last().map_or(0, |r| r.seq),
        last_hash: rows.last().map(|r| hex::encode(&r.hash)),
        root_fingerprint,
        keys: doc.keys.iter().map(|k| k.fingerprint.clone()).collect(),
        revoked_in_chain: revoked_per_chain(&rows),
        faults,
        root_pinned,
        head_pin,
        revoked_at: doc.revoked_at,
        failed_appends: doc.failed_appends,
        signature,
    })
}

/// Verify the document's signature envelope against its own embedded keys.
///
/// Malformed hex or an unknown scheme is an [`ExportError`], not a `false`:
/// those say the document cannot be read as written, whereas `false` says it
/// was read and the signature does not cover it.
fn check_envelope(
    doc: &ChainExport,
    envelope: &ExportSignature,
    keyring: &ExportKeyring,
) -> Result<bool, ExportError> {
    if envelope.scheme != EXPORT_SIGNATURE_SCHEME {
        return Err(ExportError::malformed(
            "signature.scheme",
            format!(
                "{:?} (expected {EXPORT_SIGNATURE_SCHEME:?})",
                envelope.scheme
            ),
        ));
    }
    let sig = hex::decode(&envelope.sig).map_err(|e| ExportError::malformed("signature.sig", e))?;
    let preimage = export_preimage(doc)?;
    Ok(keyring
        .keys
        .get(&envelope.signer_fp)
        .is_some_and(|pk| matches!(check_against(pk, &preimage, &sig), Signer::Valid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::shared_token::SharedTokenManager;
    use crate::gateway::security::store::SecurityStore;
    use crate::identity::keystore::AgentKeystore;
    use crate::identity::record::{LedgerOutcome, NewRecord};
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    const NO_PINS: ExportPins = ExportPins {
        roots: Vec::new(),
        head: None,
    };

    fn ledger() -> (AgentLedger, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store.clone(),
            dir.path().join("t.vault"),
        ));
        let _ = vault.generate_token();
        (
            AgentLedger::new(Arc::new(AgentKeystore::new(store, vault))),
            dir,
        )
    }

    fn append(l: &AgentLedger, agent: &str, target: &str) {
        l.append(&NewRecord {
            agent_id: agent.to_string(),
            action: LedgerAction::ToolCall,
            target: target.to_string(),
            outcome: LedgerOutcome::Ok,
            args_fp: Some("fp".into()),
            detail: format!("{target}: did a thing"),
            principal: None,
        })
        .unwrap();
    }

    #[test]
    fn a_fresh_export_is_signed_and_the_signature_verifies() {
        let (l, _d) = ledger();
        append(&l, "main", "bash");
        let doc = export_chain(&l, "main").unwrap();
        let envelope = doc.signature.as_ref().expect("exports are signed now");
        assert_eq!(envelope.scheme, EXPORT_SIGNATURE_SCHEME);
        assert_eq!(
            envelope.signer_fp, doc.records[0].signer_fp,
            "signed by the agent's (only) key"
        );

        let report = verify_export(&doc, &NO_PINS).unwrap();
        assert_eq!(report.signature, Some(true));
        assert!(report.ok, "{:?}", report.faults);
    }

    #[test]
    fn a_bit_flipped_signature_fails_the_report_without_faulting_the_chain() {
        let (l, _d) = ledger();
        append(&l, "main", "bash");
        let mut doc = export_chain(&l, "main").unwrap();
        let envelope = doc.signature.as_mut().unwrap();
        // Flip one hex character of the signature.
        let first = envelope.sig.remove(0);
        envelope.sig.insert(0, if first == '0' { '1' } else { '0' });

        let report = verify_export(&doc, &NO_PINS).unwrap();
        assert_eq!(report.signature, Some(false));
        assert!(!report.ok, "a bad envelope must flip the verdict");
        assert!(
            report.faults.is_empty(),
            "the chain itself is untouched — the envelope is not a ChainFault"
        );
    }

    #[test]
    fn editing_any_row_breaks_the_envelope_too() {
        // The envelope's job: document-level tampering is caught even by a
        // reader who never walks the chain.
        let (l, _d) = ledger();
        append(&l, "main", "bash");
        let mut doc = export_chain(&l, "main").unwrap();
        doc.records[1].target = "harmless".into();

        let report = verify_export(&doc, &NO_PINS).unwrap();
        assert_eq!(report.signature, Some(false));
        assert!(!report.ok);
        assert!(report.faults.contains(&ChainFault::HashMismatch { seq: 2 }));
    }

    #[test]
    fn an_envelope_naming_a_key_the_document_does_not_carry_fails() {
        let (l, _d) = ledger();
        append(&l, "main", "bash");
        let mut doc = export_chain(&l, "main").unwrap();
        doc.signature.as_mut().unwrap().signer_fp = "0".repeat(16);

        let report = verify_export(&doc, &NO_PINS).unwrap();
        assert_eq!(report.signature, Some(false));
        assert!(!report.ok);
    }

    #[test]
    fn an_unsigned_legacy_document_still_verifies_with_the_absence_reported() {
        // Backward compatibility, asserted by deleting the key from the JSON
        // rather than by trusting the reasoning: a document exported before
        // signing existed is simply a document without the field.
        let (l, _d) = ledger();
        append(&l, "main", "bash");
        let doc = export_chain(&l, "main").unwrap();
        let mut value = serde_json::to_value(&doc).unwrap();
        value.as_object_mut().unwrap().remove("signature");
        let legacy: ChainExport = serde_json::from_value(value).unwrap();

        let report = verify_export(&legacy, &NO_PINS).unwrap();
        assert_eq!(
            report.signature, None,
            "absence must be reported, not hidden"
        );
        assert!(report.ok, "unsigned is not a fault: {:?}", report.faults);
    }

    #[test]
    fn an_orphan_chain_exports_unsigned() {
        // Records but no identity row: there is no active key to sign with,
        // and minting one here would recreate the row whose deletion is the
        // attack. The export must say "unsigned", plainly.
        let (l, _d) = ledger();
        append(&l, "main", "bash");
        {
            let conn = l.keys().store().conn.lock().unwrap();
            conn.execute("DELETE FROM agent_identities WHERE agent_id='main'", [])
                .unwrap();
        }

        let doc = export_chain(&l, "main").unwrap();
        assert!(doc.signature.is_none());
        assert!(doc.identity_row_missing);

        let report = verify_export(&doc, &NO_PINS).unwrap();
        assert_eq!(report.signature, None);
        assert!(!report.ok, "the missing identity row is still faulted");
        assert!(report.faults.contains(&ChainFault::IdentityMissing));
    }

    #[test]
    fn reformatting_the_json_does_not_break_the_envelope() {
        // The signature covers the document's canonical CONTENT, not the
        // file's bytes: pretty-printing and key reordering must survive, or
        // every hand-off through an editor would read as tampering.
        let (l, _d) = ledger();
        append(&l, "main", "bash");
        let doc = export_chain(&l, "main").unwrap();
        let pretty = serde_json::to_string_pretty(&doc).unwrap();
        let parsed: ChainExport = serde_json::from_str(&pretty).unwrap();

        let report = verify_export(&parsed, &NO_PINS).unwrap();
        assert_eq!(report.signature, Some(true));
        assert!(report.ok, "{:?}", report.faults);
    }
}
