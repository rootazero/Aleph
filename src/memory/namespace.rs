//! Namespace-scoped memory access control
//!
//! Provides type-safe namespace isolation for multi-user memory data.
//! Enforces data isolation at compile-time using `NamespaceScope` enum.

/// Namespace scope for memory access control
///
/// Enforces type-safe data isolation for multi-user scenarios.
/// Maps to the `namespace` column in memory tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamespaceScope {
    /// Owner namespace - no filtering (accesses all owner data)
    Owner,
    /// Guest namespace - scoped to specific guest ID
    Guest(String),
    /// Shared namespace - accessible to all authenticated users
    Shared,
}

impl NamespaceScope {
    /// Converts namespace scope to SQL WHERE clause filter
    ///
    /// Returns (`filter_clause`, `bind_params`)
    #[must_use]
    pub fn to_sql_filter(&self) -> (String, Vec<String>) {
        match self {
            Self::Owner => ("1=1".to_string(), vec![]),
            Self::Guest(guest_id) => (
                "namespace = ?".to_string(),
                vec![format!("guest:{}", guest_id)],
            ),
            Self::Shared => ("namespace = ?".to_string(), vec!["shared".to_string()]),
        }
    }

    /// Converts namespace scope to database column value
    ///
    /// Used for INSERT/UPDATE operations
    #[must_use]
    pub fn to_namespace_value(&self) -> String {
        match self {
            Self::Owner => "owner".to_string(),
            Self::Guest(guest_id) => format!("guest:{guest_id}"),
            Self::Shared => "shared".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owner_scope_no_filter() {
        let scope = NamespaceScope::Owner;
        let (filter, params) = scope.to_sql_filter();
        assert_eq!(filter, "1=1");
        assert!(params.is_empty());
    }

    #[test]
    fn test_guest_scope_filters_correctly() {
        let scope = NamespaceScope::Guest("abc-123".to_string());
        let (filter, params) = scope.to_sql_filter();
        assert_eq!(filter, "namespace = ?");
        assert_eq!(params, vec!["guest:abc-123"]);
    }

    #[test]
    fn test_shared_scope_filters_correctly() {
        let scope = NamespaceScope::Shared;
        let (filter, params) = scope.to_sql_filter();
        assert_eq!(filter, "namespace = ?");
        assert_eq!(params, vec!["shared"]);
    }

    #[test]
    fn test_namespace_value_conversion() {
        assert_eq!(NamespaceScope::Owner.to_namespace_value(), "owner");
        assert_eq!(
            NamespaceScope::Guest("xyz".to_string()).to_namespace_value(),
            "guest:xyz"
        );
        assert_eq!(NamespaceScope::Shared.to_namespace_value(), "shared");
    }
}
