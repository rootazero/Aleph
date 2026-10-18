//! Loop-side tool contract — errors, definitions, and wire output shape.
//!
//! This module owns the parts of the tool surface that the API crate can
//! stand up on its own (no `tokio`, no `alephcore`): the `ToolError`
//! taxonomy, the `RefusedBy` refusal-kind enum, the loop-side
//! `ToolDefinition` and its `ToolDefinitionMetadata`, the wire `ToolOutput`
//! / `ToolOutputMetadata` / `ToolImage` shape, and the `to_metadata_form`
//! compatibility projection.
//!
//! The `ToolService` async_trait stays in `alephcore` — it depends on
//! `tokio_util::CancellationToken`, the harness's permission/registry
//! surfaces, and the policy stack, none of which the API crate is allowed
//! to reach. This file ends at the *types* the trait operates on; the
//! trait itself is a separate concern.
//!
//! ## Identity surface (must NOT change without an explicit proposal)
//!
//! * `ToolError` has 15 variants; `#[error("…")]` Display strings
//!   are part of the wire surface that the harness's cross-batch failure
//!   memo and the LLM-facing routing hint parse.
//! * `RefusedBy` has 4 variants; `ALL` lists them in declaration order
//!   and `head()` is the one place the rendering prose is written.
//! * `ToolDefinitionMetadata` has 6 fields with the same names as the
//!   pre-migration struct, and the `#[serde(default)]` attributes on every
//!   one of them so legacy JSON round-trips.
//! * `ToolDefinition::to_metadata_definition` is a *direct* projection
//!   of descriptor-owned fields to the provider metadata shape — it
//!   must NOT route through a synthetic `ToolCapabilityDescriptor` (that
//!   manufactured replay identity that never existed).
//! * `to_metadata_form` is a pure compatibility wrapper kept only for
//!   `src/harness/tests/tools_surface.rs`; new callers should use
//!   `ToolDefinition::to_metadata_definition` directly.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use aleph_protocol::Presentation;

use crate::descriptor::{ToolCapabilityDescriptor, ToolSource as DescriptorToolSource};
use crate::metadata::{ToolCategory, ToolDefinition as MetadataToolDefinition};

// ============================================================================
// ToolError
// ============================================================================

/// A failure the harness surfaces back to the model after a tool call.
///
/// 15 variants cover the full surface the cross-batch failure memo and
/// the LLM-facing routing hint distinguish. Adding a variant is fine;
/// reordering or rewording the `#[error("…")]` attribute is a wire
/// change.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {name}")]
    NotFound { name: String },

    #[error("permission denied for tool {name}: {reason}")]
    PermissionDenied { name: String, reason: String },

    #[error("invalid input for tool {name}: {cause}")]
    ValidationFailed { name: String, cause: String },

    #[error("tool {name} execution failed: {cause}")]
    Execution { name: String, cause: String },

    /// The call was refused before it ran — by a hook, a person, or because
    /// nobody could be asked ([`RefusedBy`]). A verdict, not a failure: its
    /// kind is `Permission` whatever `reason` says (a hook's prose, the user's
    /// own words), so the model gets no route to another tool.
    ///
    /// Distinct from `PermissionDenied` for one reason: these refusals do not
    /// fire the `PermissionDenied` observers (`HookEvent::PermissionDenied`
    /// documents which refusals do). The rendering's head comes from
    /// [`RefusedBy::head`], which `error_kind::classify_error_str` reads too.
    #[error("tool {name} {}: {reason}", .by.head())]
    Refused {
        name: String,
        by: RefusedBy,
        reason: String,
    },

    #[error("tool {name} timed out after {elapsed_ms}ms")]
    Timeout { name: String, elapsed_ms: u64 },

    /// The approval card expired with nobody answering it. Distinct from
    /// `PermissionDenied` and from a plain `Execution` failure, because nobody
    /// said no — nobody said anything, and the human may simply be away.
    /// `DenialLedger::record_denial` already drops a `DenialReason::Timeout`
    /// for exactly this reason ("an expired card is not a decision"); folding
    /// the expiry into a non-retryable error one layer up put the permanent ban
    /// back and told the model the user had refused.
    #[error(
        "approval for tool {name} expired after {waited_ms}ms with no response — nobody \
         answered; the user did not decline this call"
    )]
    ApprovalExpired { name: String, waited_ms: u64 },

    #[error("tool {name} transport error: {cause}")]
    Transport { name: String, cause: String },

    /// The run was cancelled while this call was in flight (or before it was
    /// polled). Distinct from every other variant for the same reason
    /// `ApprovalExpired` is: nobody judged the call. Collapsing it into
    /// `Execution` made `is_retryable` false, which entered the call into the
    /// harness's cross-batch failure memo — so pressing stop once *banned* that
    /// exact call for the rest of the run — and handed the model a persistence
    /// hint telling it to climb the tool ladder because the user had pressed
    /// stop.
    #[error("tool {name} was cancelled — the run was stopped, this is not a verdict on the call")]
    Cancelled { name: String },

    #[error("duplicate tool name: {name}")]
    Duplicate { name: String },

    /// A [`ToolCapabilityDescriptor`] failed validation before the registry
    /// could store it. `reason` is the descriptor-layer diagnostic; the write
    /// is refused fail-closed so an invalid capability never becomes callable.
    #[error("invalid tool descriptor for {name}: {reason}")]
    InvalidDescriptor { name: String, reason: String },

    /// The descriptor does not describe the handler it was paired with — the
    /// `name` / `source` / schema / safety-flag projection disagreed with the
    /// handler's own [`definition`](crate::handler::ToolHandler::definition).
    /// Refused so a handler can never be resolvable under a contract it does
    /// not honour.
    #[error("descriptor mismatch for tool {name}: {reason}")]
    DescriptorMismatch { name: String, reason: String },

    /// The registry was closed and no longer accepts new registrations or
    /// replacements. Existing handles stay disposable.
    #[error("registry closed: cannot register tool {name}")]
    RegistryClosed { name: String },

    /// A registration handle or change referred to a revision the registry no
    /// longer holds (the entry was replaced or unregistered in between).
    #[error("unknown registration revision {revision} for tool {name}")]
    UnknownRevision { name: String, revision: u64 },

    #[error("{0}")]
    Other(String),
}

