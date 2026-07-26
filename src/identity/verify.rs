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
//! access to the rows.

use serde::Serialize;

use super::hash::{compute_hash, Preimage};
use super::keystore::{AgentKeystore, KeyError};
use super::record::LedgerRecord;

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

fn preimage_of(r: &LedgerRecord) -> Preimage<'_> {
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
        match keys.verify(&r.signer_fp, &r.hash, &r.signature) {
            Ok(()) => {}
            Err(KeyError::MissingKey(fingerprint)) => {
                faults.push(ChainFault::UnknownSigner {
                    seq: r.seq,
                    fingerprint,
                });
            }
            Err(KeyError::Store(e)) => return Err(KeyError::Store(e)),
            Err(_) => faults.push(ChainFault::BadSignature { seq: r.seq }),
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
