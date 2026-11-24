//! Public traits the LLM-facing scoped service exposes to extensions / hosts.

use crate::tools::service::ToolDefinition;

/// Current constraint facts supplied by the trusted invocation owner, not tool input.
///
/// Called once per canonical invocation with an installed activation controller.
/// Counts exclude the invocation being admitted and include other occupied slots;
/// an already-Active controller is lifecycle state, not a reserved invocation slot.
/// Thus activation and invocation both require usage strictly below the limit
/// before consuming one new slot. The owner is responsible for accounting and
/// serialization/reservations; this read contract does not create an atomic budget.
/// Named conditions and time must come from the owner's current trusted state.
/// Return an error when that state cannot be read; never substitute dummy facts.
/// This code-installed source is not an RPC field or a new authorization authority.
pub type CapabilityConstraintContextSource = dyn Fn(
        &str,
    ) -> Result<
        crate::capability::constraints::ConstraintContext,
        crate::capability::constraints::ConstraintViolation,
    > + Send
    + Sync;

// =============================================================================
// ToolDefinitionRewriter trait
// =============================================================================

/// Request-time rewriter for tool definitions.
///
/// Mirrors opencode's `plugin.trigger("tool.definition", ...)` extension
/// point: implementations may rewrite a tool's `description` and/or
/// `input_schema` before the catalog is published to the LLM, without
/// touching the underlying registry entry. Common uses:
///
/// - Inject agent-specific hints into tool descriptions
/// - Strip or restrict schema fields based on capability flags
/// - Append usage examples for tools that lack them
///
/// Rewriters run in attachment order on a per-tool basis. They are
/// invoked from `list()` and (on cache miss) from `metadata_schema()`;
/// the rewritten definitions are what the LLM sees.
///
/// Implementations should be deterministic per `(tool name, scope)` —
/// the schema cache is keyed on a generation counter and assumes
/// rewriter output does not change between cache hits. Bump the
/// generation explicitly (via [`super::ScopedToolService::bump_cache_generation`])
/// when external state that a rewriter reads has changed.
pub trait ToolDefinitionRewriter: Send + Sync {
    /// Mutate the tool definition in place. Called once per tool per
    /// `list()` / `metadata_schema()` build. The implementation MUST NOT
    /// rename the tool — `def.name` should be left untouched, since the
    /// dispatch path resolves the underlying handler by that name.
    fn rewrite(&self, def: &mut ToolDefinition);
}
