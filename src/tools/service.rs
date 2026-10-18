//! `ToolService` — consumer-side façade over tool dispatch.
//!
//! See: docs/superpowers/specs/2026-04-18-tool-service-facade-design.md
//!
//! ## P0 Task 2: pure types moved to `aleph-slot-api`
//!
//! The pure types this trait operates on (the [`ToolError`] taxonomy, the
//! [`RefusedBy`] refusal-kind enum, the loop-side [`ToolDefinition`] and
//! its [`ToolDefinitionMetadata`], the wire [`ToolOutput`] /
//! [`ToolOutputMetadata`] / [`ToolImage`] shape, and the
//! [`to_metadata_form`] compatibility projection) live in
//! `aleph-slot-api` as of P0 Task 2 — they have no `tokio`, no
//! `alephcore`, no I/O. The trait itself stays here because it
//! depends on `tokio_util::CancellationToken`, the policy stack, and
//! the recovery-tool machinery, none of which the API crate is
//! allowed to reach. This file ends at the trait; the data it
//! operates on now lives at `aleph_slot_api::tool::*`.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

// Re-exports — keep every existing `crate::tools::service::ToolError` /
// `crate::tools::service::ToolDefinition` / etc. path compiling
// unchanged. The single source of truth for the type is
// `aleph_slot_api::tool::*`; this is a Core-side compatibility alias.
pub use aleph_slot_api::tool::{
    to_metadata_form, RefusedBy, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolImage,
    ToolOutput, ToolOutputMetadata,
};

// `ToolSource` is the descriptor 3-variant (`Builtin` / `Mcp { server_id }` /
// `Extension { plugin_id }`). The metadata 6-variant
// (`crate::tool_metadata::ToolSource` / `aleph_slot_api::metadata::ToolSource`)
// is a distinct type at a different path. The two are kept distinct on
// purpose: the descriptor owns canonical capability identity, the metadata
// owns the projection / panel presentation. Round-tripping the metadata
// form to a descriptor `ToolSource` is the responsibility of
// `aleph_slot_api::metadata::project_metadata_source_to_descriptor`, which
// is a closed six-arm mapping that does NOT default to `Builtin` for
// unmapped arms.
pub use aleph_slot_api::descriptor::ToolSource;

// `ToolErrorKind` (and its classifier) live next to `ToolError` in
// the API crate. Re-exported here for callers that reached it via
// the Core path. Same single source of truth, same wire surface.
pub use aleph_slot_api::error_kind::ToolErrorKind;

