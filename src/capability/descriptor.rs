//! Capability descriptor — the unified, kind-agnostic capability contract.
//!
//! [`CapabilityDescriptor`] is the shape every backend exposes through the
//! [`crate::capability::facade::Zahir`] facade. Task 1 establishes the pure
//! types only: [`CapabilityKind`] names all nine capability kinds, but only
//! `Tool` has a live backend today
//! ([`crate::capability::backend::ToolBackendAdapter`]). The other eight kinds
//! are deferred — they are *named* here (so the contract is closed) but not
//! *executable*: no backend implements them, none is mounted, and none can be
//! invoked this phase.
//!
//! [`Service`] is a pure relation edge (`requires` / `provides` / `conflicts`):
//! it carries no data and is never invocable.

use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};

/// The nine capability kinds this contract names.
///
/// Only `Tool` is executable this phase. The other eight exist so the type
/// system can *say* "this is a Skill" without a backend that can run one — a
/// deferred kind is non-invocable by construction, because no adapter
/// implements [`crate::capability::backend::CapabilityBackend`] for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityKind {
    Tool,
    Skill,
    Agent,
    Task,
    Resource,
    EventSource,
    Subscription,
    Plugin,
    Hook,
}

/// A namespaced capability identifier, e.g. `aleph/tools/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapabilityId {
    pub namespace: String,
    pub name: String,
}

/// Monotonic capability revision, assigned by the owning registry on every
/// successful mutation. Mirrors `ToolHandlerRegistry`'s own revision counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityRevision(pub u64);

/// Versioned schema identity: the schema version plus a SHA-256 fingerprint
/// of the schema's canonical form.
///
/// The fingerprint is computed by the describe/projection path, not here —
/// Task 1 only names the field; the digest is left uncomputed until a later
/// task owns canonical schema serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchemaRef {
    pub version: u32,
    pub fingerprint: [u8; 32],
}

/// How one capability relates to another in the service graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRelation {
    Requires,
    Provides,
    Conflicts,
}

/// A service-graph edge: the capability this descriptor belongs to stands in
/// `relation` to `target`.
///
/// Pure relation data — no payload, never invocable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub relation: ServiceRelation,
    pub target: CapabilityId,
}

/// How a capability is surfaced to transports.
///
/// Pure data. The `projection_only` / `notification_only` markers are enforced
/// at the facade layer (Task 4), not encoded here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionMetadata {
    /// Projection surface name (e.g. `"mcp"`, `"acp-json"`,
    /// `"state-database"`). `None` = not projected anywhere yet.
    pub surface: Option<String>,
}

/// The unified, kind-agnostic capability descriptor.
///
/// Every backend produces this shape; the existing
/// `ToolCapabilityDescriptor` is one source that folds into it (see
/// [`crate::capability::backend::ToolBackendAdapter`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub id: CapabilityId,
    pub kind: CapabilityKind,
    pub schema: SchemaRef,
    pub revision: CapabilityRevision,
    pub lifetime: LifetimeScope,
    pub visibility: VisibilityScope,
    pub owner: OwnerRef,
    pub services: Vec<Service>,
    pub metadata: ProjectionMetadata,
}
