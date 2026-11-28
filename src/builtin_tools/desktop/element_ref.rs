//! Session-bound AX tokens and native mutation admission.
//!
//! Each `(session, pid)` lane holds its most recent observation. A published
//! token names one of that observation's driver-owned handles. Admission and
//! publication share a mutex, so an action cannot be queued using a token
//! already superseded by a new observation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, Instant};

use aleph_protocol::desktop_bridge::methods::ax::{
    AxReferenceError, NativeAxReference, AX_REFERENCE_TTL_SECS, MAX_AX_REFERENCE_SNAPSHOTS,
};

use crate::sync_primitives::Mutex;

use super::types::DesktopOutput;

pub(super) fn token_error_output(error: &TokenError) -> DesktopOutput {
    reference_error_output(error.code(), error.message())
}

pub(super) fn driver_error_output(error: aleph_desktop::DesktopError) -> DesktopOutput {
    match error {
        aleph_desktop::DesktopError::AxReference { code, message } => {
            reference_error_output(code.code(), message)
        }
        other => DesktopOutput::failed(other.to_string()),
    }
}

pub(super) fn reference_error_output(code: &str, message: impl Into<String>) -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: Some(serde_json::json!({ "code": code })),
        message: Some(super::recovery::with_hint(message.into())),
    }
}

const MAX_LANES: usize = MAX_AX_REFERENCE_SNAPSHOTS;

/// Platform handle and safe target metadata from one published observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementRecord {
    pub pid: i32,
    pub window_id: Option<u64>,
    pub reference: NativeAxReference,
}

/// One issued snapshot: the id tokens name, and the elements they index into.
struct Snapshot {
    id: u32,
    issued: Instant,
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
    pending: HashSet<i32>,
    observations: Vec<Weak<SnapshotRead>>,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| {
    Mutex::new(Store {
        next_id: 1,
        lanes: HashMap::new(),
        lru: VecDeque::new(),
        pending: HashSet::new(),
        observations: Vec::new(),
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
    IndexOutOfRange {
        index: usize,
        count: usize,
    },
    /// The token was combined with explicit targeting arguments
    /// (`pid`/`role`/`element_title`/`x`/`y`): ambiguous intent, refused.
    ConflictingTarget,
    ActionPending,
}

impl TokenError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidFormat | Self::IndexOutOfRange { .. } => "invalid_reference",
            Self::Stale => AxReferenceError::StaleReference.code(),
            Self::ConflictingTarget => "conflicting_reference_target",
            Self::ActionPending => AxReferenceError::ActionPending.code(),
        }
    }

    /// The model-facing message. Staleness guidance is part of the contract:
    /// "stale" must read as "re-snapshot and retry", never as "click failed".
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::InvalidFormat => "element token has invalid format — use a `token` value \
                 exactly as returned by ax_snapshot (shaped like `s00000001:3`)"
                .to_string(),
            Self::Stale => "element token is stale. Take a fresh desktop_ax_snapshot and \
                 use one of the returned tokens"
                .to_string(),
            Self::IndexOutOfRange { index, count } => format!(
                "element token index {index} is out of range — its snapshot had {count} \
                 elements. Take a fresh ax_snapshot and use an index it actually returned"
            ),
            Self::ConflictingTarget => "An element token carries its own app, window and element. \
                 Pass it without other targeting fields or coordinates."
                .to_string(),
            Self::ActionPending => "A native AX action is still pending for this target, or \
                 overlapped this observation. Take a fresh desktop_ax_snapshot once it settles"
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
#[cfg(test)]
pub fn issue(session: &str, pid: i32, elements: Vec<ElementRecord>) -> u32 {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    issue_locked(&mut store, session, pid, elements)
}

fn issue_locked(store: &mut Store, session: &str, pid: i32, elements: Vec<ElementRecord>) -> u32 {
    let id = store.next_id;
    // Wrapping matters only after 4 billion snapshots; skip 0 so it never
    // becomes a valid id.
    store.next_id = store.next_id.wrapping_add(1).max(1);

    let key = (session.to_string(), pid);
    // Same-scope replacement *is* the invalidation: the lane holds exactly one
    // live snapshot, so every token from a previous observation of this app is
    // stale from here on.
    store.lanes.insert(
        key.clone(),
        Snapshot {
            id,
            issued: Instant::now(),
            elements,
        },
    );
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
    let store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    resolve_locked(&store, session, token).cloned()
}

fn resolve_locked<'a>(
    store: &'a Store,
    session: &str,
    token: &str,
) -> Result<&'a ElementRecord, TokenError> {
    let (id, index) = parse(token)?;
    let snapshot = store
        .lanes
        .iter()
        .find(|((lane_session, _), snap)| lane_session == session && snap.id == id)
        .map(|(_, snap)| snap)
        .filter(|snap| snap.issued.elapsed() < Duration::from_secs(AX_REFERENCE_TTL_SECS))
        .ok_or(TokenError::Stale)?;
    snapshot
        .elements
        .get(index)
        .ok_or(TokenError::IndexOutOfRange {
            index,
            count: snapshot.elements.len(),
        })
}

