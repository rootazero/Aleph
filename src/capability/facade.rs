//! The Zahir facade — the single, kind-agnostic capability surface.
//!
//! [`Zahir`] is the read/query face every consumer (UI, MCP projection, ACP
//! JSON projection) goes through. It never invokes a capability directly; it
//! describes, resolves (into a lease), subscribes to change, and projects.
//!
//! Task 1 establishes the trait and the pure transport types. The immutable
//! snapshot / lease-invalidation semantics are documented here and implemented
//! in Task 2 (lease staleness) and Task 4 (subscription state machine).

use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityId, CapabilityKind, CapabilityRevision, ProjectionMetadata,
};
use crate::capability::ownership::{OwnerGeneration, VisibilityScope};

/// A scope selecting which capabilities a facade operation targets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Restrict to one capability kind; `None` = all kinds.
    pub kind: Option<CapabilityKind>,
    /// Restrict to a namespace prefix; `None` = all namespaces.
    pub namespace: Option<String>,
    /// The visibility envelope to describe/project/subscribe under.
    /// `VisibilityScope::default()` = unrestricted.
    pub visibility: VisibilityScope,
}

/// A reference by which a single capability is resolved into a lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub id: CapabilityId,
    /// The visibility envelope to resolve the id under.
    pub visibility: VisibilityScope,
}

/// A monotonic subscription position.
///
/// `0` = "from the beginning"; a cursor records the subscriber's last
/// committed progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Cursor(pub u64);

/// An immutable view of a scope's capabilities at one moment.
///
/// Contents are frozen for the snapshot's lifetime: a later mutation or owner
/// bump does not rewrite a held snapshot; it produces a new snapshot or an
/// `Invalidated` change event instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySnapshot {
    pub capabilities: Vec<CapabilityDescriptor>,
    pub committed_cursor: Cursor,
}

/// A resolved, possibly-stale lease.
///
/// The lease carries the owner generation captured at resolve time. A consumer
/// MUST treat the lease as invalid the moment the current owner generation
/// differs (owner `bump` / `revoke`) — fail-closed, never a stale handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendLease {
    pub descriptor: CapabilityDescriptor,
    pub owner_generation: OwnerGeneration,
}

/// Closed error set for `Zahir::resolve` / `Zahir::validate_lease`.
///
/// `StaleOwner` is NOT a resolve-time error: a freshly minted lease carries the
/// generation read at resolve time, so it can never be stale at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// Lookup miss, binding missing (and not revoked), descriptor freshness
    /// mismatch, or any unclassifiable failure — fail-closed.
    Unknown,
    /// The caller's `Reference.visibility` has no binding for the id (resolve-only).
    NotVisible,
    /// The binding has been revoked.
    Revoked,
    /// The lease's `owner_generation` lags the current per-binding generation
    /// (validate-only).
    StaleOwner,
}

/// A single capability change delivered to a subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityChange {
    Registered {
        id: CapabilityId,
        revision: CapabilityRevision,
    },
    Replaced {
        id: CapabilityId,
        revision: CapabilityRevision,
    },
    Unregistered {
        id: CapabilityId,
        revision: CapabilityRevision,
    },
    /// The owner generation advanced; a held cursor stays valid but the
    /// subscriber must resync (snapshot + delta) — never restart from zero.
    Invalidated,
}

/// A subscription's change stream.
///
/// Task 1 establishes the type only. Task 4 attaches the snapshot + delta
/// resync state machine: first subscribe returns an atomic snapshot + committed
/// cursor; a generation bump emits [`CapabilityChange::Invalidated`] while
/// preserving the last committed cursor, then re-syncs with dedup by
/// `OwnerGeneration` / `CapabilityRevision`.
///
/// **Pure value semantics.** This struct holds no store, registry, or
/// subscription wiring. It is a state machine over `(cursor, changes)` and
/// nothing else — owner-generation verification lives in the future concrete
/// facade (`lease` / `future` impls). Dedup uses `(id, revision)` only; the
/// owner-generation check belongs at the backend boundary, not here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityChangeStream {
    pub committed_cursor: Cursor,
    pub changes: Vec<CapabilityChange>,
}

