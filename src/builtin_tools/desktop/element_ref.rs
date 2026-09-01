//! Element tokens — snapshot-scoped, staleness-checked element references.
//!
//! Ported from cua-driver's `element_token` (`docs/reference/FEATURE_LOCATOR.md`
//! §7.3 keeps the porting notes). The problem they solve: `ax_snapshot` hands
//! the model an indexed element list, but the index is only meaningful against
//! the snapshot that produced it. Without a token the model re-typed `center`
//! coordinates by hand — and a window that moved between snapshot and click
//! turned those coordinates into a click on whatever happens to be there now.
//! A token binds the reference to the snapshot that issued it, so acting on a
//! superseded observation is a *named, closed-set error* instead of a silent
//! mis-click.
//!
//! # Contract (mirrors cua-driver, adapted to Aleph's locator model)
//!
//! * Format `s{snapshot_id:08x}:{element_index}` — short, grep-able in logs,
//!   the index legible inline. `snapshot_id` 0 never issues.
//! * One live snapshot per `(session, pid)` lane: `ax_snapshot` is a
//!   whole-app observation, so re-snapshotting the same app **supersedes the
//!   previous snapshot immediately** — every token minted from it goes stale.
//!   A global LRU cap bounds total lanes across sessions and pids.
//! * Resolution is fail-closed: a stale token is never silently reinterpreted
//!   as a bare index, and a token combined with explicit targeting arguments
//!   (`pid` / `role` / `element_title` / `x` / `y`) is a conflict error, not a
//!   precedence puzzle.
//! * A resolved token is an *identity* (`pid`, `role`, `title`, last-seen
//!   center), not a handle: every consumer re-resolves it against the live AX
//!   tree at action time (the limbs re-walk per call already — see
//!   `aleph_desktop::ax_rank`), so a moved window is found at its new position
//!   and a vanished element is a named error, not a stale coordinate.
//!
//! The store is process-global state like the sibling ledgers
//! (`session_lock`, `held_inputs`): one `Mutex`, poison-safe, no `unwrap`.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;

use crate::sync_primitives::Mutex;

/// Cap on live snapshot lanes across all sessions and pids. Snapshots are
/// large only in the elements vector (bounded by the snapshot's own
/// `max_elements` ≤ 500), so 32 lanes is a few hundred KB worst case — and far
/// above any real interleaving of apps a single desktop operator drives.
const MAX_LANES: usize = 32;

/// The snapshot-scoped identity of one element, recorded at issue time.
///
/// `center` is the *last observed* click point in global pixels: a tiebreak
/// hint for re-resolution, never a coordinate to act on blindly.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementRecord {
    pub pid: i32,
    pub role: String,
    pub title: Option<String>,
    pub center: [f64; 2],
}

/// One issued snapshot: the id tokens name, and the elements they index into.
struct Snapshot {
    id: u32,
    elements: Vec<ElementRecord>,
}

/// Lane key: the session that took the snapshot and the app it observed.
type LaneKey = (String, i32);

struct Store {
    /// Next snapshot id. Starts at 1; 0 never issues, so a zeroed or
    /// default-constructed id can never resolve.
    next_id: u32,
    lanes: HashMap<LaneKey, Snapshot>,
    /// Lane keys in issue order (most recent last), for the global LRU cap.
    lru: VecDeque<LaneKey>,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| {
    Mutex::new(Store {
        next_id: 1,
        lanes: HashMap::new(),
        lru: VecDeque::new(),
    })
});

/// The closed set of ways a token can fail to resolve. Every variant maps to
/// exactly one model-facing message; a stale token must be *readable as an
/// instruction* ("take a fresh snapshot"), not a generic failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// Not shaped like `s{8 hex}:{index}`.
    InvalidFormat,
    /// Well-formed but names a snapshot that was superseded (same app
    /// re-snapshotted), evicted (LRU), or never issued by this session.
    Stale,
    /// The snapshot is live but the index is past its element count.
    IndexOutOfRange { index: usize, count: usize },
    /// The token was combined with explicit targeting arguments
    /// (`pid`/`role`/`element_title`/`x`/`y`): ambiguous intent, refused.
    ConflictingTarget,
    /// The token resolved, but a fresh look at the live AX tree found no
    /// element matching its identity — the UI changed since the snapshot.
    ElementGone,
}

impl TokenError {
    /// The model-facing message. Staleness guidance is part of the contract:
    /// "stale" must read as "re-snapshot and retry", never as "click failed".
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::InvalidFormat => "element token has invalid format — use a `token` value \
                 exactly as returned by ax_snapshot (shaped like `s00000001:3`)"
                .to_string(),
            Self::Stale => "element token is stale — the snapshot that issued it was \
                 superseded by a newer snapshot of the same app (or evicted). Take a fresh \
                 ax_snapshot and act on one of its tokens"
                .to_string(),
            Self::IndexOutOfRange { index, count } => format!(
                "element token index {index} is out of range — its snapshot had {count} \
                 elements. Take a fresh ax_snapshot and use an index it actually returned"
            ),
            Self::ConflictingTarget => "element token conflicts with explicit targeting \
                 (pid / role / element_title / x / y) — pass the token alone; it carries its \
                 own target"
                .to_string(),
            Self::ElementGone => "the element this token names no longer matches anything in \
                 the app's live accessibility tree — the UI changed since the snapshot. Take a \
                 fresh ax_snapshot"
                .to_string(),
        }
    }
}

