//! Context capability fabric wire contract.
//!
//! This module is the **one** declaration of what a context capability looks
//! like on the wire. Registry entries, tool-catalog projections, MCP/ACP
//! projections and diagnostics all read these types; no transport layer keeps a
//! second copy of the shape (design §5).
//!
//! Two boundaries are deliberately separate:
//!
//! * **Native Rust execution** stays on [`crate::context::budget::preflight::PreflightStage`].
//!   Its `fn name(&self) -> &'static str` is a compile-time identity for
//!   built-in stages and must not be widened into a dynamic ABI: a plugin id
//!   arriving over the wire cannot become a `&'static str`.
//! * **External execution** (WASM, MCP, Node/HTTP, out-of-process) speaks the
//!   serde JSON contract below. Dynamic capability ids live in
//!   [`ContextCapabilityDescriptor::id`] and in registry keys. The external
//!   adapter bridges a dynamic wire call to a native stage; the trait never
//!   learns about plugin ids.
//!
//! ## What this module enforces, and what it does not
//!
//! Enforced here, fail-closed, by `validate` / `validate_against`: the
//! declaration itself, scope/permission coherence, per-call budgets, snapshot
//! presence for transforming operations, and fingerprint agreement between
//! request, snapshot and result. An unverifiable call is rejected, never
//! repaired.
//!
//! **Not** enforced here, and therefore a contract requirement on whoever
//! builds the call: [`ContextSnapshot::payload`] is `serde_json::Value`, so this
//! module cannot prove that hidden reasoning, credentials, unauthorised project
//! files or other sessions were filtered out before the payload was built. No
//! such filter exists yet (Core-side, a later task). A snapshot builder must
//! apply the scope filter itself; a consumer must not read receipt of a
//! snapshot as evidence that filtering happened.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The only wire version this module understands.
///
/// Every operation name is `"<api_version>/<operation>"` (see
/// [`ContextOperation::wire_name`]), so a peer that speaks a different version
/// fails at the name, not at a silently-drifting field set.
pub const CONTEXT_CAPABILITY_API_VERSION: &str = "context.fabric/v1";

/// Hard ceiling for a declared token budget, above every model window Aleph
/// targets. A descriptor above this is a budget bug, not a big context.
pub const CONTEXT_BUDGET_MAX_TOKENS: u64 = 4_000_000;

/// Hard ceiling for a declared byte budget (64 MiB).
pub const CONTEXT_BUDGET_MAX_BYTES: u64 = 64 * 1024 * 1024;

// ============================================================================
// Phase / mode / lifecycle vocabulary
// ============================================================================

/// The context phases a capability can own or observe (design §5).
///
/// `Lifecycle` is the phase that carries `before_*` / `after_*` subscriptions;
/// `Status` is read-only inspection and therefore carries no mutation
/// permission (see [`crate::extension::capability::context_capability_class`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextPhase {
    Admission,
    Preflight,
    Compaction,
    Recovery,
    Search,
    Status,
    Lifecycle,
}

impl ContextPhase {
    /// Every phase, in wire order.
    pub const ALL: [Self; 7] = [
        Self::Admission,
        Self::Preflight,
        Self::Compaction,
        Self::Recovery,
        Self::Search,
        Self::Status,
        Self::Lifecycle,
    ];

    /// The stable wire tag. Pinned against the serde representation by the
    /// `phase_tags_are_stable` test, so the two can never drift apart.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::Preflight => "preflight",
            Self::Compaction => "compaction",
            Self::Recovery => "recovery",
            Self::Search => "search",
            Self::Status => "status",
            Self::Lifecycle => "lifecycle",
        }
    }
}

/// How a capability participates in its declared phases (design §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextCapabilityMode {
    /// The stock Aleph implementation (default owner).
    Builtin,
    /// Sole owner of the declared phases; failure falls back to builtin.
    Replace,
    /// Runs in declared order alongside other stages; never the final owner.
    Compose,
    /// Subscribes to events/metrics only; cannot change context.
    Observer,
    /// May transform candidates for a declared phase only.
    Interceptor,
    /// Atomic multi-phase owner; must pass capability/scope/budget/health as
    /// one unit and cannot be split into contradictory owners.
    Bundle,
}

impl ContextCapabilityMode {
    /// Every mode, in wire order.
    pub const ALL: [Self; 6] = [
        Self::Builtin,
        Self::Replace,
        Self::Compose,
        Self::Observer,
        Self::Interceptor,
        Self::Bundle,
    ];
}

/// Lifecycle events a capability may subscribe to (design §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextLifecycleEvent {
    SessionStart,
    ToolResultAdmitted,
    BeforePreflight,
    AfterPreflight,
    BeforeCompaction,
    AfterCompaction,
    FoldRecorded,
    RestoreRequested,
    TurnEnd,
    SessionShutdown,
}

impl ContextLifecycleEvent {
    /// Every lifecycle event, in wire order.
    pub const ALL: [Self; 10] = [
        Self::SessionStart,
        Self::ToolResultAdmitted,
        Self::BeforePreflight,
        Self::AfterPreflight,
        Self::BeforeCompaction,
        Self::AfterCompaction,
        Self::FoldRecorded,
        Self::RestoreRequested,
        Self::TurnEnd,
        Self::SessionShutdown,
    ];
}

/// Which namespace a capability's data lives in.
///
/// `CrossSession` is a *declaration*, not a grant: the host must grant it
/// explicitly and audit it before any data crosses a session boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextNamespace {
    Session,
    Project,
    CrossSession,
}

impl ContextNamespace {
    /// How far this namespace reaches. Used to reject a request whose snapshot
    /// claims a wider namespace than the capability declared.
    #[must_use]
    pub const fn reach(self) -> u8 {
        match self {
            Self::Session => 0,
            Self::Project => 1,
            Self::CrossSession => 2,
        }
    }
}

// ============================================================================
// Scope / budget / policy
// ============================================================================

/// The namespace handles a capability declares it may observe.
///
/// Empty `handles` means "the invoking session only" — a capability never gains
/// project or cross-session reach by omission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextScope {
    pub namespace: ContextNamespace,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handles: Vec<String>,
}

impl ContextScope {
    /// Reject handles that identify nothing or identify the same thing twice.
    ///
    /// A duplicate handle would let a later owner-map lookup resolve two
    /// namespaces to one entry, so it is refused here rather than deduplicated.
    pub fn validate(&self) -> Result<(), ContextContractError> {
        for (index, handle) in self.handles.iter().enumerate() {
            if handle.trim().is_empty() {
                return Err(ContextContractError::EmptyScopeHandle);
            }
            if self.handles[..index].contains(handle) {
                return Err(ContextContractError::DuplicateScopeHandle {
                    handle: handle.clone(),
                });
            }
        }
        Ok(())
    }
}

/// A hard input or output ceiling, in both units the pipeline bills in.
///
/// **Wire only.** This is the ceiling a capability publishes in its descriptor
/// and the ceiling a request grants for one call. It is a different fact from
/// [`crate::context::budget::ContextBudget`], which is the mutable runtime
/// pressure state of a live session (token budget, fold nudges, calibrated
/// per-turn snapshots). The two share a name; they are not interchangeable and
/// neither may be constructed from the other here.
///
/// A zero is *unset*, never *unlimited*: [`ContextBudget::validate`] rejects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextBudget {
    pub max_tokens: u64,
    pub max_bytes: u64,
}