impl CapabilityChangeStream {
    /// Begin a stream from an initial atomic snapshot.
    ///
    /// The snapshot already represents every capability visible at
    /// `snapshot.committed_cursor`, so the new stream's `changes` list starts
    /// empty. Subsequent events arrive one at a time via [`Self::append`].
    pub fn from_snapshot(snapshot: CapabilitySnapshot) -> Self {
        Self {
            committed_cursor: snapshot.committed_cursor,
            changes: Vec::new(),
        }
    }

    /// Append a single change at the given cursor.
    ///
    /// Returns `true` and advances `committed_cursor` iff `cursor` is strictly
    /// greater than the current cursor. Stale or equal cursors are rejected
    /// without mutating the stream — monotonicity is the only contract here.
    pub fn append(&mut self, cursor: Cursor, change: CapabilityChange) -> bool {
        if cursor <= self.committed_cursor {
            return false;
        }
        self.committed_cursor = cursor;
        self.changes.push(change);
        true
    }

    /// Resync after an [`Invalidated`] event using a fresh snapshot + delta.
    ///
    /// This is a pure value state machine; it never reads or writes a store.
    /// Steps:
    ///
    /// 1. Snapshot `committed_cursor` as `old`.
    /// 2. Drop delta entries with `cursor <= old` (stale tail).
    /// 3. Drop surviving entries whose `(id, revision)` already appears in
    ///    `self.changes` — the subscriber has already seen them. Dedup also
    ///    collapses duplicates within the accepted tail.
    /// 4. Push one [`Invalidated`] followed by the surviving delta in input
    ///    order.
    /// 5. Advance `committed_cursor` to
    ///    `max(old, snapshot.committed_cursor, accepted cursors)`. The cursor
    ///    never regresses; the stream's history is preserved, never zeroed.
    ///
    /// Returns `false` **only** when `snapshot.committed_cursor < old` — a
    /// resync source that lags behind the subscriber. On failure the stream is
    /// not mutated (fail-closed).
    pub fn invalidate_and_resync(
        &mut self,
        snapshot: CapabilitySnapshot,
        delta: Vec<(Cursor, CapabilityChange)>,
    ) -> bool {
        let old = self.committed_cursor;
        if snapshot.committed_cursor < old {
            return false;
        }

        let mut new_cursor = old.max(snapshot.committed_cursor);
        let mut accepted = Vec::with_capacity(delta.len());
        for (cursor, change) in delta {
            if cursor <= old {
                continue;
            }
            if let Some((id, rev)) = id_rev_of(&change) {
                let already = self
                    .changes
                    .iter()
                    .chain(accepted.iter())
                    .any(|existing| match id_rev_of(existing) {
                        Some((eid, erev)) => eid == id && erev == rev,
                        None => false,
                    });
                if already {
                    continue;
                }
            }
            new_cursor = new_cursor.max(cursor);
            accepted.push(change);
        }

        self.changes.push(CapabilityChange::Invalidated);
        self.changes.extend(accepted);
        self.committed_cursor = new_cursor;
        true
    }
}

/// Extract the `(id, revision)` payload of a change, if it has one.
///
/// [`CapabilityChange::Invalidated`] carries no per-capability identity and
/// returns `None` so dedup can skip it; multiple `Invalidated` entries (one
/// per resync call) are therefore preserved verbatim.
fn id_rev_of(change: &CapabilityChange) -> Option<(&CapabilityId, CapabilityRevision)> {
    match change {
        CapabilityChange::Registered { id, revision }
        | CapabilityChange::Replaced { id, revision }
        | CapabilityChange::Unregistered { id, revision } => Some((id, *revision)),
        CapabilityChange::Invalidated => None,
    }
}

