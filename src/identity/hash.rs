//! The chain preimage.
//!
//! Three deliberate departures from the reference implementation this is
//! modelled on (buzz's `buzz-audit`), each closing a defect it has:
//!
//! 1. **Every variable-length field is length-prefixed.** buzz concatenates
//!    `presence-tag || bytes` for adjacent optional fields, so
//!    `Some([0x01, 0x01]) + None` and `Some([0x01]) + Some("\0")` produce the
//!    same fragment. Unreachable with its two current producers, but it is a
//!    real preimage-framing defect: two different records can hash the same.
//! 2. **The timestamp is hashed as an integer**, not as a rendered RFC-3339
//!    string. buzz hashes `created_at.to_rfc3339()`, whose sub-second digit
//!    count *follows the value* (chrono emits 0/3/6/9), so a nanosecond
//!    instant and its microsecond truncation are different strings — which is
//!    what made every one of its entries fail verification until it started
//!    truncating at both the write site and inside the hash function. An
//!    integer has one representation and no round-trip to lose.
//! 3. **A domain-separation tag leads the preimage**, so a digest computed
//!    here can never collide with one computed by another Aleph subsystem over
//!    a coincidentally-identical byte run.
//!
//! Retained from the reference: the identity (`agent_id`) leads the hash, so a
//! row lifted out of one agent's chain and inserted into another's recomputes
//! to a different digest and fails verification.

use sha2::{Digest, Sha256};

use super::record::{LedgerAction, LedgerOutcome};

/// Hashed in place of `prev_hash` for a chain's first entry (stored as NULL).
pub const GENESIS_HASH: [u8; 32] = [0u8; 32];

/// Domain separator. Bump the version suffix only alongside a migration —
/// changing it invalidates every existing chain.
const DOMAIN: &[u8] = b"aleph-agent-ledger-v1";

/// The fields a chain digest commits to. Borrowed rather than owned so the
/// writer can hash before it has built the final record.
pub struct Preimage<'a> {
    pub agent_id: &'a str,
    pub seq: i64,
    pub at_ms: i64,
    pub action: LedgerAction,
    pub outcome: LedgerOutcome,
    pub target: &'a str,
    pub args_fp: Option<&'a str>,
    pub detail: &'a str,
    pub signer_fp: &'a str,
    pub prev_hash: Option<&'a [u8]>,
}

/// Length-prefixed field: `u32` big-endian length, then the bytes.
///
/// This is what makes the concatenation unambiguous — no two distinct field
/// tuples can produce the same byte run.
fn lp(hasher: &mut Sha256, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    hasher.update(len.to_be_bytes());
    hasher.update(&bytes[..bytes.len().min(len as usize)]);
}

/// Optional length-prefixed field: presence tag, then (if present) the field.
/// The tag keeps `Some("")` distinct from `None`.
fn opt_lp(hasher: &mut Sha256, bytes: Option<&[u8]>) {
    match bytes {
        Some(b) => {
            hasher.update([1u8]);
            lp(hasher, b);
        }
        None => hasher.update([0u8]),
    }
}

/// SHA-256 over the record's identity, content and chain link.
///
/// Field order is fixed. Changing the order, the set, or any `as_str()` inside
/// it invalidates every stored chain.
#[must_use]
pub fn compute_hash(p: &Preimage<'_>) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    // Identity leads: a row cannot be replayed into another agent's chain.
    lp(&mut h, p.agent_id.as_bytes());
    h.update(p.seq.to_be_bytes());
    h.update(p.at_ms.to_be_bytes());
    lp(&mut h, p.action.as_str().as_bytes());
    lp(&mut h, p.outcome.as_str().as_bytes());
    lp(&mut h, p.target.as_bytes());
    opt_lp(&mut h, p.args_fp.map(str::as_bytes));
    lp(&mut h, p.detail.as_bytes());
    lp(&mut h, p.signer_fp.as_bytes());
    match p.prev_hash {
        Some(prev) => h.update(prev),
        None => h.update(GENESIS_HASH),
    }
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample<'a>() -> Preimage<'a> {
        Preimage {
            agent_id: "main",
            seq: 1,
            at_ms: 1_700_000_000_123,
            action: LedgerAction::ToolCall,
            outcome: LedgerOutcome::Ok,
            target: "file_ops",
            args_fp: Some("abc123"),
            detail: "file_ops: operation=delete",
            signer_fp: "0011223344556677",
            prev_hash: None,
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(compute_hash(&sample()), compute_hash(&sample()));
    }

    #[test]
    fn agent_id_is_part_of_identity() {
        // The whole point: the same logical record in two agents' chains
        // hashes differently, so a row cannot be transplanted.
        let a = compute_hash(&sample());
        let mut other = sample();
        other.agent_id = "trader";
        assert_ne!(a, compute_hash(&other));
    }

    #[test]
    fn sensitive_to_every_field() {
        let base = compute_hash(&sample());

        let mut p = sample();
        p.seq = 2;
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        p.at_ms += 1;
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        p.action = LedgerAction::ToolDenied;
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        p.outcome = LedgerOutcome::Denied;
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        p.target = "bash";
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        p.args_fp = Some("def456");
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        p.detail = "something else";
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        p.signer_fp = "ffffffffffffffff";
        assert_ne!(base, compute_hash(&p));

        let mut p = sample();
        let prev = [0xABu8; 32];
        p.prev_hash = Some(&prev);
        assert_ne!(base, compute_hash(&p));
    }

    #[test]
    fn presence_tag_distinguishes_none_from_empty() {
        let mut none = sample();
        none.args_fp = None;
        let mut empty = sample();
        empty.args_fp = Some("");
        assert_ne!(compute_hash(&none), compute_hash(&empty));
    }

    #[test]
    fn length_prefixes_make_adjacent_fields_unambiguous() {
        // The reference implementation's framing defect, pinned here so a
        // future "simplification" that drops the length prefixes fails loudly:
        // moving a character across a field boundary must change the digest.
        let mut a = sample();
        a.target = "ab";
        a.detail = "c";
        let mut b = sample();
        b.target = "a";
        b.detail = "bc";
        assert_ne!(compute_hash(&a), compute_hash(&b));
    }

    #[test]
    fn genesis_is_not_the_same_as_a_zero_prev_hash_row() {
        // A first entry (prev NULL) and an entry chained to an all-zero hash
        // are indistinguishable BY CONSTRUCTION — the sentinel is the zero
        // block. This test documents that equivalence rather than asserting a
        // difference that does not exist: the genesis position is instead
        // enforced by `verify`, which requires seq 1 to have prev_hash NULL.
        let mut chained = sample();
        chained.prev_hash = Some(&GENESIS_HASH);
        assert_eq!(compute_hash(&sample()), compute_hash(&chained));
    }
}
