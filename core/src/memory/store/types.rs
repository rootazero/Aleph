//! Common types used by the storage traits.
//!
//! Provides filter, scoring, and query types shared across
//! MemoryStore, GraphStore, and SessionStore implementations.

use crate::memory::context::{FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryScope, MemoryTier};
use crate::memory::namespace::NamespaceScope;
use crate::gateway::workspace::WorkspaceFilter;

// ---------------------------------------------------------------------------
// SQL String Escaping — prevent injection in LanceDB/DataFusion filter strings
// ---------------------------------------------------------------------------

/// Escape a string value for safe interpolation into a DataFusion SQL filter.
///
/// Replaces single quotes with doubled single quotes (`'` → `''`), which is
/// the standard SQL escaping mechanism. This prevents injection attacks when
/// building filter expressions like `column = '{value}'`.
///
/// # Examples
/// ```rust,ignore
/// let safe = escape_sql_string("O'Brien");
/// assert_eq!(safe, "O''Brien");
/// format!("name = '{}'", safe); // "name = 'O''Brien'"
/// ```
pub fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

// ---------------------------------------------------------------------------
// SearchFilter — filter for memory fact searches
// ---------------------------------------------------------------------------

/// Filter criteria for searching memory facts.
///
/// All fields are optional; `None` means "no constraint on this field".
/// Use the builder methods for ergonomic construction:
///
/// ```rust,ignore
/// let filter = SearchFilter::new()
///     .with_valid_only()
///     .with_namespace(NamespaceScope::Owner)
///     .with_fact_type(FactType::Preference);
/// ```
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    /// Restrict to a specific namespace scope.
    pub namespace: Option<NamespaceScope>,
    /// Restrict to a specific workspace.
    pub workspace: Option<WorkspaceFilter>,
    /// Restrict to a specific fact type.
    pub fact_type: Option<FactType>,
    /// Restrict to a specific memory layer.
    pub layer: Option<MemoryLayer>,
    /// Restrict to a specific memory category.
    pub category: Option<MemoryCategory>,
    /// Filter by validity flag (`true` = only valid, `false` = only invalid).
    pub is_valid: Option<bool>,
    /// Restrict to facts whose `path` starts with this prefix.
    pub path_prefix: Option<String>,
    /// Minimum confidence score (inclusive).
    pub min_confidence: Option<f32>,
    /// Only facts created at or after this Unix timestamp (seconds).
    pub created_after: Option<i64>,
    /// Only facts created at or before this Unix timestamp (seconds).
    pub created_before: Option<i64>,
    /// Restrict to a specific cognitive memory tier.
    pub tier: Option<MemoryTier>,
    /// Restrict to a specific visibility scope.
    pub scope: Option<MemoryScope>,
    /// Restrict to a specific persona identifier.
    pub persona_id: Option<String>,
    /// Pre-built scope stack OR clause (set by `with_scope_stack`).
    scope_stack_clause: Option<String>,
}

impl SearchFilter {
    /// Create an empty filter (no constraints).
    pub fn new() -> Self {
        Self::default()
    }

    /// Shortcut: only valid facts with optional namespace.
    pub fn valid_only(namespace: Option<NamespaceScope>) -> Self {
        Self {
            namespace,
            workspace: None,
            is_valid: Some(true),
            ..Default::default()
        }
    }

    // -- builder methods ---------------------------------------------------

    /// Set namespace scope.
    pub fn with_namespace(mut self, ns: NamespaceScope) -> Self {
        self.namespace = Some(ns);
        self
    }

    /// Set workspace filter.
    pub fn with_workspace(mut self, ws: WorkspaceFilter) -> Self {
        self.workspace = Some(ws);
        self
    }

    /// Set fact type filter.
    pub fn with_fact_type(mut self, ft: FactType) -> Self {
        self.fact_type = Some(ft);
        self
    }

    /// Set memory layer filter.
    pub fn with_layer(mut self, layer: MemoryLayer) -> Self {
        self.layer = Some(layer);
        self
    }

    /// Set memory category filter.
    pub fn with_category(mut self, category: MemoryCategory) -> Self {
        self.category = Some(category);
        self
    }

