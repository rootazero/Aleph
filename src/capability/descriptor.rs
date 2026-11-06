//! Capability descriptor — the unified, kind-agnostic capability contract.
//!
//! [`CapabilityDescriptor`] is the shape every backend exposes through the
//! [`crate::capability::facade::Zahir`] facade. The descriptor keeps the kind
//! axis closed while the Tool and special-capability handler backends provide
//! executable adapters. Other kinds remain descriptive until their owning
//! backend is mounted.
//!
//! [`Service`] is a pure relation edge (`requires` / `provides` / `conflicts`):
//! it carries no data and is never invocable.

pub use aleph_slot_api::capability::{CapabilityId, Service, ServiceRelation};

use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};

/// The capability kinds named by the contract.
///
/// Tool, Ambient, Terminal, and UI-backed capabilities have executable
/// capability-layer adapters. The remaining kinds stay descriptive until their
/// owning backend is mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityKind {
    Tool,
    Ambient,
    Terminal,
    Ui,
    Skill,
    Agent,
    Task,
    Resource,
    EventSource,
    Subscription,
    Plugin,
    Hook,
}

impl CapabilityKind {
    /// Compatibility spelling for callers that use the acronym from the wire contract.
    pub const UI: Self = Self::Ui;
}

/// A behavior axis orthogonal to [`CapabilityKind`].
///
/// A kind is the primary classification of a capability; facets describe the
/// externally composable behavior that consumers may discover, invoke,
/// subscribe to, or manage. Keeping the axes separate lets one provider expose
/// both an event source and callable operations without pretending that the
/// whole provider is a `Tool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityFacet {
    Callable,
    EventSource,
    Subscription,
    ProviderLifecycle,
    ExternalEffect,
}

/// A compact, copyable set of behavior facets carried by a descriptor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CapabilityFacets(u8);

impl CapabilityFacets {
    const CALLABLE: u8 = 1 << 0;
    const EVENT_SOURCE: u8 = 1 << 1;
    const SUBSCRIPTION: u8 = 1 << 2;
    const PROVIDER_LIFECYCLE: u8 = 1 << 3;
    const EXTERNAL_EFFECT: u8 = 1 << 4;

    #[must_use]
    pub const fn new() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn with(self, facet: CapabilityFacet) -> Self {
        Self(self.0 | Self::bit(facet))
    }

    #[must_use]
    pub const fn contains(self, facet: CapabilityFacet) -> bool {
        self.0 & Self::bit(facet) != 0
    }

    #[must_use]
    const fn bit(facet: CapabilityFacet) -> u8 {
        match facet {
            CapabilityFacet::Callable => Self::CALLABLE,
            CapabilityFacet::EventSource => Self::EVENT_SOURCE,
            CapabilityFacet::Subscription => Self::SUBSCRIPTION,
            CapabilityFacet::ProviderLifecycle => Self::PROVIDER_LIFECYCLE,
            CapabilityFacet::ExternalEffect => Self::EXTERNAL_EFFECT,
        }
    }
}

/// Monotonic capability revision, assigned by the owning registry on every
/// successful mutation. Mirrors `ToolHandlerRegistry`'s own revision counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityRevision(pub u64);

/// Versioned schema identity: the schema version plus a SHA-256 fingerprint
/// of the schema's canonical form.
///
/// The fingerprint is computed by `capability::backend::to_descriptor` over
/// the canonicalized input schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchemaRef {
    pub version: u32,
    pub fingerprint: [u8; 32],
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
    /// Orthogonal behavior facets. This is descriptive metadata only; provider,
    /// owner, and descriptor authorities remain in their existing sources.
    pub facets: CapabilityFacets,
}

impl CapabilityDescriptor {
    #[must_use]
    pub const fn supports(&self, facet: CapabilityFacet) -> bool {
        self.facets.contains(facet)
    }
}
