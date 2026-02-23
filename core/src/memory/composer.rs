//! Context composition: assembles memory context at session start
//! by layered union of Core + Global + Workspace + Persona facts.

use crate::memory::context::{MemoryFact, MemoryTier};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::SearchFilter;

/// Request for context composition
pub struct CompositionRequest {
    /// Current persona ID (None if no persona active)
    pub persona_id: Option<String>,
    /// Current workspace ID
    pub workspace: String,
    /// Current namespace (user identity)
    pub namespace: String,
    /// Token budget for context injection
    pub token_budget: usize,
}

/// Assembled context ready for prompt injection
pub struct ComposedContext {
    /// Core facts: always injected into system prompt
    pub core_facts: Vec<MemoryFact>,
    /// Relevant facts: ranked by relevance, for <relevant_memories> tag
    pub relevant_facts: Vec<MemoryFact>,
    /// Total tokens consumed
    pub total_tokens: usize,
}

/// Builds SearchFilters for layered memory retrieval.
///
/// `ContextComposer` is a stateless utility that constructs the correct
/// LanceDB filters for Core Memory loading and non-Core retrieval,
/// respecting the scope stack (Global -> Workspace -> Persona).
/// The actual async retrieval will be wired in a later phase.
pub struct ContextComposer;

impl ContextComposer {
    /// Build filter for Core Memory retrieval.
    ///
    /// Matches: tier=Core AND (scope=Global OR scope=Persona(P))
    /// AND namespace=owner AND is_valid=true.
    ///
    /// Core facts are identity-level knowledge that should always be
    /// loaded into the system prompt regardless of workspace context.
    pub fn build_core_filter(req: &CompositionRequest) -> SearchFilter {
        SearchFilter::new()
            .with_tier(MemoryTier::Core)
            .with_scope_stack(req.persona_id.as_deref(), &req.workspace)
            .with_namespace(NamespaceScope::Owner)
            .with_valid_only()
    }

    /// Build filter for non-Core retrieval (STM + LTM).
    ///
    /// Matches: scope_stack(Global, Workspace=W, Persona=P)
    /// AND namespace=owner AND is_valid=true.
    ///
    /// Does NOT filter by tier -- returns both ShortTerm and LongTerm
    /// facts so the caller can rank them by relevance.
    pub fn build_retrieval_filter(req: &CompositionRequest) -> SearchFilter {
        SearchFilter::new()
            .with_scope_stack(req.persona_id.as_deref(), &req.workspace)
            .with_namespace(NamespaceScope::Owner)
            .with_valid_only()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request(persona: Option<&str>) -> CompositionRequest {
        CompositionRequest {
            persona_id: persona.map(|s| s.to_string()),
            workspace: "aleph".to_string(),
            namespace: "owner".to_string(),
            token_budget: 2000,
        }
    }

    #[test]
    fn test_build_core_filter_with_persona() {
        let req = make_request(Some("reviewer"));
        let filter = ContextComposer::build_core_filter(&req);
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("tier = 'core'"), "Should filter by Core tier, got: {sql}");
        assert!(sql.contains("scope = 'global'"), "Should include Global scope, got: {sql}");
        assert!(sql.contains("scope = 'persona'"), "Should include Persona scope, got: {sql}");
        assert!(sql.contains("persona_id = 'reviewer'"), "Should filter by persona, got: {sql}");
        assert!(sql.contains("is_valid = true"), "Should only return valid facts, got: {sql}");
    }

    #[test]
    fn test_build_core_filter_without_persona() {
        let req = make_request(None);
        let filter = ContextComposer::build_core_filter(&req);
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("tier = 'core'"), "Should filter by Core tier, got: {sql}");
        assert!(sql.contains("scope = 'global'"), "Should include Global, got: {sql}");
        assert!(!sql.contains("persona"), "Should NOT include persona without persona_id, got: {sql}");
    }

    #[test]
    fn test_build_retrieval_filter_no_core_tier() {
        let req = make_request(Some("reviewer"));
        let filter = ContextComposer::build_retrieval_filter(&req);
        let sql = filter.to_lance_filter().unwrap();
        assert!(!sql.contains("tier = 'core'"), "Retrieval filter should NOT restrict to Core, got: {sql}");
        assert!(sql.contains("scope = 'global'"), "Should include Global, got: {sql}");
        assert!(sql.contains("scope = 'workspace'"), "Should include Workspace, got: {sql}");
        assert!(sql.contains("scope = 'persona'"), "Should include Persona, got: {sql}");
    }

    #[test]
    fn test_build_retrieval_filter_without_persona() {
        let req = make_request(None);
        let filter = ContextComposer::build_retrieval_filter(&req);
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("scope = 'global'"), "Should include Global, got: {sql}");
        assert!(sql.contains("scope = 'workspace'"), "Should include Workspace, got: {sql}");
        assert!(!sql.contains("persona"), "Should NOT include persona, got: {sql}");
    }
}