    /// Restrict to valid facts only.
    pub fn with_valid_only(mut self) -> Self {
        self.is_valid = Some(true);
        self
    }

    /// Restrict to facts whose path starts with `prefix`.
    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path_prefix = Some(prefix.into());
        self
    }

    /// Set minimum confidence threshold.
    pub fn with_min_confidence(mut self, min: f32) -> Self {
        self.min_confidence = Some(min);
        self
    }

    /// Set created-after timestamp.
    pub fn with_created_after(mut self, ts: i64) -> Self {
        self.created_after = Some(ts);
        self
    }

    /// Set created-before timestamp.
    pub fn with_created_before(mut self, ts: i64) -> Self {
        self.created_before = Some(ts);
        self
    }

    /// Set cognitive memory tier filter.
    pub fn with_tier(mut self, tier: MemoryTier) -> Self {
        self.tier = Some(tier);
        self
    }

    /// Set visibility scope filter.
    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Set persona identifier filter.
    pub fn with_persona_id(mut self, id: &str) -> Self {
        self.persona_id = Some(id.to_string());
        self
    }

    /// Build scope-stack filter: Global OR (Workspace=W) OR (Persona=P).
    ///
    /// This generates an OR-clause that retrieves facts visible from the
    /// given scope stack. Overrides individual `scope` / `persona_id` filters.
    pub fn with_scope_stack(mut self, persona_id: Option<&str>, workspace: &str) -> Self {
        let ws_safe = escape_sql_string(workspace);
        let mut parts = vec![
            "scope = 'global'".to_string(),
            format!("(scope = 'workspace' AND workspace = '{ws_safe}')"),
        ];
        if let Some(pid) = persona_id {
            let pid_safe = escape_sql_string(pid);
            parts.push(format!("(scope = 'persona' AND persona_id = '{pid_safe}')"));
        }
        self.scope_stack_clause = Some(format!("({})", parts.join(" OR ")));
        self
    }

    // -- filter expression -------------------------------------------------

    /// Build a LanceDB (DataFusion SQL) filter expression.
    ///
    /// Returns `None` when no constraints are set, meaning "match everything".
    /// String values use single quotes as required by DataFusion.
    pub fn to_lance_filter(&self) -> Option<String> {
        let mut clauses: Vec<String> = Vec::new();

        if let Some(ref ns) = self.namespace {
            let val = ns.to_namespace_value();
            clauses.push(format!("namespace = '{}'", escape_sql_string(&val)));
        }

        if let Some(ref ws) = self.workspace {
            match ws {
                WorkspaceFilter::All => {} // no filter needed
                _ => clauses.push(ws.to_sql_filter()),
            }
        }

        if let Some(ref ft) = self.fact_type {
            clauses.push(format!("fact_type = '{}'", ft.as_str()));
        }

        if let Some(layer) = self.layer {
            clauses.push(format!("layer = '{}'", layer.as_str()));
        }

        if let Some(category) = self.category {
            clauses.push(format!("category = '{}'", category.as_str()));
        }

        if let Some(valid) = self.is_valid {
            clauses.push(format!("is_valid = {}", valid));
        }

        if let Some(ref prefix) = self.path_prefix {
            // DataFusion supports the `starts_with` function.
            clauses.push(format!("starts_with(path, '{}')", escape_sql_string(prefix)));
        }

        if let Some(min_conf) = self.min_confidence {
            clauses.push(format!("confidence >= {}", min_conf));
        }

        if let Some(ts) = self.created_after {
            clauses.push(format!("created_at >= {}", ts));
        }

        if let Some(ts) = self.created_before {
            clauses.push(format!("created_at <= {}", ts));
        }

        // If scope_stack_clause is set, use it (overrides individual scope/persona)
        if let Some(ref clause) = self.scope_stack_clause {
            clauses.push(clause.clone());
        } else {
            if let Some(ref scope) = self.scope {
                clauses.push(format!("scope = '{}'", scope.as_str()));
            }
            if let Some(ref persona_id) = self.persona_id {
                clauses.push(format!("persona_id = '{}'", escape_sql_string(persona_id)));
            }
        }

        // tier filter always applies (independent of scope stack)
        if let Some(ref tier) = self.tier {
            clauses.push(format!("tier = '{}'", tier.as_str()));
        }

        if clauses.is_empty() {
            None
        } else {
            Some(clauses.join(" AND "))
        }
    }
}

