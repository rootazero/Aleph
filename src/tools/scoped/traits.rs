//! Public traits the LLM-facing scoped service exposes to extensions / hosts.

use crate::tools::service::ToolDefinition;

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
