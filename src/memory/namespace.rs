//! The `namespace` column tag written on memory rows.
//!
//! # This is NOT the isolation layer — do not build one here
//!
//! This module used to claim it "enforces type-safe data isolation for
//! multi-user scenarios at compile time". It never did. The `Guest`/`Shared`
//! variants had zero production construction points (every caller in the tree
//! builds [`NamespaceScope::Owner`]), and the SQL predicate they fed —
//! `to_sql_filter` — returned the literal `"1=1"` for `Owner`, i.e. a
//! tautology, on the one path that was actually reachable. A reader trusting
//! that doc comment would have built the next feature on top of an isolation
//! layer that isolates nothing.
//!
//! The real sources of truth are elsewhere, and a new isolation dimension
//! belongs in one of them, not here:
//!
//! - **Per-project memory partitioning** → `crate::memory::project_scope`.
//!   Composes the active project into the existing `agent_id` partition key
//!   (`scoped_agent_id`, `GLOBAL_NS`, the two always-on floors).
//! - **Who may see a session / room** → `crate::gateway::visibility`
//!   (`session_visible` / `session_visible_to` / `project_visible`), whose
//!   membership predicate is `crate::projects::roster::is_member`.
//!
//! What survives here is the one thing that was ever real: the string tag
//! written into the `namespace` column (see [`NamespaceScope::to_namespace_value`],
//! consumed by `memory::reflector::recall_signals`).

/// Namespace tag for memory rows.
///
/// Single-variant by design — see the module docs. This is a column value, not
/// an access-control decision. Adding a variant does not create isolation;
/// isolation lives in `memory::project_scope` and `gateway::visibility`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamespaceScope {
    /// Owner namespace — the only namespace Aleph writes.
    Owner,
}

impl NamespaceScope {
    /// Converts namespace scope to database column value
    ///
    /// Used for INSERT/UPDATE operations
    #[must_use]
    pub fn to_namespace_value(&self) -> String {
        match self {
            Self::Owner => "owner".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_value_conversion() {
        assert_eq!(NamespaceScope::Owner.to_namespace_value(), "owner");
    }
}