#[async_trait]
pub trait ToolService: Send + Sync + 'static {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError>;

    async fn list(&self) -> Vec<ToolDefinition>;

    /// Every tool the model may legitimately CALL — the visible list plus the
    /// deferred tier, which is hidden from the tool array but stays executable.
    ///
    /// Distinct from [`Self::list`] because tool-name repair must be judged
    /// against what is *dispatchable*, not what is *displayed*. Building the
    /// repairer's candidate set from `list()` meant a deferred tool's correct
    /// name missed the Exact tier, leaving the Fuzzy tier free to silently
    /// rewrite it into a different, resident tool — a correct call turned into
    /// the wrong action.
    ///
    /// Defaults to `list()`: for services with no deferred tier the two sets are
    /// identical.
    async fn dispatchable_list(&self) -> Vec<ToolDefinition> {
        self.list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition>;

    /// Return the metadata-form tool schema the LLM expects, as an `Arc`
    /// for O(1) per-turn cloning. Implementations cache internally and
    /// invalidate on their own mutation signal (e.g., registry snapshot
    /// change for `CoreDispatch`, MCP `poll_changes()` for `ScopedToolService`).
    ///
    /// REQUIRED — no default impl. A default returning empty would silently
    /// hide the LLM's tool list on any forgotten override. Test mocks must
    /// also implement, typically returning `Arc::from([])`.
    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]>;

    /// The execution tier this service will actually enforce on the **next**
    /// call, when it knows one.
    ///
    /// Exists so a prompt can state the approval regime without deriving it a
    /// second time. The sub-agent spawner is the caller that forced it: a
    /// spawned child gets no tool service of its own — `subagent_spawner`
    /// wraps the PARENT's `ScopedToolService` (`parent_view_for_children`), so
    /// the tier and the very same `PlanGate` `Arc` are enforced on every call
    /// the child makes. The child's prompt nonetheless said nothing about it,
    /// because the spawner held only an `Arc<dyn ToolService>` and had no way
    /// to ask. Under [`ExecTier::Plan`](crate::config::types::policies::ExecTier::Plan)
    /// — which `subagent` is deliberately reachable from, so a plan for a
    /// large codebase can be researched by delegation — that silence costs the
    /// child its whole iteration budget discovering by refusal what one
    /// sentence would have told it.
    ///
    /// **Asking the enforcer is not a second derivation.** `ScopedToolService`
    /// answers with the identical call its own gate makes
    /// (`effective_exec_tier`), which is why this is a trait method rather
    /// than a tier snapshot threaded through `SpawnRequest`: a snapshot taken
    /// at spawn time would go stale the moment a human approves a plan, while
    /// this reads the live gate.
    ///
    /// `None` = this service has no tier wired (test stubs, pre-boot, the
    /// null service). Callers must render nothing rather than a default —
    /// stating a regime nobody enforces is the expensive half of 判据 §0.
    ///
    /// Every production DECORATOR must forward it. The default is `None`, so
    /// a decorator that forgets degrades to silence rather than to a wrong
    /// answer — safe, but silent, which is why each one has a named test.
    fn enforced_exec_tier(&self) -> Option<crate::config::types::policies::ExecTier> {
        None
    }

    /// The retrieval tools a recovery footer may name this turn — see
    /// [`RecoveryTools`](crate::tools::result_processing::RecoveryTools).
    /// `ScopedToolService` answers from the turn's gates (the same derivation
    /// its own Layer-2 footer uses); the harness's Layer-3 spill asks here.
    ///
    /// The default, `ALL`, is what a caller that cannot see any gate has always
    /// assumed. Every production DECORATOR must forward it, and one that
    /// narrows the callable set must narrow this too; one that forgets
    /// degrades to `ALL`, i.e. a footer that may name a tool the model cannot
    /// call.
    fn recovery_tools(&self) -> crate::tools::result_processing::RecoveryTools {
        crate::tools::result_processing::RecoveryTools::ALL
    }

    /// Resource-scope-aware concurrency claim for the named call, given the
    /// concrete input the LLM emitted. The harness parallel fast path admits a
    /// batch only when [`crate::tools::concurrency::batch_parallelizable`]
    /// returns `true` for the claims of every call — letting disjoint-scope
    /// mutations (e.g. writes to different files) run concurrently while
    /// same-path or whole-world mutations fall back to serial.
    ///
    /// Conservative default: whole-world exclusive. Every production impl
    /// (`ScopedToolService` / `AllowlistToolService` / `McpScopedToolService`)
    /// overrides this with real registry-visible path scopes.
    async fn call_concurrency_claim(
        &self,
        _name: &str,
        _input: &serde_json::Value,
    ) -> aleph_slot_api::concurrency::ConcurrencyClaim {
        aleph_slot_api::concurrency::ConcurrencyClaim::global()
    }

    /// Execute a tool call with an opencode-parity `AbortSignal` token.
    ///
    /// The harness Act phase forks a per-call child token from the run's
    /// cancellation token and threads it here. Production wrappers
    /// (`ScopedToolService`, `McpScopedToolService`, `AllowlistToolService`)
    /// override this to thread `cancel` all the way to
    /// [`crate::tools::runtime::LoopTool::execute`] so individual tools can
    /// `select!` against it or have their futures dropped on cancel
    /// (`kill_on_drop` for subprocesses, reqwest abort for HTTP).
    ///
    /// The default impl discards `cancel` and forwards to [`Self::execute`]
    /// — sufficient for test stubs that don't model cancellation. Anywhere
    /// a real run loop calls a tool, the production wrapper's override
    /// fires the token end-to-end.
    async fn execute_with_cancel(
        &self,
        name: &str,
        input: serde_json::Value,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.execute(name, input).await
    }

    /// Execute with cancellation AND report the post-guardrail
    /// `effective_input` the handler actually ran with (§4.3). The second
    /// tuple element is `Some(input)` ONLY when the call crossed the dispatch
    /// line — every gate passed and the handler was invoked — and `None` when
    /// the call was refused before dispatch (a blocked / guardrailed call must
    /// never produce a replay-eligible marker).
    ///
    /// The default is fail-closed `None`: a tool service that cannot name its
    /// effective input never claims one, so no caller can manufacture a marker
    /// from a pre-sanitise `input`. Production wrappers
    /// (`ScopedToolService`, `AllowlistToolService`, `McpScopedToolService`)
    /// override this to thread the real post-hook value out of the gate chain.
    async fn execute_with_cancel_effective(
        &self,
        name: &str,
        input: serde_json::Value,
        cancel: CancellationToken,
    ) -> (Result<ToolOutput, ToolError>, Option<serde_json::Value>) {
        let result = self.execute_with_cancel(name, input, cancel).await;
        (result, None)
    }
}

