//! Tool-owned schema + strict handler authorization for the diagnostic plane.
//!
//! This module is the SOLE authority boundary the operator-facing diagnostics
//! surface owns. Every operation — including `status` — passes through the
//! strict three-part ambient-identity check before any `DiagnosticControl`
//! method is reached. The arguments on the wire NEVER carry caller identity:
//! the JSON schema rejects unknown fields (so a `role`, `loopback`, or
//! `connection_id` field cannot be smuggled in), and the execution adapter
//! re-reads the task-locals (`CALLER_ROLE` / `CALLER_IS_LOOPBACK` /
//! `CALLER_CONN_ID`) inside [`execute_capability_projection_diagnostics`].
//!
//! The diagnostic control surface itself is owned by Task 1 (`src/capability/
//! diagnostic_control.rs`); Task 2 only owns the tool-side wiring and gate.
//! Startup runtime registration (the `ALEPH_CAPABILITY_DIAGNOSTICS=1` env
//! gate, the `BUILTIN_TOOL_DEFINITIONS` row, the `OPERATOR_TOOLS` /
//! `DANGEROUS_TOOLS` census entries, and the runtime-owned `Arc<DiagnosticControl>`
//! injection) is Task 3's work — Task 2 does NOT touch any of it.
//!
//! # Operation set (the seven fixed operations)
//!
//! | operation        | fields                              | result |
//! |------------------|-------------------------------------|--------|
//! | `status`         | —                                   | reads applied + queue/telemetry state |
//! | `bump_runtime`   | —                                   | returns the new owner generation (mutation result, no delivery claim) |
//! | `revoke_tool`    | `tool_name: String`                 | revocation result (mutation result) |
//! | `dispose_runtime`| —                                   | disposition result (mutation result) |
//! | `hold`           | `plane`, `duration_ms` (1..=5000)   | hold completed (timer / release / close) |
//! | `release`        | —                                   | release result (idempotent) |
//! | `close`          | —                                   | shutdown report OR `CloseTimeout` tool error |
//!
//! # Status fields distinguish queued from applied
//!
//! `DiagnosticStatusReport` separates the post-applier applied snapshot
//! (`applied_tool_ids`, `applied_owner_generations`) from the default consumer
//! queue (`pending_depth` / `pending_capacity`) and from the overflow / lag
//! telemetry counters. The publisher's initial snapshot never appears as
//! "delivered" — `applied_tool_ids` is empty before the default applier
//! drains at least once.
//!
//! # Mutation replies
//!
//! Every mutating reply carries only the authority mutation result (the new
//! generation, the revoked name, the held plane/duration, etc.). None of them
//! claim the result is "delivered" — the model must observe `status()`
//! before trusting a mutation as delivered, which is the same rule the
//! `ProjectionHost` host makes externally visible.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capability::diagnostic_control::{
    DiagnosticControl, DiagnosticError, DiagnosticLifecycle, DiagnosticPlane, DiagnosticStatus,
};
use crate::capability::projection_host::ProjectionShutdownOutcome;
use crate::gateway::caller_identity::{
    current_caller_conn_id, current_caller_is_loopback, current_caller_role,
};

// ============================================================================
// Tool name (fixed)
// ============================================================================

/// Exact tool name. The conditional registration shape Task 3 wires up keys
/// on this constant; the operator-facing allow-list (`OPERATOR_TOOLS`) and
/// the dangerous-tool census (`DANGEROUS_TOOLS`) name this exact value too.
pub const CAPABILITY_PROJECTION_DIAGNOSTICS: &str = "capability_projection_diagnostics";

// ============================================================================
// Wire schema — request
// ============================================================================

/// Wire-level mirror of [`DiagnosticPlane`]. Same two members, snake_case
/// spelling on the wire.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPlaneArg {
    SourceIntake,
    Delivery,
}

impl From<DiagnosticPlaneArg> for DiagnosticPlane {
    fn from(arg: DiagnosticPlaneArg) -> Self {
        match arg {
            DiagnosticPlaneArg::SourceIntake => DiagnosticPlane::SourceIntake,
            DiagnosticPlaneArg::Delivery => DiagnosticPlane::Delivery,
        }
    }
}

/// Reject any `duration_ms` outside `1..=5000` at parse time. Defense in
/// depth: the gate already requires an authorized caller, but a malformed
/// schema should never reach the host.
fn deserialize_bounded_duration_ms<'de, D>(de: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let n = u32::deserialize(de)?;
    if (1..=5000).contains(&n) {
        Ok(n)
    } else {
        Err(serde::de::Error::custom(format!(
            "diagnostics.duration_ms {n} outside 1..=5000"
        )))
    }
}

/// The seven fixed operations. Internally tagged on `operation`; unknown
/// operation names fail to deserialize. `deny_unknown_fields` rejects any
/// extra key on the operation object — so a caller cannot smuggle
/// `role` / `loopback` / `connection_id` / `kind` / `namespace` /
/// `generation` / any other capability-system field through the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiagnosticRequest {
    /// Read the REAL applied state plus queue / telemetry counters.
    /// No fields, but expressed as an empty struct (`Status {}`) rather than
    /// a unit variant: in an internally-tagged enum, `deny_unknown_fields`
    /// makes an empty-struct variant reject `role` / `loopback` /
    /// `connection_id` / `kind` / `namespace` / `generation` smuggled
    /// alongside the tag, whereas a unit variant silently accepts and drops
    /// extra keys. Pinned by
    /// `empty_struct_variants_reject_extra_keys_but_unit_variants_would_not`.
    Status {},
    /// Invoke the existing tree's `bump(Runtime)` and return the new generation.
    BumpRuntime {},
    /// Revoke the tool binding for `tool_name` on the same tree.
    RevokeTool { tool_name: String },
    /// Dispose the existing Runtime lifetime on the same tree (irreversible).
    DisposeRuntime {},
    /// Acquire the named hold for `duration_ms` (1..=5000).
    Hold {
        plane: DiagnosticPlaneArg,
        #[serde(deserialize_with = "deserialize_bounded_duration_ms")]
        duration_ms: u32,
    },
    /// Idempotent release of any active hold.
    Release {},
    /// Close the host and await the shared completion boundary (5000 ms).
    Close {},
}

