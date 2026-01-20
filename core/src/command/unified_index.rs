//! Unified Command Index
//!
//! Aggregates trigger keywords from all command sources (Skills, MCP, Custom)
//! for natural language command detection.

use crate::dispatcher::ToolSourceType;
use std::collections::HashMap;

/// An entry in the unified command index
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    /// Command source type
    pub source_type: ToolSourceType,
    /// Command name (ID)
    pub command_name: String,
    /// Weight (1.0 for manual triggers, 0.6 for auto-extracted)
    pub weight: f64,
}

impl IndexEntry {
    /// Create a new index entry
    pub fn new(source_type: ToolSourceType, command_name: impl Into<String>, weight: f64) -> Self {
        Self {
            source_type,
            command_name: command_name.into(),
            weight,
        }
    }
}

/// Unified command index for natural language detection
#[derive(Debug, Default)]
pub struct UnifiedCommandIndex {
    /// Map from trigger keyword (lowercase) to matching entries
    entries: HashMap<String, Vec<IndexEntry>>,
}

impl UnifiedCommandIndex {
    /// Create a new empty index
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the index is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get number of unique trigger keywords
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_entry_creation() {
        let entry = IndexEntry::new(
            ToolSourceType::Skill,
            "knowledge-graph",
            1.0,
        );
        assert_eq!(entry.source_type, ToolSourceType::Skill);
        assert_eq!(entry.command_name, "knowledge-graph");
        assert_eq!(entry.weight, 1.0);
    }

    #[test]
    fn test_unified_index_empty() {
        let index = UnifiedCommandIndex::new();
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }
}