impl ContextBudget {
    /// Build a budget. Validation is separate so a descriptor can be
    /// constructed, serialized and rejected with a precise error.
    #[must_use]
    pub const fn new(max_tokens: u64, max_bytes: u64) -> Self {
        Self {
            max_tokens,
            max_bytes,
        }
    }

    /// Reject empty and over-cap budgets.
    pub fn validate(&self) -> Result<(), ContextContractError> {
        if self.max_tokens == 0 || self.max_bytes == 0 {
            return Err(ContextContractError::EmptyBudget);
        }
        if self.max_tokens > CONTEXT_BUDGET_MAX_TOKENS || self.max_bytes > CONTEXT_BUDGET_MAX_BYTES
        {
            return Err(ContextContractError::BudgetExceedsCap {
                tokens: self.max_tokens,
                bytes: self.max_bytes,
            });
        }
        Ok(())
    }

    /// Whether this budget fits inside `ceiling` in both units.
    #[must_use]
    pub const fn fits_within(&self, ceiling: &ContextBudget) -> bool {
        self.max_tokens <= ceiling.max_tokens && self.max_bytes <= ceiling.max_bytes
    }

    /// Fail-closed containment check, naming the declared side that was
    /// exceeded so the error says which ceiling the caller tried to cross.
    fn require_within(
        &self,
        ceiling: &ContextBudget,
        side: &'static str,
    ) -> Result<(), ContextContractError> {
        if self.fits_within(ceiling) {
            Ok(())
        } else {
            Err(ContextContractError::RequestBudgetExceedsDescriptor {
                side,
                requested: *self,
                declared: *ceiling,
            })
        }
    }
}

/// What a capability may persist.
///
/// `session_events` is the only durable truth. A capability that persists must
/// declare its state rebuildable from those events; otherwise its checkpoint
/// would become a second event store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextPersistencePolicy {
    pub persists: bool,
    pub rebuildable_from_events: bool,
}

/// Side effects a capability declares. Everything defaults to denied; the host
/// grant (Task 2) is the authority that can allow any of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextSideEffectPermissions {
    pub network_egress: bool,
    pub project_write: bool,
    pub cross_session_read: bool,
}

impl ContextSideEffectPermissions {
    /// True when the capability declares no side effect at all.
    #[must_use]
    pub const fn is_pure(&self) -> bool {
        !self.network_egress && !self.project_write && !self.cross_session_read
    }
}

// ============================================================================
// Descriptor
// ============================================================================

/// The complete capability declaration — the single source every projection
/// derives from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextCapabilityDescriptor {
    /// Stable capability id, namespaced by owner (e.g. `acme/context-compactor`).
    pub id: String,
    /// Wire contract version; must equal [`CONTEXT_CAPABILITY_API_VERSION`].
    pub api_version: String,
    /// The implementation's own version (semver-ish, opaque to Core).
    pub implementation_version: String,
    /// Phases this capability claims. Must be non-empty and duplicate-free.
    pub phases: Vec<ContextPhase>,
    pub mode: ContextCapabilityMode,
    pub scope: ContextScope,
    pub input_budget: ContextBudget,
    pub output_budget: ContextBudget,
    /// Omitted on the wire decodes as "persists nothing" — the denied default.
    #[serde(default)]
    pub persistence: ContextPersistencePolicy,
    /// Omitted on the wire decodes as all-denied — the fail-closed default.
    #[serde(default)]
    pub permissions: ContextSideEffectPermissions,
    /// Lifecycle events subscribed to. Empty is valid (no subscription).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle: Vec<ContextLifecycleEvent>,
    /// Required in `Bundle` mode; the bundle installs and is removed atomically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
}

impl ContextCapabilityDescriptor {
    /// Whether this capability declares `phase`.
    #[must_use]
    pub fn declares(&self, phase: ContextPhase) -> bool {
        self.phases.contains(&phase)
    }

    /// Reject a request for a phase this capability never declared.
    pub fn require_phase(&self, phase: ContextPhase) -> Result<(), ContextContractError> {
        if self.declares(phase) {
            Ok(())
        } else {
            Err(ContextContractError::UndeclaredPhase {
                capability_id: self.id.clone(),
                phase,
            })
        }
    }

    /// Validate the declaration itself, before any call is admitted.
    pub fn validate(&self) -> Result<(), ContextContractError> {
        if self.id.is_empty() {
            return Err(ContextContractError::EmptyId);
        }
        if self.api_version != CONTEXT_CAPABILITY_API_VERSION {
            return Err(ContextContractError::UnsupportedApiVersion {
                found: self.api_version.clone(),
            });
        }
        if self.phases.is_empty() {
            return Err(ContextContractError::NoPhase);
        }
        for (index, phase) in self.phases.iter().enumerate() {
            if self.phases[..index].contains(phase) {
                return Err(ContextContractError::DuplicatePhase { phase: *phase });
            }
        }
        self.scope.validate()?;
        // Bidirectional: a cross-session namespace without the permission is an
        // undeclared reach, and the permission without the namespace is a
        // declared reach with nowhere to apply it. Both are contradictions.
        if (self.scope.namespace == ContextNamespace::CrossSession)
            != self.permissions.cross_session_read
        {
            return Err(ContextContractError::CrossSessionConsistency {
                namespace: self.scope.namespace,
                cross_session_read: self.permissions.cross_session_read,
            });
        }
        self.input_budget.validate()?;
        self.output_budget.validate()?;
        if self.persistence.persists && !self.persistence.rebuildable_from_events {
            return Err(ContextContractError::NonRebuildablePersistence);
        }
        if self.mode == ContextCapabilityMode::Bundle && self.bundle_id.is_none() {
            return Err(ContextContractError::MissingBundleId);
        }
        Ok(())
    }
}

// ============================================================================
// Requests, snapshots, results
// ============================================================================

/// A versioned operation name on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextOperation {
    Describe,
    Admit,
    Preflight,
    Compact,
    Restore,
    Search,
    Status,
    Shutdown,
}

impl ContextOperation {
    /// Every operation, in wire order.
    pub const ALL: [Self; 8] = [
        Self::Describe,
        Self::Admit,
        Self::Preflight,
        Self::Compact,
        Self::Restore,
        Self::Search,
        Self::Status,
        Self::Shutdown,
    ];

    /// The unversioned operation tag, also its serde representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Describe => "describe",
            Self::Admit => "admit",
            Self::Preflight => "preflight",
            Self::Compact => "compact",
            Self::Restore => "restore",
            Self::Search => "search",
            Self::Status => "status",
            Self::Shutdown => "shutdown",
        }
    }

    /// The versioned wire name, `"context.fabric/v1/<operation>"`.
    #[must_use]
    pub fn wire_name(self) -> String {
        format!("{CONTEXT_CAPABILITY_API_VERSION}/{}", self.as_str())
    }

    /// Whether this operation transforms context and therefore cannot run
    /// without a [`ContextSnapshot`].
    ///
    /// `describe` reports a descriptor, `status` reports pressure/health and
    /// `shutdown` disposes the capability: none of them read context, so none of
    /// them take a snapshot. Everything else (`admit`, `preflight`, `compact`,
    /// `restore`, `search`) operates on context and is rejected without one.
    #[must_use]
    pub const fn requires_snapshot(self) -> bool {
        match self {
            Self::Describe | Self::Status | Self::Shutdown => false,
            Self::Admit | Self::Preflight | Self::Compact | Self::Restore | Self::Search => true,
        }
    }

    /// The phase this operation requires, or `None` for the meta-operation
    /// `describe` (which may describe any declared phase).
    #[must_use]
    pub const fn required_phase(self) -> Option<ContextPhase> {
        match self {
            Self::Describe => None,
            Self::Admit => Some(ContextPhase::Admission),
            Self::Preflight => Some(ContextPhase::Preflight),
            Self::Compact => Some(ContextPhase::Compaction),
            Self::Restore => Some(ContextPhase::Recovery),
            Self::Search => Some(ContextPhase::Search),
            Self::Status => Some(ContextPhase::Status),
            Self::Shutdown => Some(ContextPhase::Lifecycle),
        }
    }
}