/// A transport a capability may be projected onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportTarget {
    Mcp,
    AcpJson,
    StateDatabase,
}

/// The result of projecting a scope onto a transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    pub target: TransportTarget,
    pub metadata: ProjectionMetadata,
    pub capabilities: Vec<CapabilityId>,
}

/// The read/query face of the capability layer.
///
/// [`Zahir::resolve`] returns `Result<`[`BackendLease`]`, `[`ResolveError`]`>`,
/// never a live handler: the lease names its owner generation so a consumer can
/// fail closed the moment that generation advances, re-checked at use time via
/// [`Zahir::validate_lease`]. [`Zahir::subscribe`] returns a
/// [`CapabilityChangeStream`] whose cursor resync semantics are fixed in the
/// subscription state machine.
pub trait Zahir {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot;
    fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError>;
    fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError>;
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream;
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_snapshot(cursor: u64) -> CapabilitySnapshot {
        CapabilitySnapshot {
            capabilities: Vec::new(),
            committed_cursor: Cursor(cursor),
        }
    }

    fn cap_id(namespace: &str, name: &str) -> CapabilityId {
        CapabilityId {
            namespace: namespace.to_string(),
            name: name.to_string(),
        }
    }

    fn registered(namespace: &str, name: &str, rev: u64) -> CapabilityChange {
        CapabilityChange::Registered {
            id: cap_id(namespace, name),
            revision: CapabilityRevision(rev),
        }
    }

    fn replaced(namespace: &str, name: &str, rev: u64) -> CapabilityChange {
        CapabilityChange::Replaced {
            id: cap_id(namespace, name),
            revision: CapabilityRevision(rev),
        }
    }

    fn unregistered(namespace: &str, name: &str, rev: u64) -> CapabilityChange {
        CapabilityChange::Unregistered {
            id: cap_id(namespace, name),
            revision: CapabilityRevision(rev),
        }
    }

    #[test]
    fn from_snapshot_captures_cursor_and_empties_changes() {
        let stream = CapabilityChangeStream::from_snapshot(empty_snapshot(7));
        assert_eq!(stream.committed_cursor, Cursor(7));
        assert!(stream.changes.is_empty());
    }

    #[test]
    fn append_only_accepts_strictly_monotonic_cursors() {
        let mut stream = CapabilityChangeStream::from_snapshot(empty_snapshot(5));

        // Strictly greater — accepted.
        assert!(stream.append(Cursor(6), registered("ns", "a", 1)));
        assert_eq!(stream.committed_cursor, Cursor(6));
        assert_eq!(stream.changes.len(), 1);

        // Equal cursor — rejected, stream unchanged.
        assert!(!stream.append(Cursor(6), registered("ns", "a", 2)));
        assert_eq!(stream.committed_cursor, Cursor(6));
        assert_eq!(stream.changes.len(), 1);

        // Stale cursor — rejected, stream unchanged.
        assert!(!stream.append(Cursor(3), registered("ns", "a", 2)));
        assert_eq!(stream.committed_cursor, Cursor(6));
        assert_eq!(stream.changes.len(), 1);

        // Next strictly greater — accepted.
        assert!(stream.append(Cursor(9), replaced("ns", "a", 2)));
        assert_eq!(stream.committed_cursor, Cursor(9));
        assert_eq!(stream.changes.len(), 2);
    }

