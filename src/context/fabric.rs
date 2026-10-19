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
//! ## Admission is a single funnel
//!
//! A call is executed only through [`ContextCapabilityRequest::admit`], which
//! takes the host-authenticated [`AuthenticatedCaller`] and returns an
//! [`AdmittedContextCall`]. That type has private fields and no public
//! constructor, so an executor cannot be handed an unadmitted request, and
//! [`ContextTransformResult`] can only be validated against one
//! ([`ContextTransformResult::validate_for`]). There is no public call
//! validator that skips any of it: the request has no `validate_against`, the
//! snapshot validator is private, and no result API accepts a bare budget or a
//! caller-chosen fingerprint.
//!
//! `admit` re-validates the descriptor it was given
//! ([`ContextCapabilityDescriptor::validate`] runs *inside* the admission path),
//! so a caller that never validated its own declaration cannot smuggle an
//! incoherent one past the contract.
//!
//! ## What this module enforces, and what it does not
//!
//! **Enforced here, fail-closed:** the declaration itself on every admitted
//! call; scope/permission coherence; per-direction budgets (the input ceiling
//! against the snapshot, the output ceiling against the result, resolved and
//! checked independently — never `min` of the two); snapshot presence for
//! transforming operations and absence for meta operations; the invoking session
//! identity supplied by the host; the cross-session grant; and *agreement*
//! between request, snapshot and result fingerprints.
//!
//! **Not enforced here, and therefore a contract requirement:**
//!
//! * [`ContextSnapshot::payload`] is `serde_json::Value`, so this module cannot
//!   prove that hidden reasoning, credentials, unauthorised project files or
//!   other sessions were filtered out before the payload was built. No such
//!   filter exists yet (Core-side, a later task). A snapshot builder must apply
//!   the scope filter itself; a consumer must not read receipt of a snapshot as
//!   evidence that filtering happened.
//! * `input_fingerprint` is a **correlation token, not provenance proof**. Three
//!   equal caller-supplied strings establish that request, snapshot and result
//!   were meant for each other; they say nothing about what those bytes were.
//!   Only the boundary that cut the snapshot can compute a meaningful token —
//!   [`canonical_fingerprint`] / [`ContextSnapshot::payload_fingerprint`] are
//!   the recommended derivation (one canonical encoder, shared with the tool
//!   descriptor fingerprint) — and a token that a payload transported on its own
//!   behalf proves nothing at all. Equality is checked; authorship is not, and
//!   cannot be, checked here.
//! * [`AuthenticatedCaller`] cannot be authenticated here. It refuses to be
//!   built blank and is deliberately not deserializable, so a wire payload can
//!   never forge one, but the host adapter that already authenticated the
//!   session is the only trustworthy caller of
//!   [`AuthenticatedCaller::from_host_boundary`].
//!   [`ContextSnapshot::session_id`] is never used as identity, only compared
//!   against it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::tools::descriptor::canonicalize_json;

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

/// Upper bound on a fingerprint token.
///
/// A token longer than this is a payload copied into a digest field, not a
/// digest, so it is rejected rather than hashed or truncated.
pub const CONTEXT_FINGERPRINT_MAX_LEN: usize = 512;

/// Prefix of every fingerprint this module derives.
pub const CONTEXT_FINGERPRINT_PREFIX: &str = "sha256:";

// ============================================================================
// Fingerprint derivation
// ============================================================================

/// Derive the correlation token for a JSON payload.
///
/// Uses the crate's single canonical JSON encoder ([`canonicalize_json`] — the
/// same one the tool-descriptor fingerprint uses, so there is no second
/// encoder to drift), which makes the digest deterministic and independent of
/// object key order: two peers fingerprinting the same value agree byte for
/// byte, while array order — which is semantic — still changes the digest. The
/// result is `"sha256:<64 hex chars>"`.
///
/// This is a *correlation* token. It shows that two holders of the same value
/// derived the same digest; it does not show who produced the value, and
/// nothing downstream should read it as authentication.
#[must_use]
pub fn canonical_fingerprint(value: &serde_json::Value) -> String {
    let mut bytes = Vec::new();
    canonicalize_json(value, &mut bytes);
    format!(
        "{CONTEXT_FINGERPRINT_PREFIX}{}",
        hex::encode(Sha256::digest(&bytes))
    )
}