/// A bounded view of the current context, scoped to what a capability declared.
///
/// `payload` is deliberately schema-less here so this module does not own a
/// second message DTO.
///
/// **Contract requirement, not an enforced invariant:** whoever builds a
/// snapshot must already have removed hidden reasoning, credentials,
/// unauthorised project files and other sessions from `payload`. This type
/// cannot check that — `serde_json::Value` carries no provenance — and no such
/// filter is implemented yet. What *is* enforced is the envelope: the declared
/// [`ContextScope`] may not exceed the capability's namespace, its handles must
/// be declared, `token_count`/`byte_count` must fit `input_budget`, and
/// `input_fingerprint` must match the request. Payload contents are not
/// validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextSnapshot {
    pub snapshot_id: String,
    pub session_id: String,
    /// Fingerprint of what the snapshot was cut from, so a result can be
    /// matched to the exact input it transformed.
    pub input_fingerprint: String,
    pub scope: ContextScope,
    pub token_count: u64,
    pub byte_count: u64,
    pub payload: serde_json::Value,
}

impl ContextSnapshot {
    /// Validate the snapshot envelope against the capability it is sent to and
    /// the request that carries it.
    ///
    /// Payload *contents* are out of reach here; see the type-level note.
    pub fn validate_against(
        &self,
        descriptor: &ContextCapabilityDescriptor,
        request: &ContextCapabilityRequest,
    ) -> Result<(), ContextContractError> {
        if self.input_fingerprint.trim().is_empty() {
            return Err(ContextContractError::EmptyFingerprint {
                field: "snapshot.input_fingerprint",
            });
        }
        if self.input_fingerprint != request.input_fingerprint {
            return Err(ContextContractError::FingerprintMismatch {
                expected: request.input_fingerprint.clone(),
                found: self.input_fingerprint.clone(),
            });
        }
        if self.token_count > descriptor.input_budget.max_tokens
            || self.byte_count > descriptor.input_budget.max_bytes
        {
            return Err(ContextContractError::SnapshotExceedsInputBudget {
                tokens: self.token_count,
                bytes: self.byte_count,
                max_tokens: descriptor.input_budget.max_tokens,
                max_bytes: descriptor.input_budget.max_bytes,
            });
        }
        self.scope.validate()?;
        if self.scope.namespace.reach() > descriptor.scope.namespace.reach() {
            return Err(ContextContractError::ScopeEscalation {
                declared: descriptor.scope.namespace,
                requested: self.scope.namespace,
            });
        }
        for handle in &self.scope.handles {
            if descriptor.scope.handles.contains(handle) {
                continue;
            }
            // Empty declared handles mean "the invoking session only"; that is
            // the one handle a snapshot may name without it being declared.
            let invoking_session = descriptor.scope.handles.is_empty()
                && descriptor.scope.namespace == ContextNamespace::Session
                && *handle == self.session_id;
            if !invoking_session {
                return Err(ContextContractError::ScopeHandleNotDeclared {
                    handle: handle.clone(),
                });
            }
        }
        Ok(())
    }
}

/// One wire call into a capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextCapabilityRequest {
    pub request_id: String,
    pub capability_id: String,
    pub api_version: String,
    pub phase: ContextPhase,
    pub operation: ContextOperation,
    /// Per-call ceiling the caller grants for this call's result. It may not
    /// exceed either declared ceiling of the capability (input or output): a
    /// caller cannot grant more than the capability declared it can take in or
    /// emit. A zero budget is rejected, not read as unlimited.
    pub budget: ContextBudget,
    pub input_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ContextSnapshot>,
}

impl ContextCapabilityRequest {
    /// Validate this call against the descriptor it names.
    ///
    /// Fail-closed, in this order: the request must target this capability on
    /// this wire version; the phase must be declared and must be the phase the
    /// operation belongs to; the budget must be non-empty, in-cap and contained
    /// in *both* declared budgets; the input fingerprint must be present; the
    /// snapshot must exist exactly for the operations that transform context
    /// (`admit`/`preflight`/`compact`/`restore`/`search`) and must be absent for
    /// `describe`/`status`/`shutdown`; and the snapshot itself must agree with
    /// the request and stay inside the declared scope and input budget.
    pub fn validate_against(
        &self,
        descriptor: &ContextCapabilityDescriptor,
    ) -> Result<(), ContextContractError> {
        if self.capability_id != descriptor.id {
            return Err(ContextContractError::CapabilityMismatch {
                requested: self.capability_id.clone(),
                descriptor: descriptor.id.clone(),
            });
        }
        if self.api_version != CONTEXT_CAPABILITY_API_VERSION {
            return Err(ContextContractError::UnsupportedApiVersion {
                found: self.api_version.clone(),
            });
        }
        descriptor.require_phase(self.phase)?;
        if let Some(required) = self.operation.required_phase() {
            if required != self.phase {
                return Err(ContextContractError::PhaseOperationMismatch {
                    operation: self.operation,
                    required,
                    declared: self.phase,
                });
            }
        }
        self.budget.validate()?;
        if self.input_fingerprint.trim().is_empty() {
            return Err(ContextContractError::EmptyFingerprint {
                field: "request.input_fingerprint",
            });
        }
        // A caller-declared ceiling above what the capability published is a
        // contract violation in both directions, not a request to be clamped.
        self.budget
            .require_within(&descriptor.input_budget, "input")?;
        self.budget
            .require_within(&descriptor.output_budget, "output")?;
        match (self.operation.requires_snapshot(), self.snapshot.as_ref()) {
            (true, None) => {
                return Err(ContextContractError::MissingSnapshot {
                    operation: self.operation,
                })
            }
            (false, Some(_)) => {
                return Err(ContextContractError::UnexpectedSnapshot {
                    operation: self.operation,
                })
            }
            _ => {}
        }
        if let Some(snapshot) = self.snapshot.as_ref() {
            snapshot.validate_against(descriptor, self)?;
        }
        Ok(())
    }
}

/// Continuation handle for a paginated result (`restore`, `search`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextContinuation {
    pub cursor: String,
    pub has_more: bool,
}

/// How much of the source a transform preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextQuality {
    /// Byte-exact with respect to the requested span.
    Exact,
    /// Preserved shape, condensed content.
    Condensed,
    /// Some content was dropped; recoverable only through `restore`.
    Lossy,
}

/// Why a call did not run as declared. Always surfaced to the agent and to
/// diagnostics — a fallback that hides itself is a lie about what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DegradedReason {
    /// The call failed and the bounded builtin path ran instead.
    BuiltinFallback,
    BudgetExceeded,
    ScopeDenied,
    Timeout,
    InvalidContract,
    Unavailable,
}