// =============================================================================
// Smoke tests — the substantive tests for the migrated types live in
// `aleph-slot-api::tool`, `aleph-slot-api::descriptor`, and
// `aleph-slot-api::error_kind`. These are the identity / re-export
// witnesses: they pin that the Core-side path resolves to the SAME type
// the API side defines, so any consumer that keeps importing through
// `crate::tools::service::ToolError` (or any other migrated path) still
// gets the same type-identity guarantee the rest of the crate relies on.
// =============================================================================

#[cfg(test)]
mod re_export_witness_tests {
    use super::*;
    use std::any::TypeId;

    fn assert_same_type<T: 'static, U: 'static>() {
        assert_eq!(
            TypeId::of::<T>(),
            TypeId::of::<U>(),
            "re-export identity must match the API source"
        );
    }

    #[test]
    fn tool_error_is_same_type_as_api_tool_error() {
        assert_same_type::<ToolError, aleph_slot_api::tool::ToolError>();
    }

    #[test]
    fn refused_by_is_same_type_as_api_refused_by() {
        assert_same_type::<RefusedBy, aleph_slot_api::tool::RefusedBy>();
    }

    #[test]
    fn tool_definition_is_same_type_as_api_tool_definition() {
        assert_same_type::<ToolDefinition, aleph_slot_api::tool::ToolDefinition>();
    }

    #[test]
    fn tool_definition_metadata_is_same_type_as_api_tool_definition_metadata() {
        assert_same_type::<ToolDefinitionMetadata, aleph_slot_api::tool::ToolDefinitionMetadata>();
    }

    #[test]
    fn tool_output_is_same_type_as_api_tool_output() {
        assert_same_type::<ToolOutput, aleph_slot_api::tool::ToolOutput>();
    }

    #[test]
    fn tool_output_metadata_is_same_type_as_api_tool_output_metadata() {
        assert_same_type::<ToolOutputMetadata, aleph_slot_api::tool::ToolOutputMetadata>();
    }

    #[test]
    fn tool_image_is_same_type_as_api_tool_image() {
        assert_same_type::<ToolImage, aleph_slot_api::tool::ToolImage>();
    }

    #[test]
    fn tool_source_is_same_type_as_api_descriptor_tool_source() {
        // The Core-side `crate::tools::service::ToolSource` is the
        // descriptor 3-variant, NOT the metadata 6-variant. The
        // single source of truth is `aleph_slot_api::descriptor::ToolSource`;
        // `aleph_slot_api::tool::ToolSource` is itself a re-export of
        // it. The two paths and the metadata path all resolve to
        // distinct types.
        assert_same_type::<ToolSource, aleph_slot_api::descriptor::ToolSource>();
        assert_same_type::<ToolSource, aleph_slot_api::tool::ToolSource>();
        assert!(
            TypeId::of::<ToolSource>() != TypeId::of::<aleph_slot_api::metadata::ToolSource>(),
            "descriptor ToolSource and metadata ToolSource must be distinct types"
        );
    }

    #[test]
    fn tool_error_kind_is_same_type_as_api_tool_error_kind() {
        assert_same_type::<ToolErrorKind, aleph_slot_api::error_kind::ToolErrorKind>();
    }
}