/// Parse `s{snapshot_id:08x}:{element_index}`.
fn parse(token: &str) -> Result<(u32, usize), TokenError> {
    let body = token.strip_prefix('s').ok_or(TokenError::InvalidFormat)?;
    let (id_hex, index_dec) = body.split_once(':').ok_or(TokenError::InvalidFormat)?;
    if id_hex.len() != 8 || index_dec.is_empty() {
        return Err(TokenError::InvalidFormat);
    }
    let id = u32::from_str_radix(id_hex, 16).map_err(|_| TokenError::InvalidFormat)?;
    let index: usize = index_dec.parse().map_err(|_| TokenError::InvalidFormat)?;
    Ok((id, index))
}

/// Mint a new snapshot in `session`'s lane for `pid`, superseding whatever
/// snapshot that lane held, and return its id. Token strings are rendered by
/// the caller as [`render`] with this id and the element's list index.
pub fn issue(session: &str, pid: i32, elements: Vec<ElementRecord>) -> u32 {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let id = store.next_id;
    // Wrapping matters only after 4 billion snapshots; skip 0 so it never
    // becomes a valid id.
    store.next_id = store.next_id.wrapping_add(1).max(1);

    let key = (session.to_string(), pid);
    // Same-scope replacement *is* the invalidation: the lane holds exactly one
    // live snapshot, so every token from a previous observation of this app is
    // stale from here on.
    store.lanes.insert(key.clone(), Snapshot { id, elements });
    store.lru.retain(|k| k != &key);
    store.lru.push_back(key);
    while store.lru.len() > MAX_LANES {
        if let Some(evicted) = store.lru.pop_front() {
            store.lanes.remove(&evicted);
        }
    }
    id
}

/// Render the token string for element `index` of snapshot `id`.
#[must_use]
pub fn render(snapshot_id: u32, index: usize) -> String {
    format!("s{snapshot_id:08x}:{index}")
}

/// Resolve a token to its element record.
///
/// The session scopes the lookup: a token minted in one session does not
/// resolve in another, so two agents sharing one machine cannot act on each
/// other's observations by guessing.
pub fn resolve(session: &str, token: &str) -> Result<ElementRecord, TokenError> {
    let (id, index) = parse(token)?;
    let store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = store
        .lanes
        .iter()
        .find(|((lane_session, _), snap)| lane_session == session && snap.id == id)
        .map(|(_, snap)| snap)
        .ok_or(TokenError::Stale)?;
    snapshot
        .elements
        .get(index)
        .cloned()
        .ok_or(TokenError::IndexOutOfRange {
            index,
            count: snapshot.elements.len(),
        })
}

/// Drop every lane — daemon shutdown and tests. Idempotent.
#[cfg(test)]
pub fn clear_all() {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.lanes.clear();
    store.lru.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store is process-global; serialize the tests that mutate it.
    static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn records(n: usize) -> Vec<ElementRecord> {
        (0..n)
            .map(|i| ElementRecord {
                pid: 42,
                role: "AXButton".to_string(),
                title: Some(format!("b{i}")),
                center: [i as f64, 0.0],
            })
            .collect()
    }

    #[test]
    fn issued_token_round_trips() {
        let _g = guard();
        clear_all();
        let id = issue("s1", 42, records(3));
        let token = render(id, 1);
        assert_eq!(token, format!("s{id:08x}:1"));
        let rec = resolve("s1", &token).unwrap();
        assert_eq!(rec.title.as_deref(), Some("b1"));
        assert_eq!(rec.pid, 42);
    }

    #[test]
    fn re_snapshot_supersedes_the_previous_one() {
        let _g = guard();
        clear_all();
        let old = issue("s1", 42, records(2));
        let new = issue("s1", 42, records(2));
        assert_ne!(old, new);
        assert_eq!(
            resolve("s1", &render(old, 0)),
            Err(TokenError::Stale),
            "a token from the superseded snapshot must be stale, never reinterpreted"
        );
        assert!(resolve("s1", &render(new, 0)).is_ok());
    }

    #[test]
    fn a_snapshot_of_another_app_keeps_this_lane_alive() {
        let _g = guard();
        clear_all();
        let a = issue("s1", 42, records(2));
        let _b = issue("s1", 99, records(2));
        assert!(resolve("s1", &render(a, 0)).is_ok());
    }

    #[test]
    fn tokens_do_not_cross_sessions() {
        let _g = guard();
        clear_all();
        let id = issue("s1", 42, records(2));
        assert_eq!(resolve("s2", &render(id, 0)), Err(TokenError::Stale));
    }

    #[test]
    fn out_of_range_names_the_snapshot_size() {
        let _g = guard();
        clear_all();
        let id = issue("s1", 42, records(2));
        let err = resolve("s1", &render(id, 5)).unwrap_err();
        assert_eq!(err, TokenError::IndexOutOfRange { index: 5, count: 2 });
        let msg = err.message();
        assert!(msg.contains('5') && msg.contains('2'), "{msg}");
    }

    #[test]
    fn malformed_tokens_are_invalid_not_stale() {
        let _g = guard();
        for bad in [
            "",
            "x00000001:0",
            "s1:0",
            "s00000001",
            "s00000001:",
            "szzzzzzzz:0",
            "s00000001: x",
        ] {
            assert_eq!(
                resolve("s1", bad),
                Err(TokenError::InvalidFormat),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn lru_evicts_the_oldest_lane() {
        let _g = guard();
        clear_all();
        let first = issue("s1", 1, records(1));
        for pid in 2..=(MAX_LANES as i32 + 1) {
            issue("s1", pid, records(1));
        }
        assert_eq!(
            resolve("s1", &render(first, 0)),
            Err(TokenError::Stale),
            "the eldest lane past the cap is evicted"
        );
    }

    #[test]
    fn stale_message_reads_as_an_instruction() {
        let msg = TokenError::Stale.message();
        assert!(
            msg.contains("stale") && msg.contains("ax_snapshot"),
            "{msg}"
        );
    }
}