/// Reject a fingerprint that cannot be one: blank, or longer than
/// [`CONTEXT_FINGERPRINT_MAX_LEN`] (a payload copied into a digest field is not
/// a digest, so it is refused rather than hashed or truncated).
fn validate_fingerprint(token: &str, field: &'static str) -> Result<(), ContextContractError> {
    if token.trim().is_empty() {
        return Err(ContextContractError::EmptyFingerprint { field });
    }
    if token.len() > CONTEXT_FINGERPRINT_MAX_LEN {
        return Err(ContextContractError::FingerprintTooLong {
            field,
            len: token.len(),
            max: CONTEXT_FINGERPRINT_MAX_LEN,
        });
    }
    Ok(())
}

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
    /// Reject handles that identify nothing, identify the same thing twice, or
    /// are padded into a second spelling of an existing handle.
    ///
    /// A duplicate handle would let a later owner-map lookup resolve two
    /// namespaces to one entry, so it is refused here rather than deduplicated.
    /// Surrounding whitespace is refused for the same reason: `"sess-1"` and
    /// `" sess-1"` are one session and must not be two keys.
    pub fn validate(&self) -> Result<(), ContextContractError> {
        for (index, handle) in self.handles.iter().enumerate() {
            if handle.trim().is_empty() {
                return Err(ContextContractError::EmptyScopeHandle);
            }
            if handle.trim() != handle {
                return Err(ContextContractError::UntrimmedScopeHandle {
                    handle: handle.clone(),
                });
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
    ///
    /// This runs inside [`ContextCapabilityRequest::admit`] as well: admission
    /// never trusts a caller to have validated first.
    pub fn validate(&self) -> Result<(), ContextContractError> {
        if self.id.trim().is_empty() {
            return Err(ContextContractError::EmptyId);
        }
        if self.id.trim() != self.id {
            return Err(ContextContractError::UntrimmedCapabilityId {
                id: self.id.clone(),
            });
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
/// be declared, `token_count`/`byte_count` must fit both the declared input
/// budget and the requesting call's input budget, and `input_fingerprint` must
/// agree with the request. Payload contents are not validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextSnapshot {
    pub snapshot_id: String,
    /// The session this snapshot was cut from.
    ///
    /// **Never an identity claim.** It is a claim about provenance that is
    /// checked against the host-authenticated [`AuthenticatedCaller`], and any
    /// value other than the caller's own session is refused unless a
    /// cross-session read was declared, granted *and* named as a handle.
    pub session_id: String,
    /// Fingerprint of what the snapshot was cut from, so a result can be
    /// matched to the exact input it transformed. A correlation token, not a
    /// proof of provenance; [`Self::payload_fingerprint`] is the recommended
    /// derivation.
    pub input_fingerprint: String,
    pub scope: ContextScope,
    pub token_count: u64,
    pub byte_count: u64,
    pub payload: serde_json::Value,
}

impl ContextSnapshot {
    /// The correlation token for this snapshot's own payload, derived through
    /// the crate's single canonical JSON encoder.
    ///
    /// A builder that cut this payload should publish this value as
    /// `input_fingerprint`: it is deterministic and key-order independent, so
    /// two peers fingerprinting the same bytes agree. It proves nothing about
    /// who produced the payload — see [`canonical_fingerprint`].
    #[must_use]
    pub fn payload_fingerprint(&self) -> String {
        canonical_fingerprint(&self.payload)
    }

    /// Validate the snapshot envelope during admission.
    ///
    /// Private on purpose. A public snapshot validator would be an admission
    /// path that never consulted the host-authenticated caller, which is
    /// exactly the hole that let a payload name its own session and be trusted
    /// for it. Admission is the only way in.
    fn validate_admitted(
        &self,
        descriptor: &ContextCapabilityDescriptor,
        request_fingerprint: &str,
        request_input_budget: ContextBudget,
        caller: &AuthenticatedCaller,
    ) -> Result<(), ContextContractError> {
        validate_fingerprint(&self.input_fingerprint, "snapshot.input_fingerprint")?;
        if self.input_fingerprint != request_fingerprint {
            return Err(ContextContractError::FingerprintMismatch {
                expected: request_fingerprint.to_string(),
                found: self.input_fingerprint.clone(),
            });
        }
        if self.session_id.trim().is_empty() {
            return Err(ContextContractError::EmptySessionId {
                field: "snapshot.session_id",
            });
        }
        if self.session_id.trim() != self.session_id {
            return Err(ContextContractError::UntrimmedSessionId {
                session_id: self.session_id.clone(),
            });
        }
        // Identity comes from the host boundary, never from the payload. A
        // snapshot cut from another session is a cross-session read, and all
        // three grants have to be present at once: the namespace declaration,
        // the read permission, and a declared handle naming that exact session.
        // Missing any one is a denial, never a widening.
        if self.session_id != caller.session_id() {
            let granted = descriptor.scope.namespace == ContextNamespace::CrossSession
                && descriptor.permissions.cross_session_read
                && descriptor
                    .scope
                    .handles
                    .iter()
                    .any(|handle| handle == &self.session_id);
            if !granted {
                return Err(ContextContractError::CrossSessionReadDenied {
                    caller_session: caller.session_id().to_string(),
                    snapshot_session: self.session_id.clone(),
                });
            }
        }
        // Both ceilings apply to the input side, and neither is derived from
        // the other direction's number. The descriptor ceiling is checked first
        // so the error names the declaration that was overrun; the per-call
        // ceiling then enforces what this particular call actually granted.
        for (side, ceiling) in [
            ("descriptor", descriptor.input_budget),
            ("request", request_input_budget),
        ] {
            if self.token_count > ceiling.max_tokens || self.byte_count > ceiling.max_bytes {
                return Err(ContextContractError::SnapshotExceedsInputBudget {
                    side,
                    tokens: self.token_count,
                    bytes: self.byte_count,
                    max_tokens: ceiling.max_tokens,
                    max_bytes: ceiling.max_bytes,
                });
            }
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
            // Empty declared handles mean "the invoking session only", and the
            // invoking session is the authenticated caller. `self.session_id`
            // is a field the payload chose, so it must not decide this.
            let invoking_session = descriptor.scope.handles.is_empty()
                && descriptor.scope.namespace == ContextNamespace::Session
                && handle == caller.session_id();
            if !invoking_session {
                return Err(ContextContractError::ScopeHandleNotDeclared {
                    handle: handle.clone(),
                });
            }
        }
        Ok(())
    }
}

/// The session identity of the caller, as established by the host boundary.
///
/// This module cannot authenticate anyone, so this type does not claim to: it
/// carries an identity the host adapter already authenticated, and the only
/// thing enforced here is that the id is usable (non-blank, untrimmed).
///
/// **Deliberately not serializable or deserializable.** A wire payload that could
/// build one could name any session it liked, which is exactly the defect this
/// type exists to close: [`ContextSnapshot::session_id`] is never trusted as
/// identity, only compared against this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedCaller {
    session_id: String,
}

impl AuthenticatedCaller {
    /// Build a caller from the identity the host already authenticated.
    ///
    /// The host adapter that resolved the session is the only trustworthy
    /// caller of this function. Nothing here can verify the claim; an adapter
    /// that invents one has authenticated nothing.
    pub fn from_host_boundary(session_id: impl Into<String>) -> Result<Self, ContextContractError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(ContextContractError::EmptySessionId {
                field: "caller.session_id",
            });
        }
        if session_id.trim() != session_id {
            return Err(ContextContractError::UntrimmedSessionId { session_id });
        }
        Ok(Self { session_id })
    }

    /// The authenticated session id.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// One wire call into a capability.
///
/// Construction is open (this is a wire DTO), but *verification* is not: the
/// only way to turn one into something executable is [`Self::admit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextCapabilityRequest {
    pub request_id: String,
    pub capability_id: String,
    pub api_version: String,
    pub phase: ContextPhase,
    pub operation: ContextOperation,
    /// Legacy per-direction ceiling from the first version of this wire
    /// contract, kept wire-compatible: when present it applies to whichever
    /// direction the split fields below do not specify.
    ///
    /// A call may send this field *or* [`Self::input_budget`] /
    /// [`Self::output_budget`], never both — a call that sends both is
    /// ambiguous and refused ([`ContextContractError::AmbiguousBudget`]).
    ///
    /// Absent for `describe`/`status`/`shutdown`, which transform nothing and so
    /// have no budget to grant; carrying one there is refused rather than
    /// recorded as a meaningless limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<ContextBudget>,
    /// Per-call input ceiling, checked against the snapshot's own size.
    ///
    /// Independent of [`Self::output_budget`]: a call allowed to read a lot is
    /// not thereby allowed to write a lot, and neither ceiling is ever the `min`
    /// of the two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_budget: Option<ContextBudget>,
    /// Per-call output ceiling, checked against the result's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_budget: Option<ContextBudget>,
    /// Correlation token tying request, snapshot and result together. Required
    /// for transforming operations, forbidden for meta operations (which have no
    /// input to fingerprint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ContextSnapshot>,
}

impl ContextCapabilityRequest {
    /// The one admission funnel: decide whether this call may execute, and
    /// return the proof that it may.
    ///
    /// Fail-closed, in this order:
    ///
    /// 1. the descriptor is re-validated here
    ///    ([`ContextCapabilityDescriptor::validate`]), so a caller that never
    ///    validated its own declaration cannot smuggle an incoherent one past
    ///    the contract;
    /// 2. the request targets this capability on this wire version, its
    ///    `request_id` is a usable key, the phase is declared, and the phase is
    ///    the one the operation belongs to;
    /// 3. meta operations (`describe`/`status`/`shutdown`) carry no snapshot, no
    ///    budget and no fingerprint — inventing one is refused rather than
    ///    stored as a meaningless limit;
    /// 4. transforming operations carry a fingerprint and exactly one budget
    ///    form; the two directions are resolved and each held to its *matching*
    ///    declared ceiling independently;
    /// 5. the snapshot exists and passes the private snapshot admission:
    ///    fingerprint agreement, host-authenticated caller identity, the
    ///    cross-session grant, both input ceilings, and the declared scope.
    pub fn admit<'a>(
        &'a self,
        descriptor: &'a ContextCapabilityDescriptor,
        caller: &'a AuthenticatedCaller,
    ) -> Result<AdmittedContextCall<'a>, ContextContractError> {
        descriptor.validate()?;
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
        // The request id is the idempotency/dedup key downstream will use; a
        // blank or padded one is not a key, so it is refused here rather than
        // silently normalized.
        if self.request_id.trim().is_empty() || self.request_id.trim() != self.request_id {
            return Err(ContextContractError::InvalidRequestId {
                request_id: self.request_id.clone(),
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
        if !self.operation.requires_snapshot() {
            if self.snapshot.is_some() {
                return Err(ContextContractError::UnexpectedSnapshot {
                    operation: self.operation,
                });
            }
            if self.budget.is_some() || self.input_budget.is_some() || self.output_budget.is_some()
            {
                return Err(ContextContractError::UnexpectedBudget {
                    operation: self.operation,
                });
            }
            if self.input_fingerprint.is_some() {
                return Err(ContextContractError::UnexpectedFingerprint {
                    operation: self.operation,
                });
            }
            return Ok(AdmittedContextCall {
                descriptor,
                operation: self.operation,
                input_fingerprint: None,
                input_budget: None,
                output_budget: None,
                caller,
                snapshot: None,
            });
        }

        let input_fingerprint = match self.input_fingerprint.as_deref() {
            Some(token) => {
                validate_fingerprint(token, "request.input_fingerprint")?;
                token
            }
            None => {
                return Err(ContextContractError::MissingFingerprint {
                    field: "request.input_fingerprint",
                })
            }
        };
        let (input_budget, output_budget) = self.requested_budgets()?;
        // Each direction is held to the matching declared ceiling. There is no
        // cross-direction `min` here: a wide output grant is not thereby an input
        // ceiling, and a narrow input grant is not thereby an output one.
        input_budget.require_within(&descriptor.input_budget, "input")?;
        output_budget.require_within(&descriptor.output_budget, "output")?;

        let snapshot = match self.snapshot.as_ref() {
            Some(snapshot) => snapshot,
            None => {
                return Err(ContextContractError::MissingSnapshot {
                    operation: self.operation,
                })
            }
        };
        snapshot.validate_admitted(descriptor, input_fingerprint, input_budget, caller)?;

        Ok(AdmittedContextCall {
            descriptor,
            operation: self.operation,
            input_fingerprint: Some(input_fingerprint),
            input_budget: Some(input_budget),
            output_budget: Some(output_budget),
            caller,
            snapshot: Some(snapshot),
        })
    }

    /// Resolve the two per-direction ceilings from the one-or-two budget forms
    /// the wire allows.
    ///
    /// A call may send the legacy single [`Self::budget`], which then applies to
    /// both directions, or the split [`Self::input_budget`] /
    /// [`Self::output_budget`]. Sending the legacy field *and* either split field
    /// is ambiguous — "24k here, 100k there" has no reading that is not a guess —
    /// so it is refused rather than resolved by precedence.
    fn requested_budgets(&self) -> Result<(ContextBudget, ContextBudget), ContextContractError> {
        let split = self.input_budget.is_some() || self.output_budget.is_some();
        if split && self.budget.is_some() {
            return Err(ContextContractError::AmbiguousBudget);
        }
        let input = self
            .input_budget
            .or(self.budget)
            .ok_or(ContextContractError::MissingBudget { side: "input" })?;
        let output = self
            .output_budget
            .or(self.budget)
            .ok_or(ContextContractError::MissingBudget { side: "output" })?;
        input.validate()?;
        output.validate()?;
        Ok((input, output))
    }
}

/// A call that passed [`ContextCapabilityRequest::admit`].
///
/// Private fields and no public constructor: an executor can only be handed a
/// call admission already checked, and [`ContextTransformResult::validate_for`]
/// accepts nothing else — so a result cannot be self-certified from a
/// caller-chosen fingerprint or a caller-chosen budget.
///
/// Borrows the request, descriptor and snapshot rather than cloning them: a
/// snapshot payload can be megabytes, and admission sits on the hot path of every
/// transforming call.
#[derive(Debug)]
pub struct AdmittedContextCall<'a> {
    descriptor: &'a ContextCapabilityDescriptor,
    operation: ContextOperation,
    input_fingerprint: Option<&'a str>,
    input_budget: Option<ContextBudget>,
    output_budget: Option<ContextBudget>,
    caller: &'a AuthenticatedCaller,
    snapshot: Option<&'a ContextSnapshot>,
}

impl<'a> AdmittedContextCall<'a> {
    /// The capability this call was admitted against, already re-validated.
    #[must_use]
    pub fn descriptor(&self) -> &'a ContextCapabilityDescriptor {
        self.descriptor
    }

    /// The admitted operation.
    #[must_use]
    pub fn operation(&self) -> ContextOperation {
        self.operation
    }

    /// The authenticated session that made the call.
    #[must_use]
    pub fn caller(&self) -> &'a AuthenticatedCaller {
        self.caller
    }

    /// The agreed input fingerprint, or `None` for a meta operation.
    #[must_use]
    pub fn input_fingerprint(&self) -> Option<&'a str> {
        self.input_fingerprint
    }

    /// The input ceiling in force for this call (the descriptor ceiling was
    /// already proven to contain it), or `None` for a meta operation.
    #[must_use]
    pub fn input_budget(&self) -> Option<ContextBudget> {
        self.input_budget
    }

    /// The output ceiling in force for this call (the descriptor ceiling was
    /// already proven to contain it), or `None` for a meta operation.
    #[must_use]
    pub fn output_budget(&self) -> Option<ContextBudget> {
        self.output_budget
    }

    /// The admitted snapshot, or `None` for a meta operation.
    #[must_use]
    pub fn snapshot(&self) -> Option<&'a ContextSnapshot> {
        self.snapshot
    }

    /// Whether this call transforms context and therefore expects a result.
    #[must_use]
    pub fn expects_result(&self) -> bool {
        self.operation.requires_snapshot()
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
    /// Enforce the output ceiling of the call this result answers.
    ///
    /// Takes an [`AdmittedContextCall`] rather than a bare budget or request:
    /// that is the only shape which proves the fingerprint this result must carry
    /// and the ceiling it must fit were both checked at admission, so a result
    /// cannot self-certify by supplying its own numbers. The admitted output
    /// ceiling is already contained in the descriptor's declared output budget
    /// (admission checked that), so holding the result to it covers both.
    ///
    /// Fail-closed: a meta operation has no result
    /// ([`ContextContractError::ResultForMetaOperation`]), a blank or oversized
    /// fingerprint is refused, a mismatched fingerprint is refused (a stale
    /// result must not be read as describing the current input), and an
    /// over-budget result is rejected as a contract violation — never truncated
    /// into the prompt silently.
    pub fn validate_for(&self, call: &AdmittedContextCall<'_>) -> Result<(), ContextContractError> {
        let operation = call.operation();
        if !operation.requires_snapshot() {
            return Err(ContextContractError::ResultForMetaOperation { operation });
        }
        let (output_budget, expected_fingerprint) =
            match (call.output_budget(), call.input_fingerprint()) {
                (Some(budget), Some(token)) => (budget, token),
                _ => return Err(ContextContractError::ResultForMetaOperation { operation }),
            };
        validate_fingerprint(&self.input_fingerprint, "result.input_fingerprint")?;
        if self.input_fingerprint != expected_fingerprint {
            return Err(ContextContractError::FingerprintMismatch {
                expected: expected_fingerprint.to_string(),
                found: self.input_fingerprint.clone(),
            });
        }
        if self.output_bytes > output_budget.max_bytes
            || self.output_tokens > output_budget.max_tokens
        {
            return Err(ContextContractError::OutputExceedsBudget {
                side: "request",
                bytes: self.output_bytes,
                tokens: self.output_tokens,
            });
        }
        Ok(())
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
    #[error("context capability id must not be padded: {id:?}")]
    UntrimmedCapabilityId { id: String },
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
    #[error("request grants no {side} budget for an operation that transforms context")]
    MissingBudget { side: &'static str },
    #[error(
        "request is ambiguous: send either the legacy `budget` or the split `input_budget`/`output_budget`, not both"
    )]
    AmbiguousBudget,
    #[error("context budget exceeds the hard cap: {tokens} tokens / {bytes} bytes")]
    BudgetExceedsCap { tokens: u64, bytes: u64 },
    #[error("persisted context state must be rebuildable from session events")]
    NonRebuildablePersistence,
    #[error("bundle mode requires a bundle id")]
    MissingBundleId,
    #[error(
        "context result exceeds its granted {side} output budget: {bytes} bytes / {tokens} tokens"
    )]
    OutputExceedsBudget {
        side: &'static str,
        bytes: u64,
        tokens: u64,
    },
    #[error("scope handle must not be empty")]
    EmptyScopeHandle,
    #[error("scope handle must not be padded: {handle:?}")]
    UntrimmedScopeHandle { handle: String },
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
        "snapshot exceeds the {side} input budget: {tokens} tokens / {bytes} bytes (max {max_tokens} / {max_bytes})"
    )]
    SnapshotExceedsInputBudget {
        side: &'static str,
        tokens: u64,
        bytes: u64,
        max_tokens: u64,
        max_bytes: u64,
    },
    #[error("operation {operation:?} requires a context snapshot")]
    MissingSnapshot { operation: ContextOperation },
    #[error("operation {operation:?} does not take a context snapshot")]
    UnexpectedSnapshot { operation: ContextOperation },
    #[error("operation {operation:?} does not take a budget: it transforms no context")]
    UnexpectedBudget { operation: ContextOperation },
    #[error("operation {operation:?} does not take an input fingerprint: it has no input")]
    UnexpectedFingerprint { operation: ContextOperation },
    #[error("{field} must not be empty; an empty fingerprint cannot be verified")]
    EmptyFingerprint { field: &'static str },
    #[error("{field} is required for an operation that transforms context")]
    MissingFingerprint { field: &'static str },
    #[error("{field} is {len} chars, over the {max}-char fingerprint limit")]
    FingerprintTooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    #[error("input fingerprint mismatch: expected {expected}, found {found}")]
    FingerprintMismatch { expected: String, found: String },
    #[error("snapshot scope {requested:?} exceeds the declared scope {declared:?}")]
    ScopeEscalation {
        declared: ContextNamespace,
        requested: ContextNamespace,
    },
    #[error("snapshot names scope handle {handle}, which the capability did not declare")]
    ScopeHandleNotDeclared { handle: String },
    #[error("operation {operation:?} produces no result: it transforms no context")]
    ResultForMetaOperation { operation: ContextOperation },
    #[error("{field} must not be empty")]
    EmptySessionId { field: &'static str },
    #[error("session id must not be padded: {session_id:?}")]
    UntrimmedSessionId { session_id: String },
    #[error(
        "session {caller_session} may not read a snapshot cut from session {snapshot_session}: needs a cross-session namespace, `cross_session_read`, and a declared handle naming it"
    )]
    CrossSessionReadDenied {
        caller_session: String,
        snapshot_session: String,
    },
    #[error("request id must be a non-empty, unpadded key: {request_id:?}")]
    InvalidRequestId { request_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::budget::preflight::PreflightStage;
    use crate::context::budget::ContextPressure;
    use crate::providers::message::UnifiedMessage;
    use serde_json::{json, Value};

    const CAPABILITY_ID: &str = "acme/context-compactor";
    const ALL_PHASES_ID: &str = "acme/all-phases";
    const CALLER_SESSION: &str = "sess-1";
    const OTHER_SESSION: &str = "sess-2";

    // ------------------------------------------------------------------
    // Fixtures
    // ------------------------------------------------------------------

    /// The identity the host adapter already resolved. Tests never build one
    /// from a wire payload — that is the point of the type.
    fn caller() -> AuthenticatedCaller {
        AuthenticatedCaller::from_host_boundary(CALLER_SESSION).expect("host-authenticated caller")
    }

    fn caller_for(session_id: &str) -> AuthenticatedCaller {
        AuthenticatedCaller::from_host_boundary(session_id).expect("host-authenticated caller")
    }

    fn descriptor() -> ContextCapabilityDescriptor {
        ContextCapabilityDescriptor {
            id: CAPABILITY_ID.to_string(),
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

    fn all_phase_descriptor() -> ContextCapabilityDescriptor {
        let mut descriptor = descriptor();
        descriptor.id = ALL_PHASES_ID.to_string();
        descriptor.phases = ContextPhase::ALL.to_vec();
        descriptor.mode = ContextCapabilityMode::Compose;
        descriptor
    }

    /// A descriptor that holds all three cross-session grants for the handles it
    /// is given. `validate` requires the namespace and the permission to agree,
    /// so a granted descriptor cannot be built in pieces.
    fn cross_session_descriptor(handles: &[&str]) -> ContextCapabilityDescriptor {
        let mut descriptor = descriptor();
        descriptor.scope.namespace = ContextNamespace::CrossSession;
        descriptor.permissions.cross_session_read = true;
        descriptor.scope.handles = handles.iter().map(|handle| (*handle).to_string()).collect();
        descriptor
    }

    fn snapshot_payload() -> Value {
        json!({"messages": [{"role": "user", "content": "hi"}]})
    }

    fn snapshot_from(session_id: &str) -> ContextSnapshot {
        let payload = snapshot_payload();
        ContextSnapshot {
            snapshot_id: "snap-1".to_string(),
            session_id: session_id.to_string(),
            input_fingerprint: canonical_fingerprint(&payload),
            scope: ContextScope {
                namespace: ContextNamespace::Session,
                handles: Vec::new(),
            },
            token_count: 900,
            byte_count: 40_000,
            payload,
        }
    }

    fn snapshot() -> ContextSnapshot {
        snapshot_from(CALLER_SESSION)
    }

    fn input_fingerprint() -> String {
        snapshot().input_fingerprint
    }

    /// A transforming call in the legacy single-budget form, which applies to
    /// both directions.
    fn legacy_request() -> ContextCapabilityRequest {
        ContextCapabilityRequest {
            request_id: "req-1".to_string(),
            capability_id: CAPABILITY_ID.to_string(),
            api_version: CONTEXT_CAPABILITY_API_VERSION.to_string(),
            phase: ContextPhase::Compaction,
            operation: ContextOperation::Compact,
            budget: Some(ContextBudget::new(24_000, 262_144)),
            input_budget: None,
            output_budget: None,
            input_fingerprint: Some(input_fingerprint()),
            snapshot: Some(snapshot()),
        }
    }

    /// The same call in the split-budget form, each direction at its own
    /// declared ceiling.
    fn split_request() -> ContextCapabilityRequest {
        ContextCapabilityRequest {
            budget: None,
            input_budget: Some(ContextBudget::new(120_000, 1_048_576)),
            output_budget: Some(ContextBudget::new(24_000, 262_144)),
            ..legacy_request()
        }
    }

    /// A meta operation: no snapshot, no budget, no fingerprint.
    fn meta_request(phase: ContextPhase, operation: ContextOperation) -> ContextCapabilityRequest {
        ContextCapabilityRequest {
            request_id: "req-meta".to_string(),
            capability_id: ALL_PHASES_ID.to_string(),
            api_version: CONTEXT_CAPABILITY_API_VERSION.to_string(),
            phase,
            operation,
            budget: None,
            input_budget: None,
            output_budget: None,
            input_fingerprint: None,
            snapshot: None,
        }
    }

    fn transform_result(bytes: u64, tokens: u64) -> ContextTransformResult {
        ContextTransformResult {
            input_fingerprint: input_fingerprint(),
            output: json!({"messages": []}),
            output_bytes: bytes,
            output_tokens: tokens,
            strategy: "acme.dedup.v1".to_string(),
            continuation: None,
            quality: ContextQuality::Condensed,
            degraded: None,
        }
    }

    /// The meta operations, with the phase each one belongs to.
    const META_OPERATIONS: [(ContextPhase, ContextOperation); 3] = [
        (ContextPhase::Status, ContextOperation::Describe),
        (ContextPhase::Status, ContextOperation::Status),
        (ContextPhase::Lifecycle, ContextOperation::Shutdown),
    ];

    /// The transforming operations, with the phase each one belongs to.
    const TRANSFORMING_OPERATIONS: [(ContextPhase, ContextOperation); 5] = [
        (ContextPhase::Admission, ContextOperation::Admit),
        (ContextPhase::Preflight, ContextOperation::Preflight),
        (ContextPhase::Compaction, ContextOperation::Compact),
        (ContextPhase::Recovery, ContextOperation::Restore),
        (ContextPhase::Search, ContextOperation::Search),
    ];

    /// Find the schema object that declares `property`, wherever the generator
    /// put it (root or `$defs`).
    fn schema_object_declaring<'a>(schema: &'a Value, property: &str) -> Option<&'a Value> {
        match schema {
            Value::Object(map) => {
                if map
                    .get("properties")
                    .and_then(|properties| properties.get(property))
                    .is_some()
                {
                    return Some(schema);
                }
                map.values()
                    .find_map(|value| schema_object_declaring(value, property))
            }
            Value::Array(items) => items
                .iter()
                .find_map(|value| schema_object_declaring(value, property)),
            _ => None,
        }
    }

    fn required_properties(schema: &Value, property: &str) -> Vec<String> {
        schema_object_declaring(schema, property)
            .and_then(|object| object.get("required"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
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
    fn legacy_request_and_result_round_trip_through_json() {
        let request = legacy_request();
        let encoded = serde_json::to_string(&request).expect("serialize");
        let decoded: ContextCapabilityRequest =
            serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, request);

        // The legacy form keeps the single `budget` field and omits the split
        // ones, so an old peer's payload still decodes to the same call.
        let wire: Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(wire["budget"]["max_tokens"], json!(24_000));
        assert!(wire.get("input_budget").is_none());
        assert!(wire.get("output_budget").is_none());

        let caller = caller();
        let descriptor = descriptor();
        let admitted = decoded.admit(&descriptor, &caller).expect("admit");
        assert_eq!(
            admitted.input_budget(),
            Some(ContextBudget::new(24_000, 262_144))
        );
        assert_eq!(
            admitted.output_budget(),
            Some(ContextBudget::new(24_000, 262_144))
        );

        let result = transform_result(1024, 256);
        let encoded = serde_json::to_string(&result).expect("serialize");
        let decoded: ContextTransformResult = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, result);
        assert_eq!(decoded.validate_for(&admitted), Ok(()));
    }

    #[test]
    fn split_budget_request_round_trips_through_json() {
        let request = split_request();
        let encoded = serde_json::to_string(&request).expect("serialize");
        let decoded: ContextCapabilityRequest =
            serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, request);

        let wire: Value = serde_json::from_str(&encoded).expect("json");
        assert!(wire.get("budget").is_none());
        assert_eq!(wire["input_budget"]["max_tokens"], json!(120_000));
        assert_eq!(wire["output_budget"]["max_bytes"], json!(262_144));
        assert_eq!(
            wire["snapshot"]["input_fingerprint"],
            json!(input_fingerprint())
        );

        let caller = caller();
        let descriptor = descriptor();
        let admitted = decoded.admit(&descriptor, &caller).expect("admit");
        assert_eq!(
            admitted.input_budget(),
            Some(ContextBudget::new(120_000, 1_048_576))
        );
        assert_eq!(
            admitted.output_budget(),
            Some(ContextBudget::new(24_000, 262_144))
        );
    }

    #[test]
    fn meta_request_round_trips_without_optional_payload() {
        let request = meta_request(ContextPhase::Status, ContextOperation::Status);
        let encoded = serde_json::to_string(&request).expect("serialize");
        let decoded: ContextCapabilityRequest =
            serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, request);

        for absent in [
            "budget",
            "input_budget",
            "output_budget",
            "input_fingerprint",
            "snapshot",
        ] {
            assert!(
                !encoded.contains(&format!("\"{absent}\"")),
                "a meta request must omit `{absent}`: {encoded}"
            );
        }

        let caller = caller();
        let all_phases = all_phase_descriptor();
        let admitted = decoded.admit(&all_phases, &caller).expect("meta admit");
        assert_eq!(admitted.operation(), ContextOperation::Status);
        assert!(!admitted.expects_result());
        assert!(admitted.snapshot().is_none());
        assert!(admitted.input_budget().is_none());
        assert!(admitted.output_budget().is_none());
        assert!(admitted.input_fingerprint().is_none());
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
            "input_budget",
            "output_budget",
            "input_fingerprint",
            "snapshot",
        ] {
            assert!(
                text.contains(&format!("\"{property}\"")),
                "missing {property}"
            );
        }

        // The budget forms, the fingerprint and the snapshot are optional on the
        // wire; a peer must not be told they are mandatory.
        let required = required_properties(&request_schema, "request_id");
        for required_property in [
            "request_id",
            "capability_id",
            "api_version",
            "phase",
            "operation",
        ] {
            assert!(
                required.contains(&required_property.to_string()),
                "`{required_property}` must be required: {required:?}"
            );
        }
        for optional in [
            "budget",
            "input_budget",
            "output_budget",
            "input_fingerprint",
            "snapshot",
        ] {
            assert!(
                !required.contains(&optional.to_string()),
                "`{optional}` must stay optional: {required:?}"
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
            "id": CAPABILITY_ID,
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

        // Both budget forms stay optional on the wire: a request that omits them
        // decodes, and admission (not serde) is what refuses it.
        let request_wire = json!({
            "request_id": "req-1",
            "capability_id": CAPABILITY_ID,
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
        assert_eq!(request.input_budget, None);
        assert_eq!(request.output_budget, None);

        let bare: ContextCapabilityRequest = serde_json::from_value(json!({
            "request_id": "req-2",
            "capability_id": CAPABILITY_ID,
            "api_version": CONTEXT_CAPABILITY_API_VERSION,
            "phase": "compaction",
            "operation": "compact"
        }))
        .expect("optional fields default to absent");
        assert_eq!(bare.budget, None);
        assert_eq!(bare.input_budget, None);
        assert_eq!(bare.output_budget, None);
        assert_eq!(bare.input_fingerprint, None);
        assert_eq!(bare.snapshot, None);
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

    // ------------------------------------------------------------------
    // admission re-validates the descriptor
    // ------------------------------------------------------------------

    #[test]
    fn admission_revalidates_the_descriptor() {
        let caller = caller();
        let request = legacy_request();

        let mut no_phase = descriptor();
        no_phase.phases.clear();

        let mut empty_budget = descriptor();
        empty_budget.output_budget = ContextBudget::new(24_000, 0);

        let mut contradictory_scope = descriptor();
        contradictory_scope.scope.namespace = ContextNamespace::CrossSession;

        // The request names `CAPABILITY_ID`; the padded descriptor id would also
        // trip the capability-match check, so this case proves validation runs
        // *before* that check rather than being skipped when the ids agree.
        let mut padded_id = descriptor();
        padded_id.id = format!(" {CAPABILITY_ID}");

        let mut bundle_without_id = descriptor();
        bundle_without_id.mode = ContextCapabilityMode::Bundle;

        let cases = [
            (no_phase, ContextContractError::NoPhase),
            (empty_budget, ContextContractError::EmptyBudget),
            (
                contradictory_scope,
                ContextContractError::CrossSessionConsistency {
                    namespace: ContextNamespace::CrossSession,
                    cross_session_read: false,
                },
            ),
            (
                padded_id,
                ContextContractError::UntrimmedCapabilityId {
                    id: format!(" {CAPABILITY_ID}"),
                },
            ),
            (bundle_without_id, ContextContractError::MissingBundleId),
        ];

        for (descriptor, expected) in cases {
            assert_eq!(
                request.admit(&descriptor, &caller).err(),
                Some(expected),
                "admission must re-validate the descriptor it was handed"
            );
        }
    }

    // ------------------------------------------------------------------
    // caller identity, declared handle, permission
    // ------------------------------------------------------------------

    #[test]
    fn cross_session_read_requires_all_three_grants() {
        let caller = caller_for(CALLER_SESSION);
        let mut request = legacy_request();
        request.snapshot = Some(snapshot_from(OTHER_SESSION));

        let denied = ContextContractError::CrossSessionReadDenied {
            caller_session: CALLER_SESSION.to_string(),
            snapshot_session: OTHER_SESSION.to_string(),
        };

        // 1. Session namespace, no read permission, no handle.
        assert_eq!(
            request.admit(&descriptor(), &caller).err(),
            Some(denied.clone())
        );

        // 2. Cross-session namespace and permission, but no handle names the
        //    snapshot's session.
        assert_eq!(
            request
                .admit(&cross_session_descriptor(&["sess-3"]), &caller)
                .err(),
            Some(denied.clone())
        );

        // 3. All three grants present at once: admitted.
        assert!(request
            .admit(&cross_session_descriptor(&[OTHER_SESSION]), &caller)
            .is_ok());

        // 4. The permission without the namespace is not a grant either — the
        //    descriptor contradicts itself, so admission refuses the descriptor
        //    before the snapshot is even examined.
        let mut permission_only = descriptor();
        permission_only.permissions.cross_session_read = true;
        assert_eq!(
            request.admit(&permission_only, &caller).err(),
            Some(ContextContractError::CrossSessionConsistency {
                namespace: ContextNamespace::Session,
                cross_session_read: true,
            })
        );

        // 5. A snapshot cut from the caller's own session needs no grant.
        let mut own = legacy_request();
        own.snapshot = Some(snapshot_from(CALLER_SESSION));
        assert!(own.admit(&descriptor(), &caller).is_ok());
    }

    #[test]
    fn scope_handles_are_checked_against_the_caller_not_the_snapshot() {
        let caller = caller_for(CALLER_SESSION);

        // The caller's own session is the one handle an undeclared scope grants.
        let mut own = legacy_request();
        own.snapshot.as_mut().expect("snapshot").scope.handles = vec![CALLER_SESSION.to_string()];
        assert!(own.admit(&descriptor(), &caller).is_ok());

        // A snapshot that names *only itself* is still not the caller: identity
        // comes from the host boundary, not from a payload field.
        let mut self_named = legacy_request();
        self_named
            .snapshot
            .as_mut()
            .expect("snapshot")
            .scope
            .handles = vec![OTHER_SESSION.to_string()];
        assert_eq!(
            self_named.admit(&descriptor(), &caller).err(),
            Some(ContextContractError::ScopeHandleNotDeclared {
                handle: OTHER_SESSION.to_string(),
            })
        );

        // With a cross-session grant the snapshot's session is accepted, but the
        // scope still may not name a handle the descriptor never declared.
        let granted = cross_session_descriptor(&[OTHER_SESSION]);
        let mut request = legacy_request();
        let mut foreign = snapshot_from(OTHER_SESSION);
        foreign.scope.handles = vec![CALLER_SESSION.to_string()];
        request.snapshot = Some(foreign);
        assert_eq!(
            request.admit(&granted, &caller).err(),
            Some(ContextContractError::ScopeHandleNotDeclared {
                handle: CALLER_SESSION.to_string(),
            })
        );

        // A declared handle is accepted, and the snapshot's scope stays inside
        // the declared namespace.
        let mut accepted = legacy_request();
        let mut declared_snapshot = snapshot_from(OTHER_SESSION);
        declared_snapshot.scope.handles = vec![OTHER_SESSION.to_string()];
        accepted.snapshot = Some(declared_snapshot);
        assert!(accepted.admit(&granted, &caller).is_ok());
    }

    #[test]
    fn snapshot_scope_cannot_exceed_the_declared_scope() {
        let caller = caller();
        let descriptor = descriptor();

        let mut wider = legacy_request();
        wider.snapshot.as_mut().expect("snapshot").scope.namespace = ContextNamespace::Project;
        assert_eq!(
            wider.admit(&descriptor, &caller).err(),
            Some(ContextContractError::ScopeEscalation {
                declared: ContextNamespace::Session,
                requested: ContextNamespace::Project,
            })
        );
    }

    // ------------------------------------------------------------------
    // budgets: legacy vs split, each direction independently
    // ------------------------------------------------------------------

    #[test]
    fn legacy_budget_applies_to_both_directions() {
        let caller = caller();
        let descriptor = descriptor();
        let request = legacy_request();
        let admitted = request.admit(&descriptor, &caller).expect("admit");

        assert_eq!(
            admitted.input_budget(),
            Some(ContextBudget::new(24_000, 262_144))
        );
        assert_eq!(
            admitted.output_budget(),
            Some(ContextBudget::new(24_000, 262_144))
        );
        assert_eq!(admitted.operation(), ContextOperation::Compact);
        assert!(admitted.expects_result());
        assert_eq!(admitted.caller().session_id(), CALLER_SESSION);
        assert_eq!(admitted.snapshot(), request.snapshot.as_ref());
        assert_eq!(
            admitted.input_fingerprint(),
            request.input_fingerprint.as_deref()
        );
        assert_eq!(admitted.descriptor().id, CAPABILITY_ID);
    }

    #[test]
    fn split_budgets_are_resolved_per_direction_without_a_cross_direction_min() {
        let caller = caller();
        let descriptor = descriptor();

        let request = split_request();
        let admitted = request.admit(&descriptor, &caller).expect("admit");
        assert_eq!(
            admitted.input_budget(),
            Some(ContextBudget::new(120_000, 1_048_576))
        );
        assert_eq!(
            admitted.output_budget(),
            Some(ContextBudget::new(24_000, 262_144))
        );

        // The legacy form cannot express "wide in, narrow out": the one number
        // is held to both ceilings, so the wide reading fails on the output side
        // instead of being clamped to the smaller direction.
        let mut wide_legacy = legacy_request();
        wide_legacy.budget = Some(ContextBudget::new(120_000, 1_048_576));
        assert_eq!(
            wide_legacy.admit(&descriptor, &caller).err(),
            Some(ContextContractError::RequestBudgetExceedsDescriptor {
                side: "output",
                requested: ContextBudget::new(120_000, 1_048_576),
                declared: descriptor.output_budget,
            })
        );
    }

    #[test]
    fn output_budget_is_not_derived_from_the_input_budget() {
        let caller = caller();
        let descriptor = descriptor();
        let mut request = split_request();
        request.input_budget = Some(ContextBudget::new(1_000, 4_096));
        request.output_budget = Some(ContextBudget::new(24_000, 262_144));
        {
            let snapshot = request.snapshot.as_mut().expect("snapshot");
            snapshot.token_count = 900;
            snapshot.byte_count = 4_096;
        }

        let admitted = request.admit(&descriptor, &caller).expect("admit");
        assert_eq!(
            admitted.input_budget(),
            Some(ContextBudget::new(1_000, 4_096))
        );
        assert_eq!(
            admitted.output_budget(),
            Some(ContextBudget::new(24_000, 262_144))
        );

        // The result is held to the output grant, never to the smaller input
        // grant — a `min` of the two would reject this.
        assert_eq!(
            transform_result(262_144, 24_000).validate_for(&admitted),
            Ok(())
        );
        assert_eq!(
            transform_result(262_145, 24_000)
                .validate_for(&admitted)
                .err(),
            Some(ContextContractError::OutputExceedsBudget {
                side: "request",
                bytes: 262_145,
                tokens: 24_000,
            })
        );
    }

    #[test]
    fn split_budget_over_its_own_direction_is_rejected() {
        let caller = caller();
        let descriptor = descriptor();

        let mut over_input = split_request();
        over_input.input_budget = Some(ContextBudget::new(
            descriptor.input_budget.max_tokens + 1,
            1_048_576,
        ));
        assert_eq!(
            over_input.admit(&descriptor, &caller).err(),
            Some(ContextContractError::RequestBudgetExceedsDescriptor {
                side: "input",
                requested: ContextBudget::new(descriptor.input_budget.max_tokens + 1, 1_048_576),
                declared: descriptor.input_budget,
            })
        );

        let mut over_output = split_request();
        over_output.output_budget = Some(ContextBudget::new(
            24_000,
            descriptor.output_budget.max_bytes + 1,
        ));
        assert_eq!(
            over_output.admit(&descriptor, &caller).err(),
            Some(ContextContractError::RequestBudgetExceedsDescriptor {
                side: "output",
                requested: ContextBudget::new(24_000, descriptor.output_budget.max_bytes + 1),
                declared: descriptor.output_budget,
            })
        );
    }

    #[test]
    fn budget_forms_cannot_be_mixed_and_no_direction_may_be_omitted() {
        let caller = caller();
        let descriptor = descriptor();
        let budget = ContextBudget::new(24_000, 262_144);

        for mix_input in [true, false] {
            let mut ambiguous = legacy_request();
            if mix_input {
                ambiguous.input_budget = Some(budget);
            } else {
                ambiguous.output_budget = Some(budget);
            }
            assert_eq!(
                ambiguous.admit(&descriptor, &caller).err(),
                Some(ContextContractError::AmbiguousBudget)
            );
        }

        let mut only_input = legacy_request();
        only_input.budget = None;
        only_input.input_budget = Some(budget);
        assert_eq!(
            only_input.admit(&descriptor, &caller).err(),
            Some(ContextContractError::MissingBudget { side: "output" })
        );

        let mut only_output = legacy_request();
        only_output.budget = None;
        only_output.output_budget = Some(budget);
        assert_eq!(
            only_output.admit(&descriptor, &caller).err(),
            Some(ContextContractError::MissingBudget { side: "input" })
        );

        let mut no_budget = legacy_request();
        no_budget.budget = None;
        assert_eq!(
            no_budget.admit(&descriptor, &caller).err(),
            Some(ContextContractError::MissingBudget { side: "input" })
        );

        // A zero budget is unset, never unlimited.
        let mut zero = legacy_request();
        zero.budget = Some(ContextBudget::new(0, 262_144));
        assert_eq!(
            zero.admit(&descriptor, &caller).err(),
            Some(ContextContractError::EmptyBudget)
        );

        let mut zero_split = split_request();
        zero_split.output_budget = Some(ContextBudget::new(24_000, 0));
        assert_eq!(
            zero_split.admit(&descriptor, &caller).err(),
            Some(ContextContractError::EmptyBudget)
        );
    }

    #[test]
    fn snapshot_must_fit_both_the_declared_and_the_requested_input_ceiling() {
        let caller = caller();
        let descriptor = descriptor();

        // The descriptor ceiling is checked first, so the error names the
        // declaration that was overrun.
        let mut over_descriptor = legacy_request();
        over_descriptor
            .snapshot
            .as_mut()
            .expect("snapshot")
            .byte_count = descriptor.input_budget.max_bytes + 1;
        assert_eq!(
            over_descriptor.admit(&descriptor, &caller).err(),
            Some(ContextContractError::SnapshotExceedsInputBudget {
                side: "descriptor",
                tokens: 900,
                bytes: descriptor.input_budget.max_bytes + 1,
                max_tokens: descriptor.input_budget.max_tokens,
                max_bytes: descriptor.input_budget.max_bytes,
            })
        );

        // Then the per-call ceiling, which the legacy form sets to its one
        // number.
        let mut over_request = legacy_request();
        over_request.snapshot.as_mut().expect("snapshot").byte_count = 262_145;
        assert_eq!(
            over_request.admit(&descriptor, &caller).err(),
            Some(ContextContractError::SnapshotExceedsInputBudget {
                side: "request",
                tokens: 900,
                bytes: 262_145,
                max_tokens: 24_000,
                max_bytes: 262_144,
            })
        );

        let mut over_tokens = legacy_request();
        over_tokens.snapshot.as_mut().expect("snapshot").token_count = 24_001;
        assert_eq!(
            over_tokens.admit(&descriptor, &caller).err(),
            Some(ContextContractError::SnapshotExceedsInputBudget {
                side: "request",
                tokens: 24_001,
                bytes: 40_000,
                max_tokens: 24_000,
                max_bytes: 262_144,
            })
        );
    }

    // ------------------------------------------------------------------
    // snapshot presence per operation
    // ------------------------------------------------------------------

    #[test]
    fn transforming_operations_require_a_snapshot() {
        let caller = caller();
        let descriptor = all_phase_descriptor();

        for (phase, operation) in TRANSFORMING_OPERATIONS {
            assert!(operation.requires_snapshot(), "{operation:?}");
            let mut request = legacy_request();
            request.capability_id = descriptor.id.clone();
            request.phase = phase;
            request.operation = operation;
            request.snapshot = None;
            assert_eq!(
                request.admit(&descriptor, &caller).err(),
                Some(ContextContractError::MissingSnapshot { operation })
            );

            request.snapshot = Some(snapshot());
            assert!(
                request.admit(&descriptor, &caller).is_ok(),
                "{operation:?} must admit with a snapshot"
            );
        }
    }

    #[test]
    fn meta_operations_admit_without_snapshot_budget_or_fingerprint() {
        let caller = caller();
        let descriptor = all_phase_descriptor();

        for (phase, operation) in META_OPERATIONS {
            assert!(!operation.requires_snapshot(), "{operation:?}");
            let request = meta_request(phase, operation);
            let admitted = request.admit(&descriptor, &caller).expect("meta admit");

            assert_eq!(admitted.operation(), operation);
            assert!(!admitted.expects_result());
            assert!(admitted.snapshot().is_none());
            assert!(admitted.input_budget().is_none());
            assert!(admitted.output_budget().is_none());
            assert!(admitted.input_fingerprint().is_none());

            // Nothing was transformed, so nothing may be certified.
            assert_eq!(
                transform_result(0, 0).validate_for(&admitted).err(),
                Some(ContextContractError::ResultForMetaOperation { operation })
            );
        }
    }

    #[test]
    fn meta_operations_refuse_a_snapshot_budget_or_fingerprint() {
        let caller = caller();
        let descriptor = all_phase_descriptor();
        let (phase, operation) = (ContextPhase::Status, ContextOperation::Status);

        let mut with_snapshot = meta_request(phase, operation);
        with_snapshot.snapshot = Some(snapshot());
        assert_eq!(
            with_snapshot.admit(&descriptor, &caller).err(),
            Some(ContextContractError::UnexpectedSnapshot { operation })
        );

        for legacy in [true, false] {
            let mut with_budget = meta_request(phase, operation);
            if legacy {
                with_budget.budget = Some(ContextBudget::new(1_000, 2_000));
            } else {
                with_budget.output_budget = Some(ContextBudget::new(1_000, 2_000));
            }
            assert_eq!(
                with_budget.admit(&descriptor, &caller).err(),
                Some(ContextContractError::UnexpectedBudget { operation })
            );
        }

        let mut with_fingerprint = meta_request(phase, operation);
        with_fingerprint.input_fingerprint = Some("sha256:deadbeef".to_string());
        assert_eq!(
            with_fingerprint.admit(&descriptor, &caller).err(),
            Some(ContextContractError::UnexpectedFingerprint { operation })
        );
    }

    // ------------------------------------------------------------------
    // canonical fingerprints
    // ------------------------------------------------------------------

    #[test]
    fn canonical_fingerprint_is_key_order_independent_and_array_order_sensitive() {
        let left = json!({"b": [1, 2, {"z": true, "y": false}], "a": "x"});
        let reordered_keys = json!({"a": "x", "b": [1, 2, {"y": false, "z": true}]});
        let reordered_array = json!({"a": "x", "b": [2, 1, {"y": false, "z": true}]});

        assert_eq!(
            canonical_fingerprint(&left),
            canonical_fingerprint(&reordered_keys),
            "object key order is not semantic"
        );
        assert_eq!(
            canonical_fingerprint(&left),
            canonical_fingerprint(&left),
            "the derivation is deterministic"
        );
        assert_ne!(
            canonical_fingerprint(&left),
            canonical_fingerprint(&reordered_array),
            "array order is semantic"
        );

        let token = canonical_fingerprint(&left);
        assert!(token.starts_with(CONTEXT_FINGERPRINT_PREFIX));
        assert_eq!(
            token.len(),
            CONTEXT_FINGERPRINT_PREFIX.len() + 64,
            "sha256 is 64 hex chars"
        );
        assert!(token[CONTEXT_FINGERPRINT_PREFIX.len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn snapshot_payload_fingerprint_is_the_canonical_one() {
        let snapshot = snapshot();
        assert_eq!(
            snapshot.payload_fingerprint(),
            canonical_fingerprint(&snapshot_payload())
        );
        assert_eq!(snapshot.payload_fingerprint(), snapshot.input_fingerprint);
    }

    #[test]
    fn fingerprints_must_be_present_bounded_and_agree() {
        let caller = caller();
        let descriptor = descriptor();
        let overlong = "x".repeat(CONTEXT_FINGERPRINT_MAX_LEN + 1);

        let mut missing = legacy_request();
        missing.input_fingerprint = None;
        assert_eq!(
            missing.admit(&descriptor, &caller).err(),
            Some(ContextContractError::MissingFingerprint {
                field: "request.input_fingerprint",
            })
        );

        let mut empty = legacy_request();
        empty.input_fingerprint = Some("   ".to_string());
        assert_eq!(
            empty.admit(&descriptor, &caller).err(),
            Some(ContextContractError::EmptyFingerprint {
                field: "request.input_fingerprint",
            })
        );

        let mut too_long = legacy_request();
        too_long.input_fingerprint = Some(overlong.clone());
        assert_eq!(
            too_long.admit(&descriptor, &caller).err(),
            Some(ContextContractError::FingerprintTooLong {
                field: "request.input_fingerprint",
                len: CONTEXT_FINGERPRINT_MAX_LEN + 1,
                max: CONTEXT_FINGERPRINT_MAX_LEN,
            })
        );

        // The snapshot side is held to the same rule, and disagreement is a
        // stale input, not a resynchronisation opportunity.
        let mut stale = legacy_request();
        stale.input_fingerprint = Some("sha256:other".to_string());
        assert_eq!(
            stale.admit(&descriptor, &caller).err(),
            Some(ContextContractError::FingerprintMismatch {
                expected: "sha256:other".to_string(),
                found: input_fingerprint(),
            })
        );

        let mut snapshot_empty = legacy_request();
        snapshot_empty
            .snapshot
            .as_mut()
            .expect("snapshot")
            .input_fingerprint = String::new();
        assert_eq!(
            snapshot_empty.admit(&descriptor, &caller).err(),
            Some(ContextContractError::EmptyFingerprint {
                field: "snapshot.input_fingerprint",
            })
        );

        let mut snapshot_too_long = legacy_request();
        snapshot_too_long
            .snapshot
            .as_mut()
            .expect("snapshot")
            .input_fingerprint = overlong;
        assert_eq!(
            snapshot_too_long.admit(&descriptor, &caller).err(),
            Some(ContextContractError::FingerprintTooLong {
                field: "snapshot.input_fingerprint",
                len: CONTEXT_FINGERPRINT_MAX_LEN + 1,
                max: CONTEXT_FINGERPRINT_MAX_LEN,
            })
        );

        // The exact limit is still a token, not a payload copied into a digest.
        let at_limit = "x".repeat(CONTEXT_FINGERPRINT_MAX_LEN);
        let mut boundary = legacy_request();
        boundary.input_fingerprint = Some(at_limit.clone());
        boundary
            .snapshot
            .as_mut()
            .expect("snapshot")
            .input_fingerprint = at_limit;
        assert!(boundary.admit(&descriptor, &caller).is_ok());
    }

    // ------------------------------------------------------------------
    // transform results
    // ------------------------------------------------------------------

    #[test]
    fn transform_result_is_bounded_by_the_admitted_output_budget() {
        let caller = caller();
        let descriptor = descriptor();
        let request = legacy_request();
        let admitted = request.admit(&descriptor, &caller).expect("admit");

        assert_eq!(
            transform_result(262_144, 24_000).validate_for(&admitted),
            Ok(()),
            "the output budget is inclusive"
        );
        assert_eq!(
            transform_result(262_145, 10).validate_for(&admitted).err(),
            Some(ContextContractError::OutputExceedsBudget {
                side: "request",
                bytes: 262_145,
                tokens: 10,
            })
        );
        assert_eq!(
            transform_result(10, 24_001).validate_for(&admitted).err(),
            Some(ContextContractError::OutputExceedsBudget {
                side: "request",
                bytes: 10,
                tokens: 24_001,
            })
        );
    }

    #[test]
    fn transform_result_must_carry_the_admitted_fingerprint() {
        let caller = caller();
        let descriptor = descriptor();
        let request = legacy_request();
        let admitted = request.admit(&descriptor, &caller).expect("admit");

        assert_eq!(transform_result(1024, 256).validate_for(&admitted), Ok(()));

        let mut stale = transform_result(1024, 256);
        stale.input_fingerprint = "sha256:cafe".to_string();
        assert_eq!(
            stale.validate_for(&admitted).err(),
            Some(ContextContractError::FingerprintMismatch {
                expected: input_fingerprint(),
                found: "sha256:cafe".to_string(),
            })
        );

        let mut empty = transform_result(1024, 256);
        empty.input_fingerprint = "  ".to_string();
        assert_eq!(
            empty.validate_for(&admitted).err(),
            Some(ContextContractError::EmptyFingerprint {
                field: "result.input_fingerprint",
            })
        );

        let mut too_long = transform_result(1024, 256);
        too_long.input_fingerprint = "x".repeat(CONTEXT_FINGERPRINT_MAX_LEN + 1);
        assert_eq!(
            too_long.validate_for(&admitted).err(),
            Some(ContextContractError::FingerprintTooLong {
                field: "result.input_fingerprint",
                len: CONTEXT_FINGERPRINT_MAX_LEN + 1,
                max: CONTEXT_FINGERPRINT_MAX_LEN,
            })
        );
    }

    // ------------------------------------------------------------------
    // ids, handles, sessions and request ids must be usable keys
    // ------------------------------------------------------------------

    #[test]
    fn ids_handles_sessions_and_request_ids_must_be_untrimmed_and_non_empty() {
        // Capability id.
        let mut empty_id = descriptor();
        empty_id.id = "   ".to_string();
        assert_eq!(empty_id.validate(), Err(ContextContractError::EmptyId));

        let mut padded_id = descriptor();
        padded_id.id = format!(" {CAPABILITY_ID}");
        assert_eq!(
            padded_id.validate(),
            Err(ContextContractError::UntrimmedCapabilityId {
                id: format!(" {CAPABILITY_ID}"),
            })
        );

        // Scope handle.
        let mut padded_handle = descriptor();
        padded_handle.scope.handles = vec![format!(" {CALLER_SESSION}")];
        assert_eq!(
            padded_handle.validate(),
            Err(ContextContractError::UntrimmedScopeHandle {
                handle: format!(" {CALLER_SESSION}"),
            })
        );

        // Caller identity, as supplied by the host boundary.
        assert_eq!(
            AuthenticatedCaller::from_host_boundary("   ").err(),
            Some(ContextContractError::EmptySessionId {
                field: "caller.session_id",
            })
        );
        assert_eq!(
            AuthenticatedCaller::from_host_boundary(" sess-1").err(),
            Some(ContextContractError::UntrimmedSessionId {
                session_id: " sess-1".to_string(),
            })
        );

        let caller = caller();
        let descriptor = descriptor();

        let mut empty_session = legacy_request();
        empty_session
            .snapshot
            .as_mut()
            .expect("snapshot")
            .session_id = String::new();
        assert_eq!(
            empty_session.admit(&descriptor, &caller).err(),
            Some(ContextContractError::EmptySessionId {
                field: "snapshot.session_id",
            })
        );

        let mut padded_session = legacy_request();
        padded_session
            .snapshot
            .as_mut()
            .expect("snapshot")
            .session_id = format!(" {CALLER_SESSION}");
        assert_eq!(
            padded_session.admit(&descriptor, &caller).err(),
            Some(ContextContractError::UntrimmedSessionId {
                session_id: format!(" {CALLER_SESSION}"),
            })
        );

        // The request id is the dedup key downstream will use.
        for bad_request_id in ["", "   ", " req-1"] {
            let mut request = legacy_request();
            request.request_id = bad_request_id.to_string();
            assert_eq!(
                request.admit(&descriptor, &caller).err(),
                Some(ContextContractError::InvalidRequestId {
                    request_id: bad_request_id.to_string(),
                })
            );
        }
    }

    // ------------------------------------------------------------------
    // rejection of undeclared phases and operation mismatches
    // ------------------------------------------------------------------

    #[test]
    fn undeclared_phase_is_rejected() {
        let caller = caller();
        let descriptor = descriptor();

        let mut wrong = legacy_request();
        wrong.phase = ContextPhase::Search;
        wrong.operation = ContextOperation::Search;
        assert_eq!(
            wrong.admit(&descriptor, &caller).err(),
            Some(ContextContractError::UndeclaredPhase {
                capability_id: CAPABILITY_ID.to_string(),
                phase: ContextPhase::Search,
            })
        );

        assert!(legacy_request().admit(&descriptor, &caller).is_ok());
    }

    #[test]
    fn operation_phase_mismatch_is_rejected() {
        let caller = caller();
        let mut mismatch = legacy_request();
        mismatch.operation = ContextOperation::Restore;
        assert_eq!(
            mismatch.admit(&descriptor(), &caller).err(),
            Some(ContextContractError::PhaseOperationMismatch {
                operation: ContextOperation::Restore,
                required: ContextPhase::Recovery,
                declared: ContextPhase::Compaction,
            })
        );
    }

    #[test]
    fn request_for_another_capability_is_rejected() {
        let caller = caller();
        let mut request = legacy_request();
        request.capability_id = "acme/other".to_string();
        assert_eq!(
            request.admit(&descriptor(), &caller).err(),
            Some(ContextContractError::CapabilityMismatch {
                requested: "acme/other".to_string(),
                descriptor: CAPABILITY_ID.to_string(),
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

        let caller = caller();
        let mut request = legacy_request();
        request.api_version = "context.fabric/v2".to_string();
        assert_eq!(
            request.admit(&self::descriptor(), &caller).err(),
            Some(ContextContractError::UnsupportedApiVersion {
                found: "context.fabric/v2".to_string(),
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

        assert_eq!(cross_session_descriptor(&["sess-2"]).validate(), Ok(()));
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

        // `requires_snapshot` is the single source the admission path uses; the
        // meta/transforming split above must cover every operation exactly once.
        for operation in ContextOperation::ALL {
            let expected_snapshot = matches!(
                operation,
                ContextOperation::Admit
                    | ContextOperation::Preflight
                    | ContextOperation::Compact
                    | ContextOperation::Restore
                    | ContextOperation::Search
            );
            assert_eq!(
                operation.requires_snapshot(),
                expected_snapshot,
                "{operation:?}"
            );
        }
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
    }

    // ------------------------------------------------------------------
    // wire defaults
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