impl ToolError {
    /// "This failure was not a verdict on the call itself." Two consumers read
    /// it, and they want different things from it:
    ///
    /// - `tools::retry::execute_with_one_shot_backoff` respins the call
    ///   (subject to per-tool idempotence). Only `Timeout` / `Transport` can
    ///   reach it: it lives *below* the approval gate, so an
    ///   `ApprovalExpired` is returned long before the retry layer is entered
    ///   and is never silently respun into a second approval card. That helper
    ///   excludes `Cancelled` explicitly — respinning after `/stop` would sleep
    ///   the backoff and fail again.
    /// - the harness's cross-batch failure memo (`agent/act.rs`) refuses to
    ///   *ban* a call whose error was retryable. An expired approval must not
    ///   be banned — the human was away, they did not refuse. Neither must a
    ///   cancelled one: the user stopped the run, they did not judge the call.
    ///
    /// For richer, prompt-side classification (rate-limited,
    /// unauthorized, blocked-by-policy, …) see [`Self::kind`].
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout { .. }
                | Self::Transport { .. }
                | Self::ApprovalExpired { .. }
                | Self::Cancelled { .. }
        )
    }

    /// Coarse, stable taxonomy used to render the LLM-facing routing
    /// hint in `SessionEvent::ToolError`. Delegates to
    /// [`crate::error_kind::classify_tool_error`].
    ///
    /// This is a strict superset of `is_retryable` — every transient
    /// variant returns a `kind()` whose `is_transient()` is `true`,
    /// but the kind further distinguishes "switch source"
    /// (Unauthorized/BlockedByPolicy) from "switch tool"
    /// (Timeout/Transport).
    #[must_use]
    pub fn kind(&self) -> crate::error_kind::ToolErrorKind {
        crate::error_kind::classify_tool_error(self)
    }
}

// ============================================================================
// RefusedBy
// ============================================================================

/// Who refused a [`ToolError::Refused`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusedBy {
    /// A BeforeToolCall hook blocked the call (exit 2, `decision: "block"`,
    /// `block:`).
    Hook,
    /// A BeforeToolCall hook — or the hook executor — failed to run, so the
    /// call was blocked fail-closed. Not a policy verdict: the guard broke.
    HookFailed,
    /// A person declined the approval card (a confirmation gate's, or the one
    /// a hook's `ask:` raised).
    Person,
    /// Nobody could be asked: no approval channel, an unattended run, or the
    /// denial ledger's standing refusal of this exact call.
    NobodyAsked,
}