/// Parse a JSON value into a [`DiagnosticRequest`]. The schema rejects unknown
/// operations and unknown fields; `duration_ms` must be in `1..=5000`.
pub fn parse_request(value: serde_json::Value) -> Result<DiagnosticRequest, serde_json::Error> {
    serde_json::from_value(value)
}

// ============================================================================
// Wire schema — response
// ============================================================================

/// Lifecycle state as exposed by [`DiagnosticStatus`]. Wire spelling is
/// snake_case to match the rest of the project's RPC surface.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLifecycleReport {
    Active,
    Closing,
    Closed,
}

impl From<DiagnosticLifecycle> for DiagnosticLifecycleReport {
    fn from(l: DiagnosticLifecycle) -> Self {
        match l {
            DiagnosticLifecycle::Active => Self::Active,
            DiagnosticLifecycle::Closing => Self::Closing,
            DiagnosticLifecycle::Closed => Self::Closed,
        }
    }
}

/// Read-only diagnostic snapshot, distinguishing queued from applied.
///
/// `applied_tool_ids` and `applied_owner_generations` come from the REAL
/// applied snapshot, never from the publisher's seed. Both vectors are
/// sorted by tool name so the JSON output is stable across runs.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticStatusReport {
    pub lifecycle: DiagnosticLifecycleReport,
    pub registry_cursor: u64,
    pub pending_depth: usize,
    pub pending_capacity: usize,
    pub replacement_count: u64,
    pub lag_count: u64,
    pub applied_tool_ids: Vec<String>,
    /// Sorted `(tool_name, generation)` pairs.
    pub applied_owner_generations: Vec<[String; 2]>,
    /// Real default-applier `Invalidated` consumption count.
    pub applied_invalidation_count: u64,
    /// Real default-applier consumed-replacement `Snapshot` count.
    pub applied_replacement_count: u64,
    /// Registry cursor of the most recent applied replacement `Snapshot`.
    pub last_replacement_registry_cursor: Option<u64>,
    /// Tool-id names of the most recent applied replacement `Snapshot`,
    /// sorted by name for wire stability.
    pub last_replacement_tool_ids: Vec<String>,
}

impl From<DiagnosticStatus> for DiagnosticStatusReport {
    fn from(s: DiagnosticStatus) -> Self {
        let mut ids: Vec<String> = s.applied_tool_ids.iter().map(|c| c.name.clone()).collect();
        ids.sort();
        let mut gens: Vec<[String; 2]> = s
            .applied_owner_generations
            .into_iter()
            .map(|(k, v)| [k.name, v.0.to_string()])
            .collect();
        gens.sort_by(|a, b| a[0].cmp(&b[0]));
        let mut last_replacement_tool_ids = s.last_replacement_tool_ids.clone();
        last_replacement_tool_ids.sort();
        DiagnosticStatusReport {
            lifecycle: s.lifecycle.into(),
            registry_cursor: s.registry_cursor.0,
            pending_depth: s.pending_depth,
            pending_capacity: s.pending_capacity,
            replacement_count: s.replacement_count,
            lag_count: s.lag_count,
            applied_tool_ids: ids,
            applied_owner_generations: gens,
            applied_invalidation_count: s.applied_invalidation_count,
            applied_replacement_count: s.applied_replacement_count,
            last_replacement_registry_cursor: s.last_replacement_registry_cursor.map(|c| c.0),
            last_replacement_tool_ids,
        }
    }
}

/// One shared-completion-boundary report from `close()`. `source_joined` /
/// `applier_joined` reflect clean exits; `source_failed` / `applier_failed`
/// reflect `JoinError` returns. No delivery claim is attached.
#[derive(Debug, Clone, Serialize)]
pub struct ShutdownReport {
    pub source_joined: bool,
    pub applier_joined: bool,
    pub source_failed: bool,
    pub applier_failed: bool,
}

impl From<ProjectionShutdownOutcome> for ShutdownReport {
    fn from(o: ProjectionShutdownOutcome) -> Self {
        ShutdownReport {
            source_joined: o.source_joined,
            applier_joined: o.applier_joined,
            source_failed: o.source_failed,
            applier_failed: o.applier_failed,
        }
    }
}

/// Successful reply. Tagged on `operation` to mirror the request. Mutating
/// replies carry only the mutation result — no claim that the mutation has
/// been observed by the default applier or has reached the run-loop-readable
/// applied state.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum DiagnosticResponse {
    Status(DiagnosticStatusReport),
    BumpRuntime {
        owner_generation: u64,
    },
    RevokeTool {
        tool_name: String,
        /// `true` when the id was already tombstoned and this call changed
        /// nothing (idempotent repeat); `false` when this call revoked a live
        /// binding. An unbound name is an `unknown_tool` error with no effect.
        already_revoked: bool,
    },
    DisposeRuntime,
    Hold {
        plane: DiagnosticPlaneArg,
        duration_ms: u32,
    },
    Release,
    Close(ShutdownReport),
}

// ============================================================================
// Errors
// ============================================================================