// ---------------------------------------------------------------------------
// ScoredFact — a fact with its relevance score
// ---------------------------------------------------------------------------

/// A memory fact paired with its relevance score from a search operation.
///
/// The `score` is typically a cosine-similarity or reranker score in `[0, 1]`.
#[derive(Debug, Clone)]
pub struct ScoredFact {
    /// The memory fact.
    pub fact: MemoryFact,
    /// Relevance score (higher is more relevant).
    pub score: f32,
}

// ---------------------------------------------------------------------------
// MemoryFilter — filter for raw memory (Layer 1) searches
// ---------------------------------------------------------------------------

/// Filter criteria for raw memory log searches (Layer 1).
///
/// Used to restrict searches by context anchor fields and time range.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    /// Filter by application bundle ID.
    pub app_bundle_id: Option<String>,
    /// Filter by window title.
    pub window_title: Option<String>,
    /// Restrict to a specific namespace scope.
    pub namespace: Option<NamespaceScope>,
    /// Restrict to a specific workspace.
    pub workspace: Option<WorkspaceFilter>,
    /// Only memories created at or after this Unix timestamp (seconds).
    pub after_timestamp: Option<i64>,
}

impl MemoryFilter {
    /// Create an empty filter (no constraints).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a filter scoped to a specific application context.
    pub fn for_context(app_bundle_id: impl Into<String>, window_title: impl Into<String>) -> Self {
        Self {
            app_bundle_id: Some(app_bundle_id.into()),
            window_title: Some(window_title.into()),
            workspace: None,
            ..Default::default()
        }
    }