/// The bounded result of a preflight/compaction transform.
///
/// A capability may not claim to have changed Aleph state: only Core records a
/// fold after verifying the effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextTransformResult {
    /// Fingerprint of the input this result was produced from.
    pub input_fingerprint: String,
    pub output: serde_json::Value,
    pub output_bytes: u64,
    pub output_tokens: u64,
    /// Named strategy, so a result can be attributed in diagnostics.
    pub strategy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ContextContinuation>,
    pub quality: ContextQuality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<DegradedReason>,
}

impl ContextTransformResult {
    /// Enforce the declared output ceiling. An over-budget result is rejected
    /// as a contract violation; it is never truncated into the prompt silently.
    pub fn validate(&self, budget: &ContextBudget) -> Result<(), ContextContractError> {
        if self.input_fingerprint.trim().is_empty() {
            return Err(ContextContractError::EmptyFingerprint {
                field: "result.input_fingerprint",
            });
        }
        budget.validate()?;
        if self.output_bytes > budget.max_bytes || self.output_tokens > budget.max_tokens {
            return Err(ContextContractError::OutputExceedsBudget {
                bytes: self.output_bytes,
                tokens: self.output_tokens,
            });
        }
        Ok(())
    }

    /// Validate this result against the request it answers.
    ///
    /// The result must carry the request's input fingerprint — a transform
    /// whose provenance cannot be matched to the exact input it consumed is not
    /// attributable, and a stale result would otherwise be accepted as if it
    /// described the current input. The result is then held to the request's
    /// per-call budget, which `validate_against` has already constrained to the
    /// descriptor's declared ceilings.
    pub fn validate_against(
        &self,
        request: &ContextCapabilityRequest,
    ) -> Result<(), ContextContractError> {
        if self.input_fingerprint.trim().is_empty() {
            return Err(ContextContractError::EmptyFingerprint {
                field: "result.input_fingerprint",
            });
        }
        if self.input_fingerprint != request.input_fingerprint {
            return Err(ContextContractError::FingerprintMismatch {
                expected: request.input_fingerprint.clone(),
                found: self.input_fingerprint.clone(),
            });
        }
        self.validate(&request.budget)
    }
}

// ============================================================================
// Contract errors
// ============================================================================