/// Tool-facing error. `Denied` is the authorization gate's only output —
/// every other variant is a domain error from `DiagnosticControl` re-exposed
/// so the handler can map them to a structured failure response.
///
/// Wire form mirrors [`DiagnosticResponse`]: internally tagged (here on
/// `error`), snake_case, carrying only each variant's own fields — e.g.
/// `{"error":"denied","reason":"..."}`,
/// `{"error":"close_timeout","elapsed_ms":5000}`, `{"error":"host_closed"}`.
/// Serialize-only: errors are produced by this handler, never parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum DiagnosticToolError {
    /// Authorization denied: caller does not pass the strict three-part check.
    /// Returned BEFORE any host/tree side-effect, for every operation
    /// (including `status`). `reason` names which of the three facts was
    /// missing or mismatched (a struct variant so the tagged wire form can
    /// carry it).
    Denied { reason: String },
    /// `hold` duration outside the 1..=5000 ms window. Defense in depth — the
    /// JSON schema clamps this at parse time; the runtime check covers any
    /// future allow-by-config path.
    InvalidHoldDuration,
    /// Operation attempted on a host that has reached its completion boundary.
    HostClosed,
    /// Another hold is already active for this host.
    HoldAlreadyActive,
    /// `close` did not observe the shared completion boundary within 5000 ms.
    /// The host remains close-requested / fail-closed. NEVER serialized as a
    /// successful `Close` response — this variant exists for that reason.
    CloseTimeout { elapsed_ms: u64 },
    /// `revoke_tool` found no live binding (and no tombstone) for the named
    /// tool. The tree is left unchanged: no tombstone is created.
    UnknownTool { name: String },
}

impl std::fmt::Display for DiagnosticToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied { reason } => write!(f, "diagnostics denied: {reason}"),
            Self::InvalidHoldDuration => write!(f, "diagnostics hold duration out of range"),
            Self::HostClosed => write!(f, "diagnostics host is closed"),
            Self::HoldAlreadyActive => write!(f, "diagnostics hold already active"),
            Self::CloseTimeout { elapsed_ms } => {
                write!(f, "diagnostics close timed out after {elapsed_ms} ms")
            }
            Self::UnknownTool { name } => {
                write!(f, "diagnostics revoke_tool unknown: {name}")
            }
        }
    }
}

impl std::error::Error for DiagnosticToolError {}

impl From<DiagnosticError> for DiagnosticToolError {
    fn from(e: DiagnosticError) -> Self {
        match e {
            DiagnosticError::HoldAlreadyActive => Self::HoldAlreadyActive,
            DiagnosticError::InvalidHoldDuration => Self::InvalidHoldDuration,
            DiagnosticError::CloseTimeout { elapsed_ms } => Self::CloseTimeout { elapsed_ms },
            DiagnosticError::Closed => Self::HostClosed,
            DiagnosticError::UnknownTool { name } => Self::UnknownTool { name },
            DiagnosticError::AuthorityMismatch => Self::Denied {
                reason: "diagnostic control authority mismatch".to_string(),
            },
        }
    }
}

// ============================================================================
// Authorization gate
// ============================================================================

/// Return `Some(reason)` if the caller fails the strict three-part check.
///
/// Must hold for EVERY operation, including `status`. The check is read
/// directly from the ambient task-locals — the request arguments are NEVER
/// trusted for any of these facts, and `TurnContext::caller_is_operator()`
/// is deliberately NOT consulted (it deliberately treats an absent role as
/// trusted; this gate is fail-closed on absence).
///
/// Reading order matches the brief: role → loopback → connection id. Any
/// missing or mismatched fact short-circuits with a descriptive reason.
/// `ALEPH_GATEWAY_TOOLS_ALLOW=capability_projection_diagnostics` cannot
/// bypass this gate (it is enforced BEFORE the call into
/// `DiagnosticControl`; the allow-list is consulted by a separate code path
/// upstream of this handler).
///
/// Each fact is read exactly once. An "actual" connection id is a
/// NON-EMPTY string: an ambient `Some("")` is denied just like `None`.
fn authorization_failure_reason() -> Option<String> {
    let role = current_caller_role();
    if role.as_deref() != Some("operator") {
        return Some(format!("caller role is not 'operator' (got {role:?})"));
    }
    if !current_caller_is_loopback() {
        return Some("caller is not loopback".to_string());
    }
    match current_caller_conn_id() {
        Some(id) if !id.is_empty() => None,
        Some(_) => Some("caller connection id is empty".to_string()),
        None => Some("caller connection id is missing".to_string()),
    }
}

// ============================================================================
// Execute
// ============================================================================

/// Execute one diagnostic operation against the supplied control, after
/// passing the strict three-part ambient-identity check.
///
/// # Authorization
///
/// The gate is enforced at the very top of this function — no
/// `DiagnosticControl` method runs until the caller's role, loopback flag,
/// and connection id all read back as expected from the task-locals. A
/// failure returns [`DiagnosticToolError::Denied`] with the missing fact
/// named in the message, and the host/tree is left untouched.
///
/// # Mapping
///
/// `DiagnosticError` is mapped 1:1 onto [`DiagnosticToolError`] via
/// `From<DiagnosticError>`. `CloseTimeout` is intentionally a tool error
/// variant — the response shape NEVER admits a successful closure on
/// timeout (the host is fail-closed on this path).
pub async fn execute_capability_projection_diagnostics(
    request: DiagnosticRequest,
    control: Arc<DiagnosticControl>,
) -> Result<DiagnosticResponse, DiagnosticToolError> {
    // Hard authorization gate (the sole authority for this tool).
    // The check is performed before ANY contact with the projection host or
    // ownership tree, and is independent of the request payload. Reads the
    // ambient caller-identity task-locals directly — never trusts role,
    // loopback, or connection_id from the JSON arguments, never consults
    // `TurnContext::caller_is_operator()`, and is not bypassed by
    // `ALEPH_GATEWAY_TOOLS_ALLOW` (the gateway only dispatches).
    if let Some(reason) = authorization_failure_reason() {
        return Err(DiagnosticToolError::Denied { reason });
    }

    match request {
        DiagnosticRequest::Status {} => {
            let s = control.status()?;
            Ok(DiagnosticResponse::Status(s.into()))
        }
        DiagnosticRequest::BumpRuntime {} => {
            let g = control.bump_runtime()?;
            Ok(DiagnosticResponse::BumpRuntime {
                owner_generation: g.0,
            })
        }
        DiagnosticRequest::RevokeTool { tool_name } => {
            let revoked_now = control.revoke_tool(&tool_name)?;
            Ok(DiagnosticResponse::RevokeTool {
                tool_name,
                already_revoked: !revoked_now,
            })
        }
        DiagnosticRequest::DisposeRuntime {} => {
            control.dispose_runtime()?;
            Ok(DiagnosticResponse::DisposeRuntime)
        }
        DiagnosticRequest::Hold { plane, duration_ms } => {
            control
                .hold(plane.into(), Duration::from_millis(u64::from(duration_ms)))
                .await?;
            Ok(DiagnosticResponse::Hold { plane, duration_ms })
        }
        DiagnosticRequest::Release {} => {
            control.release()?;
            Ok(DiagnosticResponse::Release)
        }
        DiagnosticRequest::Close {} => {
            let outcome = control.close().await?;
            Ok(DiagnosticResponse::Close(outcome.into()))
        }
    }
}