    /// Build a LanceDB (DataFusion SQL) filter expression.
    ///
    /// Returns `None` when no constraints are set.
    pub fn to_lance_filter(&self) -> Option<String> {
        let mut clauses: Vec<String> = Vec::new();

        if let Some(ref app_id) = self.app_bundle_id {
            clauses.push(format!("app_bundle_id = '{}'", escape_sql_string(app_id)));
        }

        if let Some(ref title) = self.window_title {
            clauses.push(format!("window_title = '{}'", escape_sql_string(title)));
        }

        if let Some(ref ns) = self.namespace {
            let val = ns.to_namespace_value();
            clauses.push(format!("namespace = '{}'", escape_sql_string(&val)));
        }

        if let Some(ref ws) = self.workspace {
            match ws {
                WorkspaceFilter::All => {} // no filter needed
                _ => clauses.push(ws.to_sql_filter()),
            }
        }

        if let Some(ts) = self.after_timestamp {
            clauses.push(format!("created_at >= {}", ts));
        }

        if clauses.is_empty() {
            None
        } else {
            Some(clauses.join(" AND "))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_filter_empty_produces_none() {
        let f = SearchFilter::new();
        assert!(f.to_lance_filter().is_none());
    }

    #[test]
    fn search_filter_valid_only_shortcut() {
        let f = SearchFilter::valid_only(None);
        assert_eq!(f.to_lance_filter().unwrap(), "is_valid = true");
    }

    #[test]
    fn search_filter_builder_chain() {
        let f = SearchFilter::new()
            .with_valid_only()
            .with_namespace(NamespaceScope::Owner)
            .with_fact_type(FactType::Preference)
            .with_path_prefix("aleph://user/")
            .with_min_confidence(0.8);

        let expr = f.to_lance_filter().unwrap();
        assert!(expr.contains("namespace = 'owner'"));
        assert!(expr.contains("fact_type = 'preference'"));
        assert!(expr.contains("is_valid = true"));
        assert!(expr.contains("starts_with(path, 'aleph://user/')"));
        assert!(expr.contains("confidence >= 0.8"));
    }

    #[test]
    fn search_filter_time_range() {
        let f = SearchFilter::new()
            .with_created_after(1000)
            .with_created_before(2000);

        let expr = f.to_lance_filter().unwrap();
        assert!(expr.contains("created_at >= 1000"));
        assert!(expr.contains("created_at <= 2000"));
    }

    #[test]
    fn memory_filter_empty_produces_none() {
        let f = MemoryFilter::new();
        assert!(f.to_lance_filter().is_none());
    }

    #[test]
    fn memory_filter_for_context() {
        let f = MemoryFilter::for_context("com.example.app", "My Window");
        let expr = f.to_lance_filter().unwrap();
        assert!(expr.contains("app_bundle_id = 'com.example.app'"));
        assert!(expr.contains("window_title = 'My Window'"));
    }

    #[test]
    fn memory_filter_with_namespace_and_time() {
        let f = MemoryFilter {
            namespace: Some(NamespaceScope::Shared),
            after_timestamp: Some(1700000000),
            ..Default::default()
        };
        let expr = f.to_lance_filter().unwrap();
        assert!(expr.contains("namespace = 'shared'"));
        assert!(expr.contains("created_at >= 1700000000"));
    }

    #[test]
    fn search_filter_workspace_single() {
        let f = SearchFilter::new()
            .with_workspace(WorkspaceFilter::Single("crypto".into()));
        let sql = f.to_lance_filter().unwrap();
        assert_eq!(sql, "workspace = 'crypto'");
    }

    #[test]
    fn search_filter_workspace_multiple() {
        let f = SearchFilter::new()
            .with_workspace(WorkspaceFilter::Multiple(vec!["a".into(), "b".into()]));
        let sql = f.to_lance_filter().unwrap();
        assert_eq!(sql, "workspace IN ('a', 'b')");
    }

    #[test]
    fn search_filter_workspace_all_no_filter() {
        let f = SearchFilter::new()
            .with_workspace(WorkspaceFilter::All);
        // All means no workspace filtering, so no SQL generated
        assert!(f.to_lance_filter().is_none());
    }

    #[test]
    fn search_filter_combined_namespace_workspace() {
        let f = SearchFilter::new()
            .with_namespace(NamespaceScope::Owner)
            .with_workspace(WorkspaceFilter::Single("crypto".into()))
            .with_valid_only();
        let sql = f.to_lance_filter().unwrap();
        assert!(sql.contains("workspace = 'crypto'"));
        assert!(sql.contains("namespace = 'owner'"));
        assert!(sql.contains("is_valid = true"));
    }

    #[test]
    fn search_filter_supports_layer_and_category() {
        let filter = SearchFilter::new()
            .with_layer(MemoryLayer::L0Abstract)
            .with_category(MemoryCategory::Preferences);

        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("layer = 'l0_abstract'"));
        assert!(sql.contains("category = 'preferences'"));
    }

    #[test]
    fn memory_filter_with_workspace() {
        let f = MemoryFilter {
            workspace: Some(WorkspaceFilter::Single("novel".into())),
            ..Default::default()
        };
        let sql = f.to_lance_filter().unwrap();
        assert_eq!(sql, "workspace = 'novel'");
    }

    #[test]
    fn search_filter_supports_tier() {
        let filter = SearchFilter::new().with_tier(MemoryTier::Core);
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("tier = 'core'"));
    }

    #[test]
    fn search_filter_supports_scope_and_persona() {
        let filter = SearchFilter::new()
            .with_scope(MemoryScope::Persona)
            .with_persona_id("reviewer");
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("scope = 'persona'"));
        assert!(sql.contains("persona_id = 'reviewer'"));
    }

    #[test]
    fn search_filter_scope_stack_generates_or_clause() {
        let filter = SearchFilter::new()
            .with_scope_stack(Some("reviewer"), "aleph");
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("scope = 'global'"));
        assert!(sql.contains("scope = 'workspace'"));
        assert!(sql.contains("scope = 'persona'"));
        assert!(sql.contains("persona_id = 'reviewer'"));
    }

    #[test]
    fn search_filter_scope_stack_without_persona() {
        let filter = SearchFilter::new()
            .with_scope_stack(None, "aleph");
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("scope = 'global'"));
        assert!(sql.contains("scope = 'workspace'"));
        assert!(!sql.contains("persona"));
    }

    #[test]
    fn search_filter_tier_with_scope_stack() {
        let filter = SearchFilter::new()
            .with_tier(MemoryTier::Core)
            .with_scope_stack(Some("reviewer"), "aleph");
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("tier = 'core'"));
        assert!(sql.contains("scope = 'global'"));
    }
}
