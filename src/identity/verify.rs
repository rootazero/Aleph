//! Chain verification.
//!
//! The reference implementation this is modelled on detects in-place mutation,
//! reordering, mid-chain deletion and cross-tenant transplantation — and is
//! structurally blind to **tail truncation** and **whole-prefix deletion**,
//! because it starts from whatever row the caller asked about and never checks
//! that row against a predecessor or against a remembered head. A chain with
//! its last N rows removed is still internally consistent, so it verifies
//! clean.
//!
//! Both holes are closed here:
//!
//! * **Prefix deletion** — the chain must begin at `seq = 1` with a NULL
//!   `prev_hash`. Anything else means rows below the first surviving one are
//!   gone.
//! * **Tail truncation** — every append advances the anchor in
//!   `agent_identities`, and the next append is numbered above the anchor (see
//!   [`SecurityStore::ledger_next_position`](crate::gateway::security::store::SecurityStore::ledger_next_position)),
//!   so deleted rows leave a permanent sequence gap *and* a last-row-below-anchor
//!   mismatch.
//!
//! Signatures add what a keyless chain cannot have at all: re-chaining the
//! whole table after an edit requires the agent's private key, not just write
//! access to the rows — and the key must be **that agent's**. A signature is
//! only evidence about the identity it belongs to, so a row naming a key this
//! installation minted for somebody else is a fault
//! ([`ChainFault::ForeignSigner`]) even when the signature over it is
//! arithmetically valid. Without that check the guarantee degrades from "needs
//! this agent's private key" to "needs some agent's private key", and every
//! delegated role that signs its own work enlarges that set.
//!
//! A third hole, of the same family, was open until the chain was made to
//! declare its own key history: `agent_identities` is one mutable row, and
//! deleting it makes the next append **mint a fresh key and quietly continue
//! the chain under it** — every link valid, every signature valid, verdict
//! clean. [`ChainFault::UndeclaredSigner`] closes that by requiring every key a
//! chain is signed with to be introduced *by the chain itself* (the opening
//! `IdentityCreated`, or an `IdentityRotated` naming it). The check is
//! **set membership, not adjacency**, deliberately: records are enqueued
//! asynchronously, so an ordinary call issued just before a rotation can land
//! *after* it and be signed by the incoming key. Requiring the rotation record
//! to immediately precede the first row it covers would flag that race as
//! tampering.
//!
//! ## One walk, two key sources
//!
//! [`walk_chain`] is the whole algorithm; [`SignerSource`] is the only thing
//! that varies. The live path resolves keys from `security.db`
//! ([`Keyring`]); [`export`](super::export) resolves them from the keys
//! embedded in a portable document, so an exported chain is checked by
//! **exactly the same code** off-box. A second copy of a security check is how
//! the divergence between them gets shipped.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::hash::{compute_hash, Preimage};
use super::keystore::{AgentKeystore, KeyError};
use super::record::{LedgerAction, LedgerRecord};
use crate::gateway::security::crypto::verify_signature;

/// One thing wrong with a chain. Every variant names the sequence it concerns
/// so an operator can go straight to the row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChainFault {
    /// The row's contents do not reproduce its stored hash — it was edited.
    HashMismatch { seq: i64 },
    /// The row does not link to its predecessor's hash.
    ChainBroken { seq: i64 },
    /// The stored hash was not signed by the key the row names.
    BadSignature { seq: i64 },
    /// The signing key is not in `agent_keys` — the row claims a signer this
    /// installation has never minted.
    UnknownSigner { seq: i64, fingerprint: String },
    /// The row is signed by a key the **chain never introduces**: no opening
    /// `IdentityCreated` and no `IdentityRotated` names it. The key may well be
    /// this agent's — that is the point. Deleting the single mutable
    /// `agent_identities` row makes the next append mint a new key and continue
    /// the chain under it with every link and signature intact; only the
    /// chain's own account of its key history reveals it.
    UndeclaredSigner { seq: i64, fingerprint: String },
    /// The signing key exists but belongs to a **different agent**. Without
    /// this check, "forging a record needs *this* agent's private key" would
    /// really only mean "needs *some* agent's private key" — and every
    /// delegated role that now signs its own work widens that set.
    ForeignSigner {
        seq: i64,
        fingerprint: String,
        owner: String,
    },
    /// The chain does not start at 1: rows below `first_seq` were deleted.
    PrefixMissing { first_seq: i64 },
    /// A first entry carrying a predecessor link — the chain was re-based.
    GenesisNotNull { seq: i64 },
    /// Sequence numbers jump: rows in between were deleted.
    SeqGap { expected: i64, found: i64 },
    /// Rows exist below the anchor's remembered head — the tail was cut.
    TailTruncated { anchor_seq: i64, last_seq: i64 },
    /// The last row is at the anchor's sequence but not its hash.
    AnchorMismatch { seq: i64 },
    /// The anchor remembers a chain that has no rows at all.
    ChainWiped { anchor_seq: i64 },
    /// Records exist for an agent that has no `agent_identities` row: the
    /// anchor — the only thing that can reveal a truncated tail — is gone, and
    /// with it the agent's presence in every identity-driven listing.
    IdentityMissing,
}