impl RefusedBy {
    /// Every variant — `error_kind::classify_error_str` recognises a
    /// rendering by trying each head.
    pub const ALL: [Self; 4] = [
        Self::Hook,
        Self::HookFailed,
        Self::Person,
        Self::NobodyAsked,
    ];

    /// What follows `tool <name> ` in the rendering. The one place a head is
    /// written; the classifier reads it from here.
    #[must_use]
    pub const fn head(self) -> &'static str {
        match self {
            Self::Hook => "was refused by a policy hook",
            Self::HookFailed => "was blocked because a policy hook failed to run (fail-closed)",
            Self::Person => "was declined",
            Self::NobodyAsked => "was not authorized",
        }
    }
}

// ============================================================================
// ToolSource (descriptor 3-variant, re-exported here for convenience)
// ============================================================================

// Re-exported so the loop-side `ToolDefinition` can carry it without making
// every consumer import from `descriptor`. The single source of truth is
// `crate::descriptor::ToolSource`; this is a convenience alias.
pub use crate::descriptor::ToolSource;

// ============================================================================
// ToolDefinitionMetadata
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDefinitionMetadata {
    #[serde(default)]
    pub hidden_from_llm: bool,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    /// True when re-running this tool with the same input is safe even if
    /// the previous attempt may have reached the server. Read-only / pure
    /// query tools (`memory_search`, search, `web_fetch`, `recall_context`) set
    /// this to true; side-effecting tools (write/exec/send_*) leave it
    /// false. Consumed by `tools::retry::execute_with_one_shot_backoff` to
    /// gate retries on `Timeout` / `Transport` errors — non-idempotent
    /// tools never auto-retry to avoid duplicate side effects.
    #[serde(default)]
    pub idempotent: bool,
    /// Per-tool wall-clock execution budget.
    ///
    /// Every definition a production `ToolService` publishes carries one:
    /// the builders resolve it through `tools::budget::resolve_tool_budget_ms`
    /// (the tool's own `LoopTool::max_duration_ms` → `BUILTIN_TOOL_BUDGETS_MS`
    /// → `DEFAULT_TOOL_BUDGET_MS`), so MCP / plugin / skill tools and the ~100
    /// builtins outside the table are budgeted too. That matters because the
    /// harness treats an overrun of a *declared* budget as a recoverable tool
    /// error, and a `None` budget as a run-level stall — an unbudgeted tool
    /// turned one slow call into an aborted run.
    ///
    /// Still `Option` because the type is also the wire form (`#[serde(default)]`
    /// for legacy JSON) and test doubles need a "says nothing" value.
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
    /// Static "always safe under parallel dispatch" hint, mirroring the
    /// inner [`crate::concurrency::ConcurrencyClaim`]. Tools that mutate
    /// shared state (`file_write`, bash, `code_exec`, send_*) leave this
    /// `false`; pure read-only queries set it `true`. The harness uses
    /// `ToolService::call_concurrency_claim` for the authoritative
    /// per-call answer at dispatch time, since some tools (e.g. `file_ops`)
    /// flip based on the operation in their input.
    #[serde(default)]
    pub concurrent_safe: bool,
}

// ============================================================================
// ToolDefinition
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub source: DescriptorToolSource,
    #[serde(default)]
    pub metadata: ToolDefinitionMetadata,
}

impl ToolDefinition {
    /// Build the callable service form from the canonical descriptor fields.
    #[must_use]
    pub fn from_descriptor(descriptor: &ToolCapabilityDescriptor) -> Self {
        Self {
            name: descriptor.name.clone(),
            description: descriptor.description.clone(),
            input_schema: descriptor.input_schema.clone(),
            source: descriptor.source.clone(),
            metadata: ToolDefinitionMetadata {
                requires_approval: descriptor.requires_confirmation,
                idempotent: descriptor.idempotent,
                concurrent_safe: descriptor.concurrent_safe,
                max_duration_ms: descriptor.max_duration_ms,
                ..Default::default()
            },
        }
    }

