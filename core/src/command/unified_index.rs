//! Unified Command Index
//!
//! Aggregates trigger keywords from all command sources (Skills, MCP, Custom)
//! for natural language command detection.

use crate::command::CommandTriggers;
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

    /// Add a command with its triggers to the index
    pub fn add_command(
        &mut self,
        source_type: ToolSourceType,
        command_name: &str,
        triggers: &CommandTriggers,
    ) {
        // Add manual triggers with weight 1.0
        for trigger in &triggers.manual {
            let key = trigger.to_lowercase();
            self.entries
                .entry(key)
                .or_default()
                .push(IndexEntry::new(source_type, command_name, 1.0));
        }

        // Add auto-extracted triggers with weight 0.6
        for trigger in &triggers.auto_extracted {
            let key = trigger.to_lowercase();
            self.entries
                .entry(key)
                .or_default()
                .push(IndexEntry::new(source_type, command_name, 0.6));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandTriggers;

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

    #[test]
    fn test_add_command_with_triggers() {
        let mut index = UnifiedCommandIndex::new();
        let triggers = CommandTriggers::new(
            vec!["知识图谱".to_string(), "graph".to_string()],
            vec!["dependencies".to_string()],
        );

        index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers);

        assert!(!index.is_empty());
        assert_eq!(index.len(), 3); // 3 unique triggers
    }
}