/// The outcome of verifying one agent's chain.
#[derive(Debug, Clone, Serialize)]
pub struct ChainReport {
    pub agent_id: String,
    /// `true` only when `faults` is empty.
    pub ok: bool,
    pub records: usize,
    /// Highest sequence the anchor remembers.
    pub anchor_seq: i64,
    /// Highest sequence actually present.
    pub last_seq: i64,
    pub faults: Vec<ChainFault>,
    /// `revoked_at` as the mutable `agent_identities` row holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
    /// The same question, answered by the chain itself
    /// ([`revoked_per_chain`]). `None` when the chain makes no lifecycle
    /// statement at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_in_chain: Option<bool>,
}

impl ChainReport {
    /// The mutable column and the chain disagree about revocation.
    ///
    /// This is the executable form of "editing `revoked_at` back to NULL cannot
    /// erase a revocation" — a claim that was previously only ever *stated*,
    /// with nothing anywhere comparing the two. Deliberately **not** folded into
    /// [`Self::ok`]: the disagreement has a benign cause as well as a malicious
    /// one (a lifecycle record lost before it was written, which
    /// `failed_appends` counts), and a fault that cries wolf on a real
    /// installation stops being read.
    #[must_use]
    pub fn revocation_disagrees(&self) -> bool {
        self.revoked_in_chain
            .is_some_and(|in_chain| in_chain != self.revoked_at.is_some())
    }
}

/// What the chain's own lifecycle records say about whether this identity is
/// revoked: the last of them wins, because activating a key clears the mark
/// ([`SecurityStore::upsert_agent_identity`](crate::gateway::security::store::SecurityStore::upsert_agent_identity)).
///
/// `None` when the chain contains no lifecycle statement — a chain that has not
/// spoken is not the same as one that says "active", and reporting it as such
/// would invent a disagreement out of silence.
#[must_use]
pub fn revoked_per_chain(rows: &[LedgerRecord]) -> Option<bool> {
    rows.iter().rev().find_map(|r| match r.action {
        LedgerAction::IdentityRevoked => Some(true),
        LedgerAction::IdentityCreated | LedgerAction::IdentityRotated => Some(false),
        _ => None,
    })
}

/// What checking one row's signer found.
pub(super) enum Signer {
    Valid,
    BadSignature,
    /// A key this installation minted — for somebody else.
    Foreign {
        owner: String,
    },
    /// A key this installation has never minted.
    Unknown,
}

/// Where [`walk_chain`] resolves the public key a row names.
///
/// Two implementations, one algorithm: [`Keyring`] answers from `security.db`,
/// and [`ExportKeyring`](super::export::ExportKeyring) answers from the keys
/// carried inside a portable export — which is what lets a chain be verified on
/// a machine that has neither the database nor the daemon.
pub(super) trait SignerSource {
    /// Verify `signature` over `message` against the key `fingerprint` names,
    /// and report whether that key was even this agent's to sign with.
    fn check(
        &mut self,
        fingerprint: &str,
        message: &[u8],
        signature: &[u8],
    ) -> Result<Signer, KeyError>;
}

/// Check a signature against a resolved public key. The one place the
/// primitive is called during verification, so "valid" means the same thing on
/// both key sources.
pub(super) fn check_against(public_key: &[u8], message: &[u8], signature: &[u8]) -> Signer {
    if verify_signature(public_key, message, signature).is_ok() {
        Signer::Valid
    } else {
        Signer::BadSignature
    }
}