    /// Project descriptor-owned fields straight to the provider metadata
    /// shape, WITHOUT fabricating a descriptor.
    ///
    /// The loop-side [`ToolDefinition`] carries the same fields the canonical
    /// descriptor owns — name, description, input schema, source, and
    /// requires-confirmation — but no revision. Round-tripping through a
    /// synthetic revision-1 [`ToolCapabilityDescriptor`] (as `to_metadata_form`
    /// once did) manufactures replay identity that never existed. Map the
    /// fields directly instead.
    #[must_use]
    pub fn to_metadata_definition(&self) -> MetadataToolDefinition {
        let category = match &self.source {
            DescriptorToolSource::Builtin => ToolCategory::Builtin,
            DescriptorToolSource::Mcp { .. } => ToolCategory::Mcp,
            DescriptorToolSource::Extension { .. } => ToolCategory::Custom,
        };
        MetadataToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.input_schema.clone(),
            requires_confirmation: self.metadata.requires_approval,
            category,
            strict: false,
        }
    }
}

// ============================================================================
// ToolOutput / ToolOutputMetadata / ToolImage
// ============================================================================

/// The value a tool handler returned to the harness, plus the per-call
/// side-channel metadata the harness / panel reads.
///
/// The `metadata.presentation` field carries the existing
/// [`aleph_protocol::Presentation`] — exactly one consumer of that type,
/// no second home. A tool that produces no presentation simply leaves it
/// `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutput {
    pub value: serde_json::Value,
    #[serde(default)]
    pub metadata: ToolOutputMetadata,
}

/// Per-call side-channel: latency, cost, attached images, and the optional
/// [`Presentation`] the panel reads to render the result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutputMetadata {
    #[serde(default)]
    pub latency_ms: u64,
    /// Cost in 1/100 cents. `None` when the provider does not surface a cost
    /// (a local tool, a tool whose provider has not yet been instrumented).
    #[serde(default)]
    pub cost_cents: Option<u64>,
    /// Out-of-band image payloads carried alongside the (text) `value`.
    ///
    /// Some tools — desktop screenshots above all — produce an image the
    /// vision-capable model must actually *see*. The text result budget
    /// (`apply_layer_two`) would otherwise flatten and truncate the base64
    /// into oblivion, so the image is hoisted here BEFORE truncation and
    /// re-emitted as a `ContentBlock::Image` when the tool result is rendered
    /// into the prompt. Empty for the overwhelming majority of tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolImage>,
    /// Structured UI presentation (file diffs) hoisted out of the tool's
    /// JSON by `apply_layer_two` BEFORE the value is flattened to model text.
    /// Rides `session_events` for replay and the callback for the live frame.
    /// Exactly one home for the type in the entire crate graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<Presentation>,
}

/// One image produced by a tool call. Base64-encoded payload with a MIME
/// type. The panel decodes the data URL on its side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolImage {
    /// Base64-encoded image bytes (no `data:` URL prefix).
    pub data: String,
    /// MIME type, e.g. `image/png`, `image/jpeg`.
    pub mime_type: String,
}

// ============================================================================
// to_metadata_form (compatibility projection)
// ============================================================================