// ----------------------------------------------------------------------------
// AlephTool adapter — schema-only.
//
// Compile-forced minimal addition (see `task-3-report.md`). The real
// entry point stays `execute_capability_projection_diagnostics` plus
// `parse_request`, which the `ToolRegistry::execute_tool` dispatch arm
// invokes directly. A full `AlephTool` impl would duplicate the
// parse / execute / serialize pipeline already wired into the arm, and
// would need a `JsonSchema` derive on `DiagnosticRequest` (a wider
// module change). This struct is the minimum needed to register the
// schema in the runtime tool map: a one-method adapter mirroring the
// `MediaUnderstandTool { pipeline }` shape exactly. The `control`
// field is held so the registry keeps the handle alive for its
// lifetime; the `definition()` body only reads the const metadata, so
// no parallel state is created. The wire-level three-part check
// (operator + loopback + conn_id) is NOT bypassed — it is enforced by
// the dispatch arm before this adapter's existence matters.
// ----------------------------------------------------------------------------
#[derive(Clone)]
pub struct DiagnosticTool {
    /// Live `DiagnosticControl` handle. Held so the registry keeps the
    /// runtime in scope for the lifetime of the constructed struct
    /// (mirrors the `MediaUnderstandTool { pipeline }` shape); the
    /// `definition()` method itself only reads the const metadata, so
    /// the field is held-not-read by design.
    #[allow(dead_code)]
    control: Arc<DiagnosticControl>,
}

impl DiagnosticTool {
    /// Stable name exposed to the LLM-facing tool list and to the
    /// dispatch arm. Must match the OPERATOR_TOOLS / DANGEROUS_TOOLS
    /// entries in `gateway::method_authz` and `security::dangerous_tools`
    /// exactly (the tripwires there assert this).
    pub const NAME: &'static str = "capability_projection_diagnostics";

    /// LLM-facing description. The seven operations are listed in the
    /// order `parse_request` discriminates them; the strict
    /// `deny_unknown_fields` contract is enforced at parse time, not
    /// here, so the schema below only names the `operation` tag and
    /// leaves the variant body free-form.
    pub const DESCRIPTION: &'static str = "\
Capability-runtime diagnostic control surface. Enablement-gated on \
`ALEPH_CAPABILITY_DIAGNOSTICS=1` at startup; when disabled, the tool is \
not advertised, not dispatched, and hold / timer state is empty. The \
wire-level three-part check (operator role + loopback transport + \
non-empty connection id) lives in `execute_capability_projection_diagnostics`, \
so even when this entry is visible to the LLM only an authorized operator \
process can drive it. ALEPH_GATEWAY_TOOLS_ALLOW cannot bypass that gate.\n\
\n\
Operations (tagged `operation`):\n\
  status              — read REAL applied state, queue / telemetry counters (no args)\n\
  bump_runtime        — advance the runtime generation (no args)\n\
  revoke_tool         — drop a single tool from the live tree (`tool_name`)\n\
  dispose_runtime     — drop the whole runtime scope (no args)\n\
  hold                — freeze one plane (`plane`, `duration_ms` in 1..=5000)\n\
  release             — release any active hold (no args)\n\
  close               — shut the host (no args)";

    #[must_use]
    pub const fn new(control: Arc<DiagnosticControl>) -> Self {
        Self { control }
    }