/// Every way a context capability declaration or call can violate the contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContextContractError {
    #[error("context capability id must not be empty")]
    EmptyId,
    #[error("unsupported context capability api version: {found}")]
    UnsupportedApiVersion { found: String },
    #[error("context capability declares no phase")]
    NoPhase,
    #[error("context capability declares phase {phase:?} twice")]
    DuplicatePhase { phase: ContextPhase },
    #[error("context capability {capability_id} does not declare phase {phase:?}")]
    UndeclaredPhase {
        capability_id: String,
        phase: ContextPhase,
    },
    #[error("request targets capability {requested}, descriptor is {descriptor}")]
    CapabilityMismatch {
        requested: String,
        descriptor: String,
    },
    #[error("operation {operation:?} requires phase {required:?}, request declares {declared:?}")]
    PhaseOperationMismatch {
        operation: ContextOperation,
        required: ContextPhase,
        declared: ContextPhase,
    },
    #[error("context budget must be non-zero in both units")]
    EmptyBudget,
    #[error("context budget exceeds the hard cap: {tokens} tokens / {bytes} bytes")]
    BudgetExceedsCap { tokens: u64, bytes: u64 },
    #[error("persisted context state must be rebuildable from session events")]
    NonRebuildablePersistence,
    #[error("bundle mode requires a bundle id")]
    MissingBundleId,
    #[error("context result exceeds its output budget: {bytes} bytes / {tokens} tokens")]
    OutputExceedsBudget { bytes: u64, tokens: u64 },
    #[error("scope handle must not be empty")]
    EmptyScopeHandle,
    #[error("scope declares handle {handle} twice")]
    DuplicateScopeHandle { handle: String },
    #[error(
        "cross-session namespace/permission disagree: namespace {namespace:?}, cross_session_read {cross_session_read}"
    )]
    CrossSessionConsistency {
        namespace: ContextNamespace,
        cross_session_read: bool,
    },
    #[error(
        "request budget exceeds the descriptor's declared {side} budget: requested {requested:?}, declared {declared:?}"
    )]
    RequestBudgetExceedsDescriptor {
        side: &'static str,
        requested: ContextBudget,
        declared: ContextBudget,
    },
    #[error(
        "snapshot exceeds the declared input budget: {tokens} tokens / {bytes} bytes (max {max_tokens} / {max_bytes})"
    )]
    SnapshotExceedsInputBudget {
        tokens: u64,
        bytes: u64,
        max_tokens: u64,
        max_bytes: u64,
    },
    #[error("operation {operation:?} requires a context snapshot")]
    MissingSnapshot { operation: ContextOperation },
    #[error("operation {operation:?} does not take a context snapshot")]
    UnexpectedSnapshot { operation: ContextOperation },
    #[error("{field} must not be empty; an empty fingerprint cannot be verified")]
    EmptyFingerprint { field: &'static str },
    #[error("input fingerprint mismatch: expected {expected}, found {found}")]
    FingerprintMismatch { expected: String, found: String },
    #[error("snapshot scope {requested:?} exceeds the declared scope {declared:?}")]
    ScopeEscalation {
        declared: ContextNamespace,
        requested: ContextNamespace,
    },
    #[error("snapshot names scope handle {handle}, which the capability did not declare")]
    ScopeHandleNotDeclared { handle: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::budget::preflight::PreflightStage;
    use crate::context::budget::ContextPressure;
    use crate::providers::message::UnifiedMessage;
    use serde_json::{json, Value};

    fn descriptor() -> ContextCapabilityDescriptor {
        ContextCapabilityDescriptor {
            id: "acme/context-compactor".to_string(),
            api_version: CONTEXT_CAPABILITY_API_VERSION.to_string(),
            implementation_version: "1.4.2".to_string(),
            phases: vec![ContextPhase::Compaction, ContextPhase::Recovery],
            mode: ContextCapabilityMode::Replace,
            scope: ContextScope {
                namespace: ContextNamespace::Session,
                handles: Vec::new(),
            },
            input_budget: ContextBudget::new(120_000, 1_048_576),
            output_budget: ContextBudget::new(24_000, 262_144),
            persistence: ContextPersistencePolicy {
                persists: true,
                rebuildable_from_events: true,
            },
            permissions: ContextSideEffectPermissions::default(),
            lifecycle: vec![
                ContextLifecycleEvent::BeforeCompaction,
                ContextLifecycleEvent::AfterCompaction,
            ],
            bundle_id: None,
        }
    }

    fn request(phase: ContextPhase, operation: ContextOperation) -> ContextCapabilityRequest {
        ContextCapabilityRequest {
            request_id: "req-1".to_string(),
            capability_id: "acme/context-compactor".to_string(),
            api_version: CONTEXT_CAPABILITY_API_VERSION.to_string(),
            phase,
            operation,
            budget: ContextBudget::new(24_000, 262_144),
            input_fingerprint: "sha256:deadbeef".to_string(),
            snapshot: None,
        }
    }

    fn snapshot() -> ContextSnapshot {
        ContextSnapshot {
            snapshot_id: "snap-1".to_string(),
            session_id: "sess-1".to_string(),
            input_fingerprint: "sha256:deadbeef".to_string(),
            scope: ContextScope {
                namespace: ContextNamespace::Session,
                handles: Vec::new(),
            },
            token_count: 900,
            byte_count: 40_000,
            payload: json!({"messages": []}),
        }
    }

    /// A request for an operation that transforms context, so it carries the
    /// snapshot those operations require.
    fn transforming_request() -> ContextCapabilityRequest {
        let mut req = request(ContextPhase::Compaction, ContextOperation::Compact);
        req.snapshot = Some(snapshot());
        req
    }

    fn all_phase_descriptor() -> ContextCapabilityDescriptor {
        let mut descriptor = descriptor();
        descriptor.id = "acme/all-phases".to_string();
        descriptor.phases = ContextPhase::ALL.to_vec();
        descriptor.mode = ContextCapabilityMode::Compose;
        descriptor
    }

    fn transform_result(bytes: u64, tokens: u64) -> ContextTransformResult {
        ContextTransformResult {
            input_fingerprint: "sha256:deadbeef".to_string(),
            output: json!({"messages": []}),
            output_bytes: bytes,
            output_tokens: tokens,
            strategy: "acme.dedup.v1".to_string(),
            continuation: None,
            quality: ContextQuality::Condensed,
            degraded: None,
        }
    }

    // ------------------------------------------------------------------
    // serde round-trip
    // ------------------------------------------------------------------

    #[test]
    fn descriptor_round_trips_through_json() {
        let original = descriptor();
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: ContextCapabilityDescriptor =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, original);
        assert_eq!(decoded.validate(), Ok(()));
    }

    #[test]
    fn request_and_result_round_trip_through_json() {
        let mut req = request(ContextPhase::Compaction, ContextOperation::Compact);
        req.snapshot = Some(ContextSnapshot {
            snapshot_id: "snap-1".to_string(),
            session_id: "sess-1".to_string(),
            input_fingerprint: "sha256:deadbeef".to_string(),
            scope: ContextScope {
                namespace: ContextNamespace::Session,
                handles: vec!["sess-1".to_string()],
            },
            token_count: 900,
            byte_count: 40_000,
            payload: json!({"messages": [{"role": "user", "content": "hi"}]}),
        });
        let json = serde_json::to_string(&req).expect("serialize");
        let decoded: ContextCapabilityRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, req);
        assert_eq!(decoded.validate_against(&descriptor()), Ok(()));

        let result = transform_result(1024, 256);
        let json = serde_json::to_string(&result).expect("serialize");
        let decoded: ContextTransformResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, result);
        assert_eq!(decoded.validate_against(&req), Ok(()));
    }

    // ------------------------------------------------------------------
    // schemars
    // ------------------------------------------------------------------

    #[test]
    fn descriptor_schema_is_generated() {
        let schema = schemars::schema_for!(ContextCapabilityDescriptor);
        let value = serde_json::to_value(&schema).expect("schema json");
        let text = value.to_string();

        for property in [
            "id",
            "api_version",
            "implementation_version",
            "phases",
            "mode",
            "scope",
            "input_budget",
            "output_budget",
            "persistence",
            "permissions",
            "lifecycle",
            "bundle_id",
        ] {
            assert!(
                text.contains(&format!("\"{property}\"")),
                "schema is missing the `{property}` property: {text}"
            );
        }
        assert!(text.contains("\"admission\""));
        assert!(text.contains("\"compaction\""));
        assert!(text.contains("\"max_tokens\""));
        assert!(text.contains("\"max_bytes\""));
    }

    #[test]
    fn request_and_result_schemas_are_generated() {
        let request_schema =
            serde_json::to_value(schemars::schema_for!(ContextCapabilityRequest)).expect("schema");
        let text = request_schema.to_string();
        for property in [
            "request_id",
            "capability_id",
            "api_version",
            "phase",
            "operation",
            "budget",
            "input_fingerprint",
            "snapshot",
        ] {
            assert!(
                text.contains(&format!("\"{property}\"")),
                "missing {property}"
            );
        }

        let result_schema =
            serde_json::to_value(schemars::schema_for!(ContextTransformResult)).expect("schema");
        let text = result_schema.to_string();
        for property in [
            "input_fingerprint",
            "output",
            "output_bytes",
            "output_tokens",
            "strategy",
            "continuation",
            "quality",
            "degraded",
        ] {
            assert!(
                text.contains(&format!("\"{property}\"")),
                "missing {property}"
            );
        }
    }

    // ------------------------------------------------------------------
    // forward compatibility: unknown optional fields are ignored
    // ------------------------------------------------------------------

    #[test]
    fn unknown_optional_fields_are_ignored() {
        let wire = json!({
            "id": "acme/context-compactor",
            "api_version": CONTEXT_CAPABILITY_API_VERSION,
            "implementation_version": "1.4.2",
            "phases": ["compaction"],
            "mode": "replace",
            "scope": {"namespace": "session", "future_handle_kind": "thread"},
            "input_budget": {"max_tokens": 1000, "max_bytes": 2000},
            "output_budget": {"max_tokens": 500, "max_bytes": 1000},
            "persistence": {"persists": false, "rebuildable_from_events": false},
            "permissions": {"network_egress": false, "project_write": false, "cross_session_read": false},
            "future_capability_flag": true
        });

        let decoded: ContextCapabilityDescriptor =
            serde_json::from_value(wire).expect("unknown fields must be ignored");
        assert_eq!(decoded.lifecycle, Vec::new());
        assert_eq!(decoded.bundle_id, None);
        assert_eq!(decoded.scope.handles, Vec::<String>::new());
        assert_eq!(decoded.validate(), Ok(()));

        let request_wire = json!({
            "request_id": "req-1",
            "capability_id": "acme/context-compactor",
            "api_version": CONTEXT_CAPABILITY_API_VERSION,
            "phase": "compaction",
            "operation": "compact",
            "budget": {"max_tokens": 1000, "max_bytes": 2000},
            "input_fingerprint": "sha256:deadbeef",
            "future_request_flag": {"nested": true}
        });
        let request: ContextCapabilityRequest =
            serde_json::from_value(request_wire).expect("unknown fields must be ignored");
        assert_eq!(request.snapshot, None);
    }

    // ------------------------------------------------------------------
    // stable tags
    // ------------------------------------------------------------------

    #[test]
    fn phase_tags_are_stable() {
        let expected = [
            (ContextPhase::Admission, "admission"),
            (ContextPhase::Preflight, "preflight"),
            (ContextPhase::Compaction, "compaction"),
            (ContextPhase::Recovery, "recovery"),
            (ContextPhase::Search, "search"),
            (ContextPhase::Status, "status"),
            (ContextPhase::Lifecycle, "lifecycle"),
        ];
        assert_eq!(expected.len(), ContextPhase::ALL.len());
        for (phase, tag) in expected {
            assert_eq!(
                serde_json::to_value(phase).expect("phase json"),
                Value::String(tag.to_string())
            );
            assert_eq!(phase.as_str(), tag);
        }
    }

    #[test]
    fn mode_tags_are_stable() {
        let expected = [
            (ContextCapabilityMode::Builtin, "builtin"),
            (ContextCapabilityMode::Replace, "replace"),
            (ContextCapabilityMode::Compose, "compose"),
            (ContextCapabilityMode::Observer, "observer"),
            (ContextCapabilityMode::Interceptor, "interceptor"),
            (ContextCapabilityMode::Bundle, "bundle"),
        ];
        assert_eq!(expected.len(), ContextCapabilityMode::ALL.len());
        for (mode, tag) in expected {
            assert_eq!(
                serde_json::to_value(mode).expect("mode json"),
                Value::String(tag.to_string())
            );
        }
    }

    #[test]
    fn lifecycle_and_scope_tags_are_stable() {
        let expected = [
            (ContextLifecycleEvent::SessionStart, "session_start"),
            (
                ContextLifecycleEvent::ToolResultAdmitted,
                "tool_result_admitted",
            ),
            (ContextLifecycleEvent::BeforePreflight, "before_preflight"),
            (ContextLifecycleEvent::AfterPreflight, "after_preflight"),
            (ContextLifecycleEvent::BeforeCompaction, "before_compaction"),
            (ContextLifecycleEvent::AfterCompaction, "after_compaction"),
            (ContextLifecycleEvent::FoldRecorded, "fold_recorded"),
            (ContextLifecycleEvent::RestoreRequested, "restore_requested"),
            (ContextLifecycleEvent::TurnEnd, "turn_end"),
            (ContextLifecycleEvent::SessionShutdown, "session_shutdown"),
        ];
        assert_eq!(expected.len(), ContextLifecycleEvent::ALL.len());
        for (event, tag) in expected {
            assert_eq!(
                serde_json::to_value(event).expect("lifecycle json"),
                Value::String(tag.to_string())
            );
        }

        assert_eq!(
            serde_json::to_value(ContextNamespace::Session).expect("namespace json"),
            Value::String("session".to_string())
        );
        assert_eq!(
            serde_json::to_value(ContextNamespace::Project).expect("namespace json"),
            Value::String("project".to_string())
        );
        assert_eq!(
            serde_json::to_value(ContextNamespace::CrossSession).expect("namespace json"),
            Value::String("cross_session".to_string())
        );

        let degraded = [
            (DegradedReason::BuiltinFallback, "builtin_fallback"),
            (DegradedReason::BudgetExceeded, "budget_exceeded"),
            (DegradedReason::ScopeDenied, "scope_denied"),
            (DegradedReason::Timeout, "timeout"),
            (DegradedReason::InvalidContract, "invalid_contract"),
            (DegradedReason::Unavailable, "unavailable"),
        ];
        for (reason, tag) in degraded {
            assert_eq!(
                serde_json::to_value(reason).expect("degraded json"),
                Value::String(tag.to_string())
            );
        }

        assert_eq!(
            serde_json::to_value(ContextQuality::Exact).expect("quality json"),
            Value::String("exact".to_string())
        );
        assert_eq!(
            serde_json::to_value(ContextQuality::Condensed).expect("quality json"),
            Value::String("condensed".to_string())
        );
        assert_eq!(
            serde_json::to_value(ContextQuality::Lossy).expect("quality json"),
            Value::String("lossy".to_string())
        );
    }

    // ------------------------------------------------------------------
    // bounded budgets
    // ------------------------------------------------------------------

    #[test]
    fn budget_serializes_as_bounded_numbers() {
        let budget = ContextBudget::new(120_000, 1_048_576);
        assert_eq!(
            serde_json::to_string(&budget).expect("budget json"),
            r#"{"max_tokens":120000,"max_bytes":1048576}"#
        );
        assert_eq!(budget.validate(), Ok(()));

        assert_eq!(
            ContextBudget::new(0, 0).validate(),
            Err(ContextContractError::EmptyBudget)
        );

        let over_tokens = ContextBudget::new(CONTEXT_BUDGET_MAX_TOKENS + 1, 1);
        assert_eq!(
            over_tokens.validate(),
            Err(ContextContractError::BudgetExceedsCap {
                tokens: CONTEXT_BUDGET_MAX_TOKENS + 1,
                bytes: 1,
            })
        );

        let over_bytes = ContextBudget::new(1, CONTEXT_BUDGET_MAX_BYTES + 1);
        assert_eq!(
            over_bytes.validate(),
            Err(ContextContractError::BudgetExceedsCap {
                tokens: 1,
                bytes: CONTEXT_BUDGET_MAX_BYTES + 1,
            })
        );
    }

    #[test]
    fn descriptor_rejects_oversized_budget() {
        let mut descriptor = descriptor();
        descriptor.input_budget = ContextBudget::new(CONTEXT_BUDGET_MAX_TOKENS + 1, 1);
        assert!(matches!(
            descriptor.validate(),
            Err(ContextContractError::BudgetExceedsCap { .. })
        ));
    }

    #[test]
    fn oversized_result_is_rejected() {
        let budget = ContextBudget::new(24_000, 262_144);
        assert_eq!(transform_result(262_144, 24_000).validate(&budget), Ok(()));
        assert_eq!(
            transform_result(262_145, 10).validate(&budget),
            Err(ContextContractError::OutputExceedsBudget {
                bytes: 262_145,
                tokens: 10,
            })
        );
        assert_eq!(
            transform_result(10, 24_001).validate(&budget),
            Err(ContextContractError::OutputExceedsBudget {
                bytes: 10,
                tokens: 24_001,
            })
        );
    }

    // ------------------------------------------------------------------
    // rejection of undeclared phases and operation mismatches
    // ------------------------------------------------------------------

    #[test]
    fn undeclared_phase_is_rejected() {
        let descriptor = descriptor();
        let wrong = request(ContextPhase::Search, ContextOperation::Search);
        assert_eq!(
            wrong.validate_against(&descriptor),
            Err(ContextContractError::UndeclaredPhase {
                capability_id: "acme/context-compactor".to_string(),
                phase: ContextPhase::Search,
            })
        );

        let declared = transforming_request();
        assert_eq!(declared.validate_against(&descriptor), Ok(()));
    }

    #[test]
    fn operation_phase_mismatch_is_rejected() {
        let mismatch = request(ContextPhase::Compaction, ContextOperation::Restore);
        assert_eq!(
            mismatch.validate_against(&descriptor()),
            Err(ContextContractError::PhaseOperationMismatch {
                operation: ContextOperation::Restore,
                required: ContextPhase::Recovery,
                declared: ContextPhase::Compaction,
            })
        );
    }

    #[test]
    fn request_for_another_capability_is_rejected() {
        let mut req = request(ContextPhase::Compaction, ContextOperation::Compact);
        req.capability_id = "acme/other".to_string();
        assert_eq!(
            req.validate_against(&descriptor()),
            Err(ContextContractError::CapabilityMismatch {
                requested: "acme/other".to_string(),
                descriptor: "acme/context-compactor".to_string(),
            })
        );
    }

    #[test]
    fn unsupported_api_version_is_rejected() {
        let mut descriptor = descriptor();
        descriptor.api_version = "context.fabric/v0".to_string();
        assert_eq!(
            descriptor.validate(),
            Err(ContextContractError::UnsupportedApiVersion {
                found: "context.fabric/v0".to_string(),
            })
        );
    }

    #[test]
    fn descriptor_contract_is_enforced() {
        let mut empty_phases = descriptor();
        empty_phases.phases.clear();
        assert_eq!(empty_phases.validate(), Err(ContextContractError::NoPhase));

        let mut duplicate = descriptor();
        duplicate.phases.push(ContextPhase::Compaction);
        assert_eq!(
            duplicate.validate(),
            Err(ContextContractError::DuplicatePhase {
                phase: ContextPhase::Compaction,
            })
        );

        let mut not_rebuildable = descriptor();
        not_rebuildable.persistence = ContextPersistencePolicy {
            persists: true,
            rebuildable_from_events: false,
        };
        assert_eq!(
            not_rebuildable.validate(),
            Err(ContextContractError::NonRebuildablePersistence)
        );

        let mut bundle = descriptor();
        bundle.mode = ContextCapabilityMode::Bundle;
        bundle.bundle_id = None;
        assert_eq!(
            bundle.validate(),
            Err(ContextContractError::MissingBundleId)
        );

        let mut empty_id = descriptor();
        empty_id.id = String::new();
        assert_eq!(empty_id.validate(), Err(ContextContractError::EmptyId));
    }

    // ------------------------------------------------------------------
    // versioned operation names
    // ------------------------------------------------------------------

    #[test]
    fn operation_names_are_versioned() {
        let expected = [
            (ContextOperation::Describe, "describe", None),
            (
                ContextOperation::Admit,
                "admit",
                Some(ContextPhase::Admission),
            ),
            (
                ContextOperation::Preflight,
                "preflight",
                Some(ContextPhase::Preflight),
            ),
            (
                ContextOperation::Compact,
                "compact",
                Some(ContextPhase::Compaction),
            ),
            (
                ContextOperation::Restore,
                "restore",
                Some(ContextPhase::Recovery),
            ),
            (
                ContextOperation::Search,
                "search",
                Some(ContextPhase::Search),
            ),
            (
                ContextOperation::Status,
                "status",
                Some(ContextPhase::Status),
            ),
            (
                ContextOperation::Shutdown,
                "shutdown",
                Some(ContextPhase::Lifecycle),
            ),
        ];
        assert_eq!(expected.len(), ContextOperation::ALL.len());
        for (operation, name, phase) in expected {
            assert_eq!(operation.as_str(), name);
            assert_eq!(
                operation.wire_name(),
                format!("{CONTEXT_CAPABILITY_API_VERSION}/{name}")
            );
            assert_eq!(operation.required_phase(), phase);
            assert_eq!(
                serde_json::to_value(operation).expect("operation json"),
                Value::String(name.to_string())
            );
        }
        assert_eq!(
            ContextOperation::Shutdown.wire_name(),
            "context.fabric/v1/shutdown"
        );
    }

    // ------------------------------------------------------------------
    // budgets: request vs descriptor, snapshot vs input budget
    // ------------------------------------------------------------------

    #[test]
    fn request_budget_must_fit_descriptor_ceilings() {
        let descriptor = descriptor();

        let mut over_input = transforming_request();
        over_input.budget = ContextBudget::new(descriptor.input_budget.max_tokens + 1, 262_144);
        assert_eq!(
            over_input.validate_against(&descriptor),
            Err(ContextContractError::RequestBudgetExceedsDescriptor {
                side: "input",
                requested: ContextBudget::new(descriptor.input_budget.max_tokens + 1, 262_144),
                declared: descriptor.input_budget,
            })
        );

        let mut over_output = transforming_request();
        over_output.budget = ContextBudget::new(24_000, descriptor.input_budget.max_bytes);
        assert_eq!(
            over_output.validate_against(&descriptor),
            Err(ContextContractError::RequestBudgetExceedsDescriptor {
                side: "output",
                requested: ContextBudget::new(24_000, descriptor.input_budget.max_bytes),
                declared: descriptor.output_budget,
            })
        );

        // The fixture budget itself is accepted.
        assert_eq!(transforming_request().validate_against(&descriptor), Ok(()));
    }

    #[test]
    fn snapshot_must_fit_the_declared_input_budget() {
        let descriptor = descriptor();

        let mut too_many_bytes = transforming_request();
        too_many_bytes
            .snapshot
            .as_mut()
            .expect("snapshot")
            .byte_count = descriptor.input_budget.max_bytes + 1;
        assert_eq!(
            too_many_bytes.validate_against(&descriptor),
            Err(ContextContractError::SnapshotExceedsInputBudget {
                tokens: 900,
                bytes: descriptor.input_budget.max_bytes + 1,
                max_tokens: descriptor.input_budget.max_tokens,
                max_bytes: descriptor.input_budget.max_bytes,
            })
        );

        let mut too_many_tokens = transforming_request();
        too_many_tokens
            .snapshot
            .as_mut()
            .expect("snapshot")
            .token_count = descriptor.input_budget.max_tokens + 1;
        assert!(matches!(
            too_many_tokens.validate_against(&descriptor),
            Err(ContextContractError::SnapshotExceedsInputBudget { .. })
        ));
    }

    // ------------------------------------------------------------------
    // snapshot presence per operation
    // ------------------------------------------------------------------

    #[test]
    fn transforming_operations_require_a_snapshot() {
        let descriptor = all_phase_descriptor();
        let operations = [
            (ContextPhase::Admission, ContextOperation::Admit),
            (ContextPhase::Preflight, ContextOperation::Preflight),
            (ContextPhase::Compaction, ContextOperation::Compact),
            (ContextPhase::Recovery, ContextOperation::Restore),
            (ContextPhase::Search, ContextOperation::Search),
        ];
        for (phase, operation) in operations {
            assert!(operation.requires_snapshot(), "{operation:?}");
            let mut req = request(phase, operation);
            req.capability_id = descriptor.id.clone();
            assert_eq!(
                req.validate_against(&descriptor),
                Err(ContextContractError::MissingSnapshot { operation })
            );

            req.snapshot = Some(snapshot());
            assert_eq!(req.validate_against(&descriptor), Ok(()));
        }
    }

    #[test]
    fn meta_operations_do_not_take_a_snapshot() {
        let descriptor = all_phase_descriptor();
        let operations = [
            (ContextPhase::Status, ContextOperation::Describe),
            (ContextPhase::Status, ContextOperation::Status),
            (ContextPhase::Lifecycle, ContextOperation::Shutdown),
        ];
        for (phase, operation) in operations {
            assert!(!operation.requires_snapshot(), "{operation:?}");
            let mut req = request(phase, operation);
            req.capability_id = descriptor.id.clone();
            assert_eq!(req.validate_against(&descriptor), Ok(()));

            req.snapshot = Some(snapshot());
            assert_eq!(
                req.validate_against(&descriptor),
                Err(ContextContractError::UnexpectedSnapshot { operation })
            );
        }
    }

    // ------------------------------------------------------------------
    // scope / permission coherence
    // ------------------------------------------------------------------

    #[test]
    fn descriptor_rejects_incoherent_scope() {
        let mut duplicate = descriptor();
        duplicate.scope.handles = vec!["sess-1".to_string(), "sess-1".to_string()];
        assert_eq!(
            duplicate.validate(),
            Err(ContextContractError::DuplicateScopeHandle {
                handle: "sess-1".to_string(),
            })
        );

        let mut empty = descriptor();
        empty.scope.handles = vec!["  ".to_string()];
        assert_eq!(
            empty.validate(),
            Err(ContextContractError::EmptyScopeHandle)
        );

        let mut undeclared_cross_session = descriptor();
        undeclared_cross_session.scope.namespace = ContextNamespace::CrossSession;
        assert_eq!(
            undeclared_cross_session.validate(),
            Err(ContextContractError::CrossSessionConsistency {
                namespace: ContextNamespace::CrossSession,
                cross_session_read: false,
            })
        );

        let mut permission_without_scope = descriptor();
        permission_without_scope.permissions.cross_session_read = true;
        assert_eq!(
            permission_without_scope.validate(),
            Err(ContextContractError::CrossSessionConsistency {
                namespace: ContextNamespace::Session,
                cross_session_read: true,
            })
        );

        let mut granted = descriptor();
        granted.scope.namespace = ContextNamespace::CrossSession;
        granted.permissions.cross_session_read = true;
        assert_eq!(granted.validate(), Ok(()));
    }

    #[test]
    fn snapshot_scope_cannot_exceed_the_declared_scope() {
        let descriptor = descriptor();

        let mut wider = transforming_request();
        wider.snapshot.as_mut().expect("snapshot").scope.namespace = ContextNamespace::Project;
        assert_eq!(
            wider.validate_against(&descriptor),
            Err(ContextContractError::ScopeEscalation {
                declared: ContextNamespace::Session,
                requested: ContextNamespace::Project,
            })
        );

        let mut foreign_handle = transforming_request();
        foreign_handle
            .snapshot
            .as_mut()
            .expect("snapshot")
            .scope
            .handles = vec!["sess-2".to_string()];
        assert_eq!(
            foreign_handle.validate_against(&descriptor),
            Err(ContextContractError::ScopeHandleNotDeclared {
                handle: "sess-2".to_string(),
            })
        );

        // The invoking session may be named even when the descriptor declares
        // no handles ("invoking session only").
        let mut invoking = transforming_request();
        invoking.snapshot.as_mut().expect("snapshot").scope.handles = vec!["sess-1".to_string()];
        assert_eq!(invoking.validate_against(&descriptor), Ok(()));

        // A declared handle is accepted.
        let mut declared = descriptor.clone();
        declared.scope.handles = vec!["sess-2".to_string()];
        let mut req = transforming_request();
        req.snapshot.as_mut().expect("snapshot").scope.handles = vec!["sess-2".to_string()];
        assert_eq!(req.validate_against(&declared), Ok(()));
    }

    // ------------------------------------------------------------------
    // fingerprint agreement
    // ------------------------------------------------------------------

    #[test]
    fn fingerprints_must_be_present_and_agree() {
        let descriptor = descriptor();

        let mut empty_request = transforming_request();
        empty_request.input_fingerprint = String::new();
        assert_eq!(
            empty_request.validate_against(&descriptor),
            Err(ContextContractError::EmptyFingerprint {
                field: "request.input_fingerprint",
            })
        );

        let mut empty_snapshot = transforming_request();
        empty_snapshot
            .snapshot
            .as_mut()
            .expect("snapshot")
            .input_fingerprint = "  ".to_string();
        assert_eq!(
            empty_snapshot.validate_against(&descriptor),
            Err(ContextContractError::EmptyFingerprint {
                field: "snapshot.input_fingerprint",
            })
        );

        let mut stale_snapshot = transforming_request();
        stale_snapshot
            .snapshot
            .as_mut()
            .expect("snapshot")
            .input_fingerprint = "sha256:cafe".to_string();
        assert_eq!(
            stale_snapshot.validate_against(&descriptor),
            Err(ContextContractError::FingerprintMismatch {
                expected: "sha256:deadbeef".to_string(),
                found: "sha256:cafe".to_string(),
            })
        );
    }

    #[test]
    fn transform_result_must_match_the_request() {
        let request = transforming_request();

        assert_eq!(
            transform_result(1024, 256).validate_against(&request),
            Ok(())
        );

        let mut stale = transform_result(1024, 256);
        stale.input_fingerprint = "sha256:cafe".to_string();
        assert_eq!(
            stale.validate_against(&request),
            Err(ContextContractError::FingerprintMismatch {
                expected: "sha256:deadbeef".to_string(),
                found: "sha256:cafe".to_string(),
            })
        );

        let mut unbounded = transform_result(262_145, 256);
        unbounded.input_fingerprint = request.input_fingerprint.clone();
        assert_eq!(
            unbounded.validate_against(&request),
            Err(ContextContractError::OutputExceedsBudget {
                bytes: 262_145,
                tokens: 256,
            })
        );

        let mut unverifiable = transform_result(1024, 256);
        unverifiable.input_fingerprint = String::new();
        assert_eq!(
            unverifiable.validate_against(&request),
            Err(ContextContractError::EmptyFingerprint {
                field: "result.input_fingerprint",
            })
        );
    }

    // ------------------------------------------------------------------
    // wire defaults and version semantics
    // ------------------------------------------------------------------

    #[test]
    fn omitted_persistence_and_permissions_default_to_denied() {
        let wire = json!({
            "id": "acme/context-observer",
            "api_version": CONTEXT_CAPABILITY_API_VERSION,
            "implementation_version": "0.1.0",
            "phases": ["status"],
            "mode": "observer",
            "scope": {"namespace": "session"},
            "input_budget": {"max_tokens": 1000, "max_bytes": 2000},
            "output_budget": {"max_tokens": 500, "max_bytes": 1000}
        });
        let decoded: ContextCapabilityDescriptor =
            serde_json::from_value(wire).expect("defaults must apply");
        assert_eq!(decoded.persistence, ContextPersistencePolicy::default());
        assert_eq!(decoded.permissions, ContextSideEffectPermissions::default());
        assert!(!decoded.permissions.network_egress);
        assert!(!decoded.permissions.project_write);
        assert!(!decoded.permissions.cross_session_read);
        assert_eq!(decoded.validate(), Ok(()));
    }

    #[test]
    fn wire_versions_are_single_sourced() {
        for operation in ContextOperation::ALL {
            assert_eq!(
                operation.wire_name(),
                format!("{CONTEXT_CAPABILITY_API_VERSION}/{}", operation.as_str())
            );
            assert!(operation.wire_name().starts_with("context.fabric/v1/"));
        }

        let encoded = serde_json::to_value(descriptor()).expect("descriptor json");
        assert_eq!(
            encoded["api_version"],
            Value::String(CONTEXT_CAPABILITY_API_VERSION.to_string())
        );

        let mut wrong_version = transforming_request();
        wrong_version.api_version = "context.fabric/v2".to_string();
        assert_eq!(
            wrong_version.validate_against(&descriptor()),
            Err(ContextContractError::UnsupportedApiVersion {
                found: "context.fabric/v2".to_string(),
            })
        );
    }

    // ------------------------------------------------------------------
    // native trait ABI stays static
    // ------------------------------------------------------------------

    #[test]
    fn preflight_stage_name_remains_static() {
        struct StubStage;

        #[async_trait::async_trait]
        impl PreflightStage for StubStage {
            fn name(&self) -> &'static str {
                "builtin/static-name"
            }

            async fn prepare(
                &self,
                _messages: &mut Vec<UnifiedMessage>,
                _pressure: &ContextPressure,
                _fresh_tail_count: usize,
            ) -> usize {
                0
            }
        }

        fn static_name(stage: &dyn PreflightStage) -> &'static str {
            stage.name()
        }

        assert_eq!(static_name(&StubStage), "builtin/static-name");
    }
}
