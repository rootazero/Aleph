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
use crate::capability::ownership::OwnerGeneration;

/// A scope selecting which capabilities a facade operation targets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Restrict to one capability kind; `None` = all kinds.
    pub kind: Option<CapabilityKind>,
    /// Restrict to a namespace prefix; `None` = all namespaces.
    pub namespace: Option<String>,
}

/// A reference by which a single capability is resolved into a lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub id: CapabilityId,
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityChangeStream {
    pub committed_cursor: Cursor,
    pub changes: Vec<CapabilityChange>,
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
/// [`Zahir::resolve`] returns a [`BackendLease`], never a live handler: the
/// lease names its owner generation so a consumer can fail closed the moment
/// that generation advances. [`Zahir::subscribe`] returns a
/// [`CapabilityChangeStream`] whose cursor resync semantics are fixed in
/// Task 4.
pub trait Zahir {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot;
    fn resolve(&self, reference: Reference) -> BackendLease;
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream;
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection;
}