/// The keys a chain's rows may legitimately name, resolved once per
/// verification instead of once per row.
///
/// A chain of N rows names at most a handful of distinct signers — one per
/// rotation. Looking the key up per row (what the reference implementation
/// does, and what this did) pays N locked database round trips to answer K
/// distinct questions, and on a long chain that dominates the cost of
/// verification by an order of magnitude over the signature checks themselves.
struct Keyring<'a> {
    keys: &'a AgentKeystore,
    /// Every key this agent has ever held, loaded in one query.
    own: HashMap<String, Vec<u8>>,
    /// Fingerprints found outside `own`: their real owner, or `None` when this
    /// installation never minted them. Memoised, so a repeated foreign signer
    /// costs one lookup rather than one per row.
    others: HashMap<String, Option<String>>,
}

impl<'a> Keyring<'a> {
    fn load(keys: &'a AgentKeystore, agent_id: &str) -> Result<Self, KeyError> {
        Ok(Self {
            keys,
            own: keys
                .keys_of(agent_id)?
                .into_iter()
                .map(|k| (k.fingerprint, k.public_key))
                .collect(),
            others: HashMap::new(),
        })
    }
}

impl SignerSource for Keyring<'_> {
    /// Retired keys count as the agent's own: records signed before a rotation
    /// must stay verifiable, which is the reason keys are never deleted.
    fn check(
        &mut self,
        fingerprint: &str,
        message: &[u8],
        signature: &[u8],
    ) -> Result<Signer, KeyError> {
        if let Some(public_key) = self.own.get(fingerprint) {
            return Ok(check_against(public_key, message, signature));
        }
        if !self.others.contains_key(fingerprint) {
            let owner = self
                .keys
                .store()
                .get_agent_key(fingerprint)?
                .map(|k| k.agent_id);
            self.others.insert(fingerprint.to_string(), owner);
        }
        Ok(
            match self.others.get(fingerprint).and_then(Option::as_deref) {
                Some(owner) => Signer::Foreign {
                    owner: owner.to_string(),
                },
                None => Signer::Unknown,
            },
        )
    }
}

/// The remembered head of a chain, held outside it.
///
/// Separating "how far the chain got" from the chain itself is the only thing
/// that can reveal a truncated tail — a chain with its last rows removed is
/// internally consistent. `None` means no anchor is available at all (the
/// identity row is gone, or an export was produced without one), which is
/// itself reported rather than treated as "nothing to compare against".
#[derive(Clone, Copy)]
pub(super) struct Anchor<'a> {
    pub seq: i64,
    pub hash: Option<&'a [u8]>,
}

/// Every fingerprint the chain itself introduces: the key it opened under, plus
/// every key a recorded rotation names.
///
/// The first row's signer counts as introduced whatever its sequence, so a
/// chain whose genesis row was deleted reports that one fact
/// ([`ChainFault::PrefixMissing`]) instead of an undeclared-signer fault per
/// surviving row.
fn declared_signers(rows: &[LedgerRecord]) -> HashSet<&str> {
    let mut declared: HashSet<&str> = rows
        .iter()
        .filter(|r| {
            matches!(
                r.action,
                LedgerAction::IdentityCreated | LedgerAction::IdentityRotated
            )
        })
        .map(|r| r.target.as_str())
        .collect();
    if let Some(first) = rows.first() {
        declared.insert(first.signer_fp.as_str());
    }
    declared
}