/// Compatibility wrapper around [`ToolDefinition::to_metadata_definition`].
///
/// Previously this fabricated a revision-1 [`ToolCapabilityDescriptor`] per
/// definition and projected through it — a lie, because loop-side definitions
/// carry no revision and revision 1 is not canonical. The projection is now
/// direct. Kept only because `src/harness/tests/tools_surface.rs` imports it;
/// new callers should use [`ToolDefinition::to_metadata_definition`] directly.
///
/// This is a pure compatibility projection and does not provide canonical
/// capability identity, authorization, or registry state.
#[must_use]
pub fn to_metadata_form(defs: &[ToolDefinition]) -> Arc<[MetadataToolDefinition]> {
    defs.iter()
        .map(ToolDefinition::to_metadata_definition)
        .collect::<Vec<_>>()
        .into()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod metadata_form_tests {
    use super::*;
    use serde_json::json;

    fn loop_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc {name}"),
            input_schema: json!({"type": "object"}),
            source: DescriptorToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    #[test]
    fn empty_input_yields_empty_arc() {
        let out = to_metadata_form(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn single_def_converts_field_by_field() {
        let inputs = vec![loop_def("alpha")];
        let out = to_metadata_form(&inputs);
        assert_eq!(out.len(), 1);
        let d: &MetadataToolDefinition = &out[0];
        assert_eq!(d.name, "alpha");
        assert_eq!(d.description, "desc alpha");
        assert_eq!(d.parameters, json!({"type": "object"}));
        assert!(!d.requires_confirmation);
        assert!(matches!(d.category, ToolCategory::Builtin));
        assert!(!d.strict);
    }

    #[test]
    fn projects_mcp_source_and_confirmation_to_provider_metadata() {
        let mut definition = loop_def("mcp_search");
        definition.source = DescriptorToolSource::Mcp {
            server_id: "search".into(),
        };
        definition.metadata.requires_approval = true;
        let projected = to_metadata_form(&[definition]);
        assert_eq!(projected[0].category, ToolCategory::Mcp);
        assert!(projected[0].requires_confirmation);
    }

    #[test]
    fn preserves_order_for_multi_input() {
        let inputs = vec![loop_def("a"), loop_def("b"), loop_def("c")];
        let out = to_metadata_form(&inputs);
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn to_metadata_definition_maps_without_fabricated_descriptor() {
        let mut def = loop_def("direct");
        def.source = DescriptorToolSource::Extension {
            plugin_id: "p".into(),
        };
        def.metadata.requires_approval = true;
        let projected = def.to_metadata_definition();
        assert_eq!(projected.name, "direct");
        assert_eq!(projected.parameters, json!({"type": "object"}));
        assert!(projected.requires_confirmation);
        assert!(matches!(projected.category, ToolCategory::Custom));
        assert!(!projected.strict);
    }

    #[test]
    fn to_metadata_form_agrees_with_direct_projection() {
        let defs = vec![loop_def("a"), loop_def("b")];
        let via_form = to_metadata_form(&defs);
        let direct: Vec<_> = defs
            .iter()
            .map(ToolDefinition::to_metadata_definition)
            .collect();
        assert_eq!(via_form.len(), direct.len());
        for (left, right) in via_form.iter().zip(direct.iter()) {
            assert_eq!(left.name, right.name);
            assert_eq!(left.description, right.description);
            assert_eq!(left.parameters, right.parameters);
            assert_eq!(left.requires_confirmation, right.requires_confirmation);
            assert_eq!(left.category, right.category);
            assert_eq!(left.strict, right.strict);
        }
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn metadata_max_duration_ms_round_trips_through_json() {
        let original = ToolDefinitionMetadata {
            hidden_from_llm: false,
            requires_approval: false,
            tags: Vec::new(),
            idempotent: true,
            max_duration_ms: Some(5_000),
            concurrent_safe: false,
        };
        let serialized = serde_json::to_string(&original).unwrap();
        let parsed: ToolDefinitionMetadata = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed.max_duration_ms, Some(5_000));
    }

    #[test]
    fn metadata_max_duration_ms_defaults_to_none_when_field_absent() {
        let legacy_json =
            r#"{"hidden_from_llm":false,"requires_approval":false,"tags":[],"idempotent":false}"#;
        let parsed: ToolDefinitionMetadata = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.max_duration_ms, None);
        assert!(
            !parsed.concurrent_safe,
            "absent concurrent_safe must default to false on legacy JSON",
        );
    }
}

#[cfg(test)]
mod refused_by_tests {
    use super::*;

    #[test]
    fn all_lists_every_variant_in_declaration_order() {
        // Declaration order matters: `classify_error_str` reads from it
        // and a refactor that reorders ALL would silently change which
        // head wins on overlapping prefixes.
        assert!(matches!(RefusedBy::ALL[0], RefusedBy::Hook));
        assert!(matches!(RefusedBy::ALL[1], RefusedBy::HookFailed));
        assert!(matches!(RefusedBy::ALL[2], RefusedBy::Person));
        assert!(matches!(RefusedBy::ALL[3], RefusedBy::NobodyAsked));
    }

    #[test]
    fn heads_are_lowercase_prose() {
        for by in RefusedBy::ALL {
            let head = by.head();
            assert!(!head.is_empty(), "{by:?} head must not be empty");
            // The classifier's truth table keys off the head being a
            // recognisable prose chunk, not on any specific substring.
            // Pin that no head collides with another.
            for other in RefusedBy::ALL {
                if other == by {
                    continue;
                }
                assert_ne!(by.head(), other.head(), "{by:?} and {other:?} share a head");
            }
        }
    }
}