/// Observe without holding the store lock during provider I/O. An unresolved
/// window owner overlaps every PID until the driver proves the target scope.
pub(super) struct SnapshotRead {
    pid: Option<i32>,
    overlapped: AtomicBool,
}

impl SnapshotRead {
    pub(super) fn begin(pid: Option<i32>) -> Result<Arc<Self>, TokenError> {
        let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
        if pid.map_or(!store.pending.is_empty(), |pid| {
            store.pending.contains(&pid)
        }) {
            return Err(TokenError::ActionPending);
        }
        let read = Arc::new(Self {
            pid,
            overlapped: AtomicBool::new(false),
        });
        store.observations.retain(|read| read.strong_count() > 0);
        store.observations.push(Arc::downgrade(&read));
        Ok(read)
    }

    pub(super) fn publish(
        &self,
        session: &str,
        pid: i32,
        records: Vec<ElementRecord>,
    ) -> Result<u32, TokenError> {
        let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
        if self.overlapped.load(Ordering::Relaxed) || store.pending.contains(&pid) {
            return Err(TokenError::ActionPending);
        }
        Ok(issue_locked(&mut store, session, pid, records))
    }
}

/// Owned by the dispatched task, so dropping its caller cannot reopen the
/// observation boundary while driver I/O may still affect the app.
pub(super) struct PendingAction(i32);

impl PendingAction {
    pub(super) fn admit(
        session: &str,
        token: &str,
        expected: &ElementRecord,
    ) -> Result<Self, TokenError> {
        let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
        if resolve_locked(&store, session, token)? != expected {
            return Err(TokenError::Stale);
        }
        let pid = expected.pid;
        if !store.pending.insert(pid) {
            return Err(TokenError::ActionPending);
        }
        store.observations.retain(|read| {
            let Some(read) = read.upgrade() else {
                return false;
            };
            if read.pid.is_none_or(|observed| observed == pid) {
                read.overlapped.store(true, Ordering::Relaxed);
            }
            true
        });
        Ok(Self(pid))
    }
}

impl Drop for PendingAction {
    fn drop(&mut self) {
        STORE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .remove(&self.0);
    }
}

/// Drop every lane — daemon shutdown and tests. Idempotent.
#[cfg(test)]
pub fn clear_all() {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.lanes.clear();
    store.lru.clear();
}

#[cfg(test)]
pub(super) static TEST_STORE_GUARD: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_STORE_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn records(n: usize) -> Vec<ElementRecord> {
        (0..n)
            .map(|i| ElementRecord {
                pid: 42,
                window_id: None,
                reference: NativeAxReference {
                    lease: "driver-lease".into(),
                    index: i as u32,
                },
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
        assert_eq!(rec.reference.index, 1);
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
    fn action_reservation_blocks_overlapping_reads_until_settled() {
        let _g = guard();
        clear_all();
        let reference = records(1).pop().unwrap();
        let token = render(issue("s1", 42, vec![reference.clone()]), 0);
        let overlapping = SnapshotRead::begin(Some(42)).unwrap();
        let unscoped = SnapshotRead::begin(None).unwrap();
        let unrelated = SnapshotRead::begin(Some(99)).unwrap();
        let pending = PendingAction::admit("s1", &token, &reference).unwrap();
        assert!(matches!(
            SnapshotRead::begin(Some(42)),
            Err(TokenError::ActionPending)
        ));
        assert!(matches!(
            PendingAction::admit("s1", &token, &reference),
            Err(TokenError::ActionPending)
        ));
        assert!(unrelated.publish("s1", 99, vec![]).is_ok());
        drop(pending);
        assert_eq!(
            overlapping.publish("s1", 42, vec![]),
            Err(TokenError::ActionPending)
        );
        assert_eq!(
            unscoped.publish("s1", 42, vec![]),
            Err(TokenError::ActionPending)
        );
        assert!(SnapshotRead::begin(Some(42))
            .unwrap()
            .publish("s1", 42, vec![])
            .is_ok());
        assert_eq!(resolve("s1", &token), Err(TokenError::Stale));
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