/// Walk a chain end to end and report **every** fault: an operator deciding
/// what happened needs the shape of the damage, not just its existence.
///
/// Shared by the live verifier and the offline export verifier — see the module
/// doc. `rows` must be this agent's rows in ascending `seq` order.
pub(super) fn walk_chain(
    rows: &[LedgerRecord],
    anchor: Option<Anchor<'_>>,
    keys: &mut dyn SignerSource,
) -> Result<Vec<ChainFault>, KeyError> {
    let mut faults = Vec::new();

    let Some(first) = rows.first() else {
        if anchor.is_some_and(|a| a.seq > 0) {
            faults.push(ChainFault::ChainWiped {
                anchor_seq: anchor.map_or(0, |a| a.seq),
            });
        }
        return Ok(faults);
    };

    if first.seq != 1 {
        faults.push(ChainFault::PrefixMissing {
            first_seq: first.seq,
        });
    } else if first.prev_hash.is_some() {
        faults.push(ChainFault::GenesisNotNull { seq: first.seq });
    }

    let declared = declared_signers(rows);
    let mut expected_seq = first.seq;
    let mut prev: Option<&[u8]> = None;

    for r in rows {
        if r.seq != expected_seq {
            faults.push(ChainFault::SeqGap {
                expected: expected_seq,
                found: r.seq,
            });
            expected_seq = r.seq;
        }
        expected_seq += 1;

        if let Some(expected_prev) = prev {
            if r.prev_hash.as_deref() != Some(expected_prev) {
                faults.push(ChainFault::ChainBroken { seq: r.seq });
            }
        }

        if compute_hash(&preimage_of(r)).as_slice() != r.hash.as_slice() {
            faults.push(ChainFault::HashMismatch { seq: r.seq });
        }

        // Signed over the STORED hash: the signature proves the digest is
        // authentic, the recomputation above proves the row still matches it.
        // Both are needed — either alone is forgeable by editing the other.
        // And the key must be one THIS agent holds: a valid signature from
        // another agent's key attests to nothing about this chain.
        match keys.check(&r.signer_fp, &r.hash, &r.signature)? {
            Signer::Valid => {}
            Signer::BadSignature => faults.push(ChainFault::BadSignature { seq: r.seq }),
            Signer::Foreign { owner } => faults.push(ChainFault::ForeignSigner {
                seq: r.seq,
                fingerprint: r.signer_fp.clone(),
                owner,
            }),
            Signer::Unknown => faults.push(ChainFault::UnknownSigner {
                seq: r.seq,
                fingerprint: r.signer_fp.clone(),
            }),
        }

        // …and the chain must say where that key came from. Ownership alone is
        // satisfied by a key minted seconds ago to replace a deleted identity.
        if !declared.contains(r.signer_fp.as_str()) {
            faults.push(ChainFault::UndeclaredSigner {
                seq: r.seq,
                fingerprint: r.signer_fp.clone(),
            });
        }

        prev = Some(&r.hash);
    }

    let last = rows.last().unwrap_or(first);
    if let Some(anchor) = anchor {
        if last.seq < anchor.seq {
            faults.push(ChainFault::TailTruncated {
                anchor_seq: anchor.seq,
                last_seq: last.seq,
            });
        } else if last.seq == anchor.seq && anchor.hash != Some(last.hash.as_slice()) {
            faults.push(ChainFault::AnchorMismatch { seq: last.seq });
        }
    }

    Ok(faults)
}

pub(super) fn preimage_of(r: &LedgerRecord) -> Preimage<'_> {
    Preimage {
        agent_id: &r.agent_id,
        seq: r.seq,
        at_ms: r.at_ms,
        action: r.action,
        outcome: r.outcome,
        target: &r.target,
        args_fp: r.args_fp.as_deref(),
        detail: &r.detail,
        signer_fp: &r.signer_fp,
        prev_hash: r.prev_hash.as_deref(),
        principal: r.principal.as_deref(),
    }
}

/// Verify one agent's chain end to end.
///
/// An agent with records but **no identity row** is verified rather than
/// refused: the deleted row is reported as [`ChainFault::IdentityMissing`] and
/// the walk proceeds without an anchor. Erroring out here — what this did —
/// meant the one tamper that removes an agent from every listing also removed
/// it from the verifier, which is the failure this module exists to make
/// impossible. Only an agent with neither an identity nor a single record is
/// genuinely unknown.
pub(crate) fn verify_chain(keys: &AgentKeystore, agent_id: &str) -> Result<ChainReport, KeyError> {
    let identity = keys.identity(agent_id)?;
    let rows = keys.store().ledger_chain(agent_id)?;
    if identity.is_none() && rows.is_empty() {
        return Err(KeyError::UnknownAgent(agent_id.to_string()));
    }

    let mut faults = Vec::new();
    if identity.is_none() {
        faults.push(ChainFault::IdentityMissing);
    }
    let anchor = identity.as_ref().map(|i| Anchor {
        seq: i.head_seq,
        hash: i.head_hash.as_deref(),
    });

    let mut keyring = Keyring::load(keys, agent_id)?;
    faults.extend(walk_chain(&rows, anchor, &mut keyring)?);

    Ok(ChainReport {
        agent_id: agent_id.to_string(),
        ok: faults.is_empty(),
        records: rows.len(),
        revoked_in_chain: revoked_per_chain(&rows),
        anchor_seq: identity.as_ref().map_or(0, |i| i.head_seq),
        revoked_at: identity.and_then(|i| i.revoked_at),
        last_seq: rows.last().map_or(0, |r| r.seq),
        faults,
    })
}
