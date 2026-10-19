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
//! Only the envelope is defined here. What a snapshot payload contains (and
//! which session/project data it may contain) is decided by the core scope
//! filter that fills [`ContextSnapshot::payload`]; hidden reasoning and
//! credentials are excluded before the payload is built.

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

/// A hard input or output ceiling, in both units the pipeline bills in.
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
    pub persistence: ContextPersistencePolicy,
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

/// A scope-filtered, bounded view of the current context.
///
/// `payload` is the capability's own view, already filtered to
/// [`ContextScope`]; it is deliberately schema-less here so this module does not
/// own a second message DTO. Hidden reasoning, credentials, unauthorised
/// project files and other sessions never enter it.
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

/// One wire call into a capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextCapabilityRequest {
    pub request_id: String,
    pub capability_id: String,
    pub api_version: String,
    pub phase: ContextPhase,
    pub operation: ContextOperation,
    pub budget: ContextBudget,
    pub input_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ContextSnapshot>,
}

impl ContextCapabilityRequest {
    /// Validate this call against the descriptor it names.
    ///
    /// Rejects a request aimed at another capability, an unknown wire version,
    /// a phase the capability never declared, an operation that belongs to a
    /// different phase, and an invalid budget.
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
        budget.validate()?;
        if self.output_bytes > budget.max_bytes || self.output_tokens > budget.max_tokens {
            return Err(ContextContractError::OutputExceedsBudget {
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
            budget: ContextBudget::new(120_000, 1_048_576),
            input_fingerprint: "sha256:deadbeef".to_string(),
            snapshot: None,
        }
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

        let declared = request(ContextPhase::Compaction, ContextOperation::Compact);
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
