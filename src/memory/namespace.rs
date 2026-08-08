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
    fn test_namespace_value_conversion() {
        assert_eq!(NamespaceScope::Owner.to_namespace_value(), "owner");
        assert_eq!(
            NamespaceScope::Guest("xyz".to_string()).to_namespace_value(),
            "guest:xyz"
        );
        assert_eq!(NamespaceScope::Shared.to_namespace_value(), "shared");
    }
}
