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

use std::collections::HashMap;

use serde::Serialize;

use super::hash::{compute_hash, Preimage};
use super::keystore::{AgentKeystore, KeyError};
use super::record::LedgerRecord;
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
}

/// What checking one row's signer found.
enum Signer {
    Valid,
    BadSignature,
    /// A key this installation minted — for somebody else.
    Foreign {
        owner: String,
    },
    /// A key this installation has never minted.
    Unknown,
}

/// The keys a chain's rows may legitimately name, resolved once per
/// verification instead of once per row.
///
/// A chain of N rows names at most a handful of distinct signers — one per
/// rotation. Looking the key up per row (what the reference implementation
/// does, and what this did) pays N locked database round trips to answer K
/// distinct questions, and on a long chain that dominates the cost of
/// verification by an order of magnitude over the signature checks themselves.
struct Keyring {
    /// Every key this agent has ever held, loaded in one query.
    own: HashMap<String, Vec<u8>>,
    /// Fingerprints found outside `own`: their real owner, or `None` when this
    /// installation never minted them. Memoised, so a repeated foreign signer
    /// costs one lookup rather than one per row.
    others: HashMap<String, Option<String>>,
}

impl Keyring {
    fn load(keys: &AgentKeystore, agent_id: &str) -> Result<Self, KeyError> {
        Ok(Self {
            own: keys
                .keys_of(agent_id)?
                .into_iter()
                .map(|k| (k.fingerprint, k.public_key))
                .collect(),
            others: HashMap::new(),
        })
    }

    /// Verify `signature` over `message` against the key the row names, and
    /// report whether that key was even this agent's to sign with.
    ///
    /// Retired keys count as the agent's own: records signed before a rotation
    /// must stay verifiable, which is the reason keys are never deleted.
    fn check(
        &mut self,
        keys: &AgentKeystore,
        fingerprint: &str,
        message: &[u8],
        signature: &[u8],
    ) -> Result<Signer, KeyError> {
        if let Some(public_key) = self.own.get(fingerprint) {
            return Ok(
                if verify_signature(public_key, message, signature).is_ok() {
                    Signer::Valid
                } else {
                    Signer::BadSignature
                },
            );
        }
        if !self.others.contains_key(fingerprint) {
            let owner = keys.store().get_agent_key(fingerprint)?.map(|k| k.agent_id);
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
    }
}

/// Verify one agent's chain end to end.
///
/// Reports **every** fault rather than stopping at the first: an operator
/// deciding what happened needs the shape of the damage, not just its
/// existence.
pub fn verify_chain(keys: &AgentKeystore, agent_id: &str) -> Result<ChainReport, KeyError> {
    let identity = keys
        .identity(agent_id)?
        .ok_or_else(|| KeyError::UnknownAgent(agent_id.to_string()))?;
    let rows = keys.store().ledger_chain(agent_id)?;

    let mut faults = Vec::new();

    let Some(first) = rows.first() else {
        if identity.head_seq > 0 {
            faults.push(ChainFault::ChainWiped {
                anchor_seq: identity.head_seq,
            });
        }
        return Ok(ChainReport {
            agent_id: agent_id.to_string(),
            ok: faults.is_empty(),
            records: 0,
            anchor_seq: identity.head_seq,
            last_seq: 0,
            faults,
        });
    };

    if first.seq != 1 {
        faults.push(ChainFault::PrefixMissing {
            first_seq: first.seq,
        });
    } else if first.prev_hash.is_some() {
        faults.push(ChainFault::GenesisNotNull { seq: first.seq });
    }

    let mut keyring = Keyring::load(keys, agent_id)?;
    let mut expected_seq = first.seq;
    let mut prev: Option<&[u8]> = None;

    for r in &rows {
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
        match keyring.check(keys, &r.signer_fp, &r.hash, &r.signature)? {
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

        prev = Some(&r.hash);
    }

    let last = rows.last().unwrap_or(first);
    if last.seq < identity.head_seq {
        faults.push(ChainFault::TailTruncated {
            anchor_seq: identity.head_seq,
            last_seq: last.seq,
        });
    } else if last.seq == identity.head_seq
        && identity.head_hash.as_deref() != Some(last.hash.as_slice())
    {
        faults.push(ChainFault::AnchorMismatch { seq: last.seq });
    }

    Ok(ChainReport {
        agent_id: agent_id.to_string(),
        ok: faults.is_empty(),
        records: rows.len(),
        anchor_seq: identity.head_seq,
        last_seq: last.seq,
        faults,
    })
}