    /// Schema for the LLM-facing `UnifiedTool`. The strict
    /// `deny_unknown_fields` contract is enforced by `parse_request` at
    /// dispatch time; this schema only advertises the `operation` tag
    /// and leaves the variant body free-form, so the model sees the
    /// seven operation names without an over-specified shape the
    /// parser would then have to keep in sync.
    #[must_use]
    pub fn definition(&self) -> crate::ToolDefinition {
        crate::ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": [
                            "status",
                            "bump_runtime",
                            "revoke_tool",
                            "dispose_runtime",
                            "hold",
                            "release",
                            "close",
                        ],
                    },
                },
                "required": ["operation"],
            }),
            requires_confirmation: false,
            category: crate::ToolCategory::Builtin,
            strict: false,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ownership::{LifetimeScope, OwnershipTree};
    use crate::capability::projection_host::ProjectionHost;
    use crate::gateway::caller_identity::{CALLER_CONN_ID, CALLER_IS_LOOPBACK, CALLER_ROLE};
    use crate::tools::descriptor::{
        ReplayPolicy, ToolCapabilityDescriptor, ToolKind, SCHEMA_VERSION,
    };
    use crate::tools::registry::ToolHandlerRegistry;
    use serde_json::json;

    // ---- Test scaffolding (mirrors `diagnostic_control.rs::tests`) ----

    struct FakeHandler {
        name: String,
    }

    #[async_trait::async_trait]
    impl crate::tools::handlers::ToolHandler for FakeHandler {
        async fn invoke(
            &self,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
            Ok(crate::session::events::ToolOutput {
                value: serde_json::json!({"tool": self.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> crate::tools::service::ToolDefinition {
            crate::tools::service::ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: crate::tools::service::ToolSource::Builtin,
                metadata: crate::tools::service::ToolDefinitionMetadata {
                    idempotent: false,
                    ..Default::default()
                },
            }
        }
    }

    fn fake(name: &str) -> std::sync::Arc<dyn crate::tools::handlers::ToolHandler> {
        std::sync::Arc::new(FakeHandler { name: name.into() })
    }

    fn desc(name: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.into(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: crate::tools::service::ToolSource::Builtin,
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: false,
            idempotent: false,
            concurrent_safe: false,
            max_duration_ms: None,
            revision: 0,
            implementation_contract: None,
        }
    }

    fn empty_control() -> Arc<DiagnosticControl> {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        Arc::new(DiagnosticControl::new(host, tree).unwrap())
    }

    /// Returns the (control, tree) tuple so tests can observe state directly
    /// without going through the control. The registry is owned by the host
    /// after mount (Task 1 design), so tests observe state via `ctrl.status()`
    /// and the tree.
    fn control_with_tool() -> (Arc<DiagnosticControl>, Arc<OwnershipTree>) {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).expect("register a");
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let ctrl = Arc::new(DiagnosticControl::new(host, Arc::clone(&tree)).unwrap());
        (ctrl, tree)
    }

    /// Async helper: poll `ctrl.status()` until `pred` holds, or time out.
    /// Keeps the tokio runtime unblocked so the default applier can keep
    /// running.
    async fn await_status<F>(ctrl: &DiagnosticControl, pred: F) -> DiagnosticStatus
    where
        F: Fn(&DiagnosticStatus) -> bool,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = ctrl.status() {
                if pred(&s) {
                    return s;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for status predicate");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Scope a future with the supplied three ambient identity facts.
    /// Mimics the gateway dispatch loop's task-local scoping.
    async fn with_ambient<F, T>(role: Option<&str>, loopback: bool, conn: Option<&str>, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        CALLER_ROLE
            .scope(role.map(str::to_string), async move {
                CALLER_IS_LOOPBACK
                    .scope(loopback, async move {
                        CALLER_CONN_ID.scope(conn.map(str::to_string), fut).await
                    })
                    .await
            })
            .await
    }

    // ---- TDD test 1: three-part ambient gate ----

    /// Every missing / mismatched ambient identity fact must DENY the
    /// operation, INCLUDING `status`. The only admitted ambient combination
    /// is `("operator", true, Some(_))`. No host/tree mutation may occur
    /// even after the full loop.
    #[tokio::test]
    async fn diagnostics_requires_handler_local_operator_loopback_and_connection() {
        let (ctrl, tree) = control_with_tool();
        // Let the default applier converge so any unauthorized mutation
        // would be observable in the next status read.
        let _ = await_status(&ctrl, |s| !s.applied_tool_ids.is_empty()).await;
        let status_before = ctrl.status().expect("status before");
        assert_eq!(status_before.applied_tool_ids.len(), 1);
        assert!(!tree.is_disposed(LifetimeScope::Runtime));

        // (role, loopback, conn_id). All seven of the first combos must deny.
        let deny_combos: [(Option<&str>, bool, Option<&str>); 7] = [
            (None, false, None),
            (Some("operator"), false, None),
            (Some("operator"), true, None),
            (Some("member"), true, Some("127.0.0.1:1")),
            (Some("guest"), true, Some("127.0.0.1:2")),
            (Some("operator"), false, Some("127.0.0.1:3")),
            (None, true, Some("127.0.0.1:4")),
        ];

        for (role, loopback, conn) in deny_combos {
            // Status: must deny
            let res = with_ambient(role, loopback, conn, async {
                execute_capability_projection_diagnostics(
                    DiagnosticRequest::Status {},
                    ctrl.clone(),
                )
                .await
            })
            .await;
            assert!(
                matches!(res, Err(DiagnosticToolError::Denied { .. })),
                "status admitted for role={role:?} loopback={loopback} conn={conn:?}: {res:?}"
            );

            // One mutating op (bump_runtime): must also deny
            let res = with_ambient(role, loopback, conn, async {
                execute_capability_projection_diagnostics(
                    DiagnosticRequest::BumpRuntime {},
                    ctrl.clone(),
                )
                .await
            })
            .await;
            assert!(
                matches!(res, Err(DiagnosticToolError::Denied { .. })),
                "bump_runtime admitted for role={role:?} loopback={loopback} conn={conn:?}: {res:?}"
            );

            // dispose_runtime: must also deny (would be irreversible if it
            // got through).
            let res = with_ambient(role, loopback, conn, async {
                execute_capability_projection_diagnostics(
                    DiagnosticRequest::DisposeRuntime {},
                    ctrl.clone(),
                )
                .await
            })
            .await;
            assert!(
                matches!(res, Err(DiagnosticToolError::Denied { .. })),
                "dispose_runtime admitted for role={role:?} loopback={loopback} conn={conn:?}: {res:?}"
            );
        }

        // The only admitted combo (operator + loopback + conn_id) actually
        // executes the mutating op and returns Ok with the new generation.
        let res = with_ambient(Some("operator"), true, Some("127.0.0.1:9"), async {
            execute_capability_projection_diagnostics(
                DiagnosticRequest::BumpRuntime {},
                ctrl.clone(),
            )
            .await
        })
        .await;
        let owner_gen = match res {
            Ok(DiagnosticResponse::BumpRuntime { owner_generation }) => owner_generation,
            other => panic!("admitted bump_runtime returned {other:?}"),
        };
        assert!(owner_gen > 0);

        // State after the unauthorized loop + the single admitted bump:
        // applied snapshot now reflects the bumped generation on "a".
        let _ = await_status(&ctrl, |s| {
            s.applied_owner_generations
                .values()
                .any(|g| g.0 == owner_gen)
        })
        .await;
        let status_after = ctrl.status().expect("status after");
        assert_eq!(status_after.applied_tool_ids.len(), 1);
        assert!(
            status_after
                .applied_owner_generations
                .values()
                .any(|g| g.0 == owner_gen),
            "admitted bump must have advanced applied generation"
        );
        assert!(!tree.is_disposed(LifetimeScope::Runtime));
    }

    // ---- TDD test 2: spoofed identity arguments ----

    /// A request carrying `role` / `loopback` / `connection_id` (or any other
    /// capability-system field like `kind` / `namespace` / `generation`)
    /// MUST be rejected — either at parse time (`deny_unknown_fields`), or
    /// at the gate when the request is constructed directly. The ambient
    /// identity is NEVER trusted from arguments.
    #[tokio::test]
    async fn spoofed_identity_arguments_do_not_authorize() {
        let (ctrl, tree) = control_with_tool();
        let _ = await_status(&ctrl, |s| !s.applied_tool_ids.is_empty()).await;
        let status_before = ctrl.status().expect("status before");

        // 1) JSON with identity fields must FAIL to parse.
        for payload in [
            json!({"operation": "status", "role": "operator", "loopback": true, "connection_id": "spoof"}),
            json!({"operation": "bump_runtime", "role": "operator"}),
            json!({"operation": "revoke_tool", "tool_name": "a", "namespace": "evil"}),
            json!({"operation": "bump_runtime", "kind": "tool", "namespace": "x", "generation": 99}),
            json!({"operation": "close", "loopback": true}),
        ] {
            let repr = payload.to_string();
            assert!(
                parse_request(payload).is_err(),
                "request parser must reject identity-bearing fields: {repr}"
            );
        }

        // 2) Even with a request constructed directly, the gate requires
        // the AMBIENT task-locals. Arguments are never trusted.
        let direct_requests = [
            DiagnosticRequest::Status {},
            DiagnosticRequest::BumpRuntime {},
            DiagnosticRequest::DisposeRuntime {},
            DiagnosticRequest::Close {},
        ];
        for req in direct_requests {
            let label = format!("{req:?}");
            let res = execute_capability_projection_diagnostics(req, ctrl.clone()).await;
            assert!(
                matches!(res, Err(DiagnosticToolError::Denied { .. })),
                "no ambient identity must deny {label}: {res:?}"
            );
        }

        // 3) Even when the ambient task-locals authorize, an extra
        // `connection_id` field in the (constructed) request cannot
        // strengthen the gate (deny_unknown_fields already rejects it on
        // the wire path). Constructed requests have no such field, so we
        // simply verify the ambient-only check admits the authorized call.
        let res = with_ambient(Some("operator"), true, Some("127.0.0.1:42"), async {
            execute_capability_projection_diagnostics(DiagnosticRequest::Status {}, ctrl.clone())
                .await
        })
        .await;
        assert!(
            matches!(res, Ok(DiagnosticResponse::Status(_))),
            "ambient-only authorization must admit: {res:?}"
        );

        // State unchanged by the unauthorized attempts (the admitted call
        // was a read-only status).
        let status_after = ctrl.status().expect("status after");
        assert_eq!(status_before.lifecycle, status_after.lifecycle);
        assert_eq!(
            status_before.applied_tool_ids.len(),
            status_after.applied_tool_ids.len()
        );
        assert!(!tree.is_disposed(LifetimeScope::Runtime));
    }

    // ---- TDD test 3: bounded schema ----

    /// The seven operations are pinned; `duration_ms` is `1..=5000`; `plane`
    /// is the two-member enum; arbitrary capability-system fields are NOT
    /// accepted; unknown operations are NOT accepted.
    #[test]
    fn diagnostic_operations_have_bounded_schema() {
        // (a) all seven fixed operations deserialize.
        let cases: &[(&str, serde_json::Value)] = &[
            ("status", json!({"operation": "status"})),
            ("bump_runtime", json!({"operation": "bump_runtime"})),
            (
                "revoke_tool",
                json!({"operation": "revoke_tool", "tool_name": "a"}),
            ),
            ("dispose_runtime", json!({"operation": "dispose_runtime"})),
            (
                "hold",
                json!({"operation": "hold", "plane": "source_intake", "duration_ms": 1000}),
            ),
            ("release", json!({"operation": "release"})),
            ("close", json!({"operation": "close"})),
        ];
        for (label, payload) in cases {
            let req = parse_request(payload.clone())
                .unwrap_or_else(|e| panic!("operation {label} must parse: {e}"));
            let op_name = match &req {
                DiagnosticRequest::Status {} => "status",
                DiagnosticRequest::BumpRuntime {} => "bump_runtime",
                DiagnosticRequest::RevokeTool { .. } => "revoke_tool",
                DiagnosticRequest::DisposeRuntime {} => "dispose_runtime",
                DiagnosticRequest::Hold { .. } => "hold",
                DiagnosticRequest::Release {} => "release",
                DiagnosticRequest::Close {} => "close",
            };
            assert_eq!(op_name, *label, "operation {label} dispatched correctly");
        }

        // (b) boundary values for `duration_ms`: 1 and 5000 succeed, 0 and
        // 5001 fail.
        parse_request(json!({"operation": "hold", "plane": "source_intake", "duration_ms": 1}))
            .expect("duration_ms=1 must parse");
        parse_request(json!({"operation": "hold", "plane": "source_intake", "duration_ms": 5000}))
            .expect("duration_ms=5000 must parse");
        assert!(parse_request(
            json!({"operation": "hold", "plane": "source_intake", "duration_ms": 0})
        )
        .is_err());
        assert!(parse_request(
            json!({"operation": "hold", "plane": "source_intake", "duration_ms": 5001})
        )
        .is_err());

        // (c) `plane` is the two-member enum only.
        for plane in ["source_intake", "delivery"] {
            parse_request(json!({"operation": "hold", "plane": plane, "duration_ms": 100}))
                .unwrap_or_else(|_| panic!("plane {plane} must parse"));
        }
        assert!(parse_request(
            json!({"operation": "hold", "plane": "anywhere", "duration_ms": 100})
        )
        .is_err());

        // (d) unknown operations are rejected.
        for op in ["delete_everything", "bump", "hold_plane", "", "STATUS"] {
            assert!(
                parse_request(json!({"operation": op})).is_err(),
                "unknown operation {op:?} must be rejected"
            );
        }

        // (e) required fields are required.
        assert!(
            parse_request(json!({"operation": "hold", "plane": "source_intake"})).is_err(),
            "hold requires duration_ms"
        );
        assert!(
            parse_request(json!({"operation": "hold", "duration_ms": 100})).is_err(),
            "hold requires plane"
        );
        assert!(
            parse_request(json!({"operation": "revoke_tool"})).is_err(),
            "revoke_tool requires tool_name"
        );

        // (f) arbitrary capability-system fields are rejected (the brief:
        // no arbitrary capability kind/namespace/generation).
        for payload in [
            json!({"operation": "bump_runtime", "kind": "tool"}),
            json!({"operation": "bump_runtime", "namespace": "evil"}),
            json!({"operation": "bump_runtime", "generation": 99}),
            json!({"operation": "revoke_tool", "tool_name": "a", "namespace": "evil", "kind": "tool"}),
            json!({"operation": "close", "elapsed_ms": 5000}),
        ] {
            let repr = payload.to_string();
            assert!(
                parse_request(payload).is_err(),
                "arbitrary fields must be rejected: {repr}"
            );
        }
    }

    // ---- TDD test 4: close timeout is a tool error ----

    /// `close` timeout MUST be serialized as a tool error (`DiagnosticToolError::CloseTimeout`)
    /// and NEVER as a successful `DiagnosticResponse::Close`. The From impl
    /// pins this contract independently of the (test-only) live-close path.
    #[tokio::test]
    async fn close_timeout_is_reported_as_tool_error() {
        // (a) The mapping pins it: a `DiagnosticError::CloseTimeout` from the
        // control becomes a `DiagnosticToolError::CloseTimeout`, NOT a
        // success variant.
        let err: DiagnosticToolError = DiagnosticError::CloseTimeout { elapsed_ms: 5000 }.into();
        assert_eq!(err, DiagnosticToolError::CloseTimeout { elapsed_ms: 5000 });
        assert!(
            !matches!(err, DiagnosticToolError::Denied { .. }),
            "CloseTimeout must not be re-coded as Denied"
        );
        // The success variants do NOT admit a CloseTimeout payload: their
        // fields are unrelated.
        assert!(
            matches!(
                DiagnosticResponse::Close(ShutdownReport {
                    source_joined: false,
                    applier_joined: false,
                    source_failed: false,
                    applier_failed: false,
                }),
                DiagnosticResponse::Close(_)
            ),
            "Close response has no elapsed_ms / timeout slot"
        );

        // (b) Live path: a successful `close` returns the ShutdownReport
        // structure with join flags; the host remains fail-closed.
        let ctrl = empty_control();
        let res = with_ambient(Some("operator"), true, Some("127.0.0.1:55"), async {
            execute_capability_projection_diagnostics(DiagnosticRequest::Close {}, ctrl.clone())
                .await
        })
        .await;
        match res {
            Ok(DiagnosticResponse::Close(report)) => {
                // On an idle host, both workers should join cleanly.
                assert!(report.source_joined, "source worker must join");
                assert!(report.applier_joined, "applier worker must join");
                assert!(!report.source_failed);
                assert!(!report.applier_failed);
            }
            other => panic!("authorized close must return Close(ShutdownReport): {other:?}"),
        }
    }

    /// LIVE close timeout through the authorized tool entry point.
    ///
    /// The REAL default applier is stalled mid-delivery via the host's
    /// test-only applier barrier, so the shared completion boundary cannot
    /// be reached. Virtual time (`start_paused`) lets the production 5000 ms
    /// deadline elapse deterministically without a wall-clock sleep. The
    /// tool must return `Err(CloseTimeout { elapsed_ms: 5000 })` — never a
    /// `Close` success — and the host must stay close-requested /
    /// fail-closed (lifecycle `Closing`, no fabricated completion, mutating
    /// operations refused as `HostClosed`).
    #[tokio::test(start_paused = true)]
    async fn live_close_timeout_is_returned_as_tool_error_and_host_stays_fail_closed() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).expect("register a");
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let ctrl = Arc::new(DiagnosticControl::new(Arc::clone(&host), Arc::clone(&tree)).unwrap());
        let _ = await_status(&ctrl, |s| !s.applied_tool_ids.is_empty()).await;

        // Stall the real applier on the next delivery.
        let entered = host.test_arm_applier_gate();
        tokio::pin!(entered);
        reg.register(desc("b"), fake("b")).expect("register b");
        tokio::time::timeout(Duration::from_secs(10), &mut entered)
            .await
            .expect("real default applier did not enter the barrier");

        let started = tokio::time::Instant::now();
        let res = with_ambient(Some("operator"), true, Some("127.0.0.1:56"), async {
            execute_capability_projection_diagnostics(DiagnosticRequest::Close {}, ctrl.clone())
                .await
        })
        .await;
        let err = match res {
            Err(err) => err,
            Ok(resp) => panic!("close timeout must never be reported as success: {resp:?}"),
        };
        assert_eq!(err, DiagnosticToolError::CloseTimeout { elapsed_ms: 5000 });
        assert!(started.elapsed() <= Duration::from_millis(5001));
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({"error": "close_timeout", "elapsed_ms": 5000})
        );

        // Host stays close-requested / fail-closed.
        assert!(
            host.is_closing(),
            "timeout must leave the host close-requested"
        );
        assert_eq!(
            ctrl.status().expect("status").lifecycle,
            DiagnosticLifecycle::Closing
        );
        assert!(
            !host.test_completion_recorded(),
            "timeout must not fabricate completion"
        );
        let bump = with_ambient(Some("operator"), true, Some("127.0.0.1:56"), async {
            execute_capability_projection_diagnostics(
                DiagnosticRequest::BumpRuntime {},
                ctrl.clone(),
            )
            .await
        })
        .await;
        assert_eq!(bump.unwrap_err(), DiagnosticToolError::HostClosed);
        assert!(!tree.is_disposed(LifetimeScope::Runtime));

        // Release the barrier; the retained joins complete through the same
        // boundary (no leaked workers).
        host.test_release_applier_gate();
        let outcome = Arc::clone(&host).close_and_await().await;
        assert!(outcome.source_joined && outcome.applier_joined);
        assert_eq!(
            ctrl.status().expect("status").lifecycle,
            DiagnosticLifecycle::Closed
        );
    }

    // ---- Stable error wire form ----

    /// `DiagnosticToolError` serializes symmetrically with
    /// `DiagnosticResponse`: internally tagged (`error`), snake_case, with
    /// only the variant's own fields. Pinned per variant so a downstream
    /// registry never hand-copies the mapping.
    #[test]
    fn tool_error_serializes_as_stable_tagged_wire_form() {
        let cases = [
            (
                DiagnosticToolError::Denied {
                    reason: "caller is not loopback".into(),
                },
                json!({"error": "denied", "reason": "caller is not loopback"}),
            ),
            (
                DiagnosticToolError::InvalidHoldDuration,
                json!({"error": "invalid_hold_duration"}),
            ),
            (
                DiagnosticToolError::HostClosed,
                json!({"error": "host_closed"}),
            ),
            (
                DiagnosticToolError::HoldAlreadyActive,
                json!({"error": "hold_already_active"}),
            ),
            (
                DiagnosticToolError::CloseTimeout { elapsed_ms: 5000 },
                json!({"error": "close_timeout", "elapsed_ms": 5000}),
            ),
            (
                DiagnosticToolError::UnknownTool { name: "zz".into() },
                json!({"error": "unknown_tool", "name": "zz"}),
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(serde_json::to_value(&err).unwrap(), expected, "{err:?}");
        }
        // Display semantics are unchanged by the structured variant.
        assert_eq!(
            DiagnosticToolError::Denied { reason: "x".into() }.to_string(),
            "diagnostics denied: x"
        );
    }

    /// Pins the serde fact the request-shape comment relies on: for an
    /// internally tagged enum with `deny_unknown_fields`, an EMPTY STRUCT
    /// variant rejects extra keys, while a UNIT variant accepts and drops
    /// them. That is why every field-less operation is `Name {}`.
    #[test]
    fn empty_struct_variants_reject_extra_keys_but_unit_variants_would_not() {
        #[derive(Debug, Deserialize)]
        #[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
        enum Probe {
            Unit,
            Empty {},
        }
        assert!(
            serde_json::from_value::<Probe>(json!({"operation": "unit", "role": "operator"}))
                .is_ok()
        );
        assert!(
            serde_json::from_value::<Probe>(json!({"operation": "empty", "role": "operator"}))
                .is_err()
        );
        assert!(parse_request(json!({"operation": "release", "role": "operator"})).is_err());
    }

    // ---- Actual connection id must be non-empty ----

    /// An ambient `CALLER_CONN_ID` of `Some("")` is NOT an actual connection
    /// id: it must deny exactly like a missing one, for every operation
    /// (including `status`), with no host/tree side-effect.
    #[tokio::test]
    async fn empty_connection_id_is_denied() {
        let (ctrl, tree) = control_with_tool();
        let _ = await_status(&ctrl, |s| !s.applied_tool_ids.is_empty()).await;
        let before = ctrl.status().expect("status before");

        for req in [
            DiagnosticRequest::Status {},
            DiagnosticRequest::BumpRuntime {},
            DiagnosticRequest::DisposeRuntime {},
            DiagnosticRequest::Close {},
        ] {
            let label = format!("{req:?}");
            let res = with_ambient(Some("operator"), true, Some(""), async {
                execute_capability_projection_diagnostics(req, ctrl.clone()).await
            })
            .await;
            match res {
                Err(err @ DiagnosticToolError::Denied { .. }) => assert!(
                    err.to_string().contains("connection id is empty"),
                    "{label}: denial must name the empty connection id: {err}"
                ),
                other => panic!("{label}: empty connection id admitted: {other:?}"),
            }
        }

        let after = ctrl.status().expect("status after");
        assert_eq!(before.lifecycle, after.lifecycle);
        assert_eq!(
            before.applied_owner_generations,
            after.applied_owner_generations
        );
        assert!(!tree.is_disposed(LifetimeScope::Runtime));
    }

    /// RED (wire): the four observational receipt fields added to the
    /// `status` response MUST appear in the JSON form, in their canonical
    /// snake_case wire names, with `last_replacement_tool_ids` sorted for
    /// wire stability.
    #[test]
    fn status_report_carries_four_applied_receipt_wire_fields() {
        use crate::capability::facade::Cursor;
        use std::collections::HashMap;

        let status = DiagnosticStatus {
            lifecycle: DiagnosticLifecycle::Active,
            registry_cursor: Cursor(42),
            pending_depth: 0,
            pending_capacity: 64,
            replacement_count: 0,
            lag_count: 0,
            applied_tool_ids: Vec::new(),
            applied_owner_generations: HashMap::new(),
            applied_invalidation_count: 3,
            applied_replacement_count: 2,
            last_replacement_registry_cursor: Some(Cursor(99)),
            last_replacement_tool_ids: vec!["b".to_string(), "a".to_string()],
        };
        let report: DiagnosticStatusReport = status.into();
        let value = serde_json::to_value(&report).expect("status serialises");

        assert_eq!(value["applied_invalidation_count"], json!(3));
        assert_eq!(value["applied_replacement_count"], json!(2));
        assert_eq!(value["last_replacement_registry_cursor"], json!(99));
        assert_eq!(
            value["last_replacement_tool_ids"],
            json!(["a", "b"]),
            "tool ids must be sorted on the wire"
        );
    }
}