    #[test]
    fn invalidate_and_resync_advances_cursor_without_zeroing() {
        let mut stream = CapabilityChangeStream::from_snapshot(empty_snapshot(5));
        assert!(stream.append(Cursor(6), registered("ns", "a", 1)));
        assert_eq!(stream.committed_cursor, Cursor(6));

        let delta = vec![
            (Cursor(7), replaced("ns", "a", 2)),
            (Cursor(8), unregistered("ns", "b", 3)),
        ];
        // snapshot.committed_cursor (10) wins over old (6) and both delta
        // cursors (7, 8); the resync never resets the cursor to zero.
        assert!(stream.invalidate_and_resync(empty_snapshot(10), delta));
        assert_eq!(stream.committed_cursor, Cursor(10));

        // Pre-existing history is preserved; Invalidated is appended once;
        // both delta entries follow in input order.
        assert_eq!(stream.changes.len(), 4);
        assert!(matches!(stream.changes[0], CapabilityChange::Registered { .. }));
        assert!(matches!(stream.changes[1], CapabilityChange::Invalidated));
        assert!(matches!(stream.changes[2], CapabilityChange::Replaced { .. }));
        assert!(matches!(stream.changes[3], CapabilityChange::Unregistered { .. }));
    }

    #[test]
    fn invalidate_and_resync_dedups_by_id_and_revision() {
        let mut stream = CapabilityChangeStream::from_snapshot(empty_snapshot(1));
        // Pre-existing change that the delta will echo.
        stream.changes.push(registered("ns", "x", 2));
        stream.committed_cursor = Cursor(1);

        let delta = vec![
            // Same (id, revision) as the existing change — must be dropped.
            (Cursor(2), registered("ns", "x", 2)),
            // Brand new (id, revision) — must be kept.
            (Cursor(3), registered("ns", "y", 1)),
        ];
        assert!(stream.invalidate_and_resync(empty_snapshot(4), delta));

        // 1 pre-existing + 1 Invalidated + 1 surviving delta entry.
        assert_eq!(stream.changes.len(), 3);
        assert_eq!(stream.committed_cursor, Cursor(4));
        assert!(matches!(stream.changes[0], CapabilityChange::Registered { .. }));
        assert!(matches!(stream.changes[1], CapabilityChange::Invalidated));
        assert!(matches!(
            stream.changes[2],
            CapabilityChange::Registered { ref id, revision: CapabilityRevision(1) }
                if id == &cap_id("ns", "y")
        ));
    }

    #[test]
    fn invalidate_and_resync_rejects_regressing_snapshot() {
        let mut stream = CapabilityChangeStream::from_snapshot(empty_snapshot(10));
        stream.changes.push(CapabilityChange::Invalidated);
        stream.committed_cursor = Cursor(10);

        // snapshot.committed_cursor (5) lags the stream's cursor (10): fail
        // closed, no mutation.
        assert!(!stream.invalidate_and_resync(empty_snapshot(5), Vec::new()));
        assert_eq!(stream.committed_cursor, Cursor(10));
        assert_eq!(stream.changes.len(), 1);
        assert!(matches!(stream.changes[0], CapabilityChange::Invalidated));
    }

    #[test]
    fn scope_defaults_to_unrestricted_visibility() {
        let scope = Scope::default();
        assert_eq!(scope.kind, None);
        assert_eq!(scope.namespace, None);
        assert_eq!(scope.visibility, VisibilityScope::default());
    }

    #[test]
    fn reference_carries_its_visibility() {
        let id = cap_id("ns", "name");
        let visibility = VisibilityScope {
            workspace: Some("w".to_string()),
            ..VisibilityScope::default()
        };
        let reference = Reference {
            id: id.clone(),
            visibility: visibility.clone(),
        };
        assert_eq!(reference.id, id);
        assert_eq!(reference.visibility, visibility);
    }

    #[test]
    fn resolve_error_is_a_closed_set_of_distinct_variants() {
        let variants = [
            ResolveError::Unknown,
            ResolveError::NotVisible,
            ResolveError::Revoked,
            ResolveError::StaleOwner,
        ];
        // Closed set: every variant compares equal only to itself, and cloning
        // preserves identity.
        for (i, a) in variants.iter().enumerate() {
            assert_eq!(a.clone(), *a);
            for (j, b) in variants.iter().enumerate() {
                assert_eq!(a == b, i == j, "variants {i} and {j} must differ");
            }
        }
    }
}
