//! Unified Command Index
//!
//! Aggregates trigger keywords from all command sources (Skills, MCP, Custom)
//! for natural language command detection.

use crate::command::CommandTriggers;
use crate::dispatcher::ToolSourceType;
use std::collections::HashMap;

/// Stop words to exclude from auto-extraction
const STOP_WORDS: &[&str] = &[
    // English
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
    "to", "for", "and", "or", "but", "in", "on", "at", "by", "with",
    "from", "as", "of", "this", "that", "it", "its", "can", "will",
    // Chinese
    "的", "是", "和", "与", "用", "来", "可以", "进行", "这个", "那个",
    "一个", "在", "了", "有", "不", "也", "就", "都", "而", "及",
];

/// Extract keywords from a description string
pub fn extract_keywords_from_description(description: &str) -> Vec<String> {
    description
        .split(|c: char| {
            c.is_whitespace()
                || c == ','
                || c == '，'
                || c == '。'
                || c == '.'
                || c == ';'
                || c == '；'
                || c == '、'
        })
        .map(|s| s.trim())
        .filter(|w| !w.is_empty())
        .filter(|w| w.chars().count() >= 2) // At least 2 characters
        .filter(|w| !STOP_WORDS.contains(&w.to_lowercase().as_str()))
        .map(|w| w.to_lowercase())
        .collect()
}

/// A scored match result
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredMatch {
    /// Command source type
    pub source_type: ToolSourceType,
    /// Command name
    pub command_name: String,
    /// Match score (higher = better match)
    pub score: f64,
}

impl ScoredMatch {
    /// Get type priority for sorting (lower = higher priority)
    fn type_priority(&self) -> u8 {
        match self.source_type {
            ToolSourceType::Builtin | ToolSourceType::Native => 0,
            ToolSourceType::Skill => 1,
            ToolSourceType::Mcp => 2,
            ToolSourceType::Custom => 3,
        }
    }
}

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

    /// Find commands matching the input text
    /// Returns matches sorted by: type priority (asc), then score (desc)
    pub fn find_matches(&self, input: &str) -> Vec<ScoredMatch> {
        let input_lower = input.to_lowercase();
        let mut scores: HashMap<String, ScoredMatch> = HashMap::new();

        // Check each trigger keyword
        for (trigger, entries) in &self.entries {
            if input_lower.contains(trigger) {
                for entry in entries {
                    let key = format!("{:?}:{}", entry.source_type, entry.command_name);
                    scores
                        .entry(key)
                        .and_modify(|m| m.score += entry.weight)
                        .or_insert_with(|| ScoredMatch {
                            source_type: entry.source_type,
                            command_name: entry.command_name.clone(),
                            score: entry.weight,
                        });
                }
            }
        }

        // Sort by type priority (asc), then score (desc)
        let mut matches: Vec<ScoredMatch> = scores.into_values().collect();
        matches.sort_by(|a, b| {
            a.type_priority()
                .cmp(&b.type_priority())
                .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
        });

        matches
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

    #[test]
    fn test_extract_keywords_english() {
        let keywords = extract_keywords_from_description("Generate knowledge graphs and analyze dependencies");
        assert!(keywords.contains(&"generate".to_string()));
        assert!(keywords.contains(&"knowledge".to_string()));
        assert!(keywords.contains(&"graphs".to_string()));
        assert!(keywords.contains(&"analyze".to_string()));
        assert!(keywords.contains(&"dependencies".to_string()));
        // Should not contain stop words
        assert!(!keywords.contains(&"and".to_string()));
    }

    #[test]
    fn test_extract_keywords_chinese() {
        let keywords = extract_keywords_from_description("生成知识图谱，分析代码依赖关系");
        // Chinese splits on punctuation, so we get segments
        assert!(keywords.iter().any(|k| k.contains("生成")));
        assert!(keywords.iter().any(|k| k.contains("知识图谱")));
        assert!(keywords.iter().any(|k| k.contains("分析")));
    }

    #[test]
    fn test_extract_keywords_filters_stop_words() {
        let keywords = extract_keywords_from_description("the quick brown fox and the lazy dog");
        assert!(!keywords.contains(&"the".to_string()));
        assert!(!keywords.contains(&"and".to_string()));
        assert!(keywords.contains(&"quick".to_string()));
        assert!(keywords.contains(&"brown".to_string()));
    }

    #[test]
    fn test_find_matches_basic() {
        let mut index = UnifiedCommandIndex::new();

        // Add a skill with triggers
        let triggers1 = CommandTriggers::new(
            vec!["知识图谱".to_string(), "graph".to_string()],
            vec!["dependencies".to_string()],
        );
        index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers1);

        // Add another command
        let triggers2 = CommandTriggers::new(
            vec!["翻译".to_string(), "translate".to_string()],
            Vec::new(),
        );
        index.add_command(ToolSourceType::Custom, "translate", &triggers2);

        // Test finding matches
        let matches = index.find_matches("帮我画个知识图谱");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].command_name, "knowledge-graph");

        let matches2 = index.find_matches("translate this to English");
        assert_eq!(matches2.len(), 1);
        assert_eq!(matches2[0].command_name, "translate");
    }

    #[test]
    fn test_find_matches_priority_sorting() {
        let mut index = UnifiedCommandIndex::new();

        // Two commands with overlapping triggers
        let triggers1 = CommandTriggers::new(vec!["analyze".to_string()], Vec::new());
        index.add_command(ToolSourceType::Skill, "skill-analyze", &triggers1);

        let triggers2 = CommandTriggers::new(vec!["analyze".to_string()], Vec::new());
        index.add_command(ToolSourceType::Custom, "custom-analyze", &triggers2);

        // Skill should come before Custom due to type priority
        let matches = index.find_matches("analyze this code");
        assert!(matches.len() >= 2);
        assert_eq!(matches[0].source_type, ToolSourceType::Skill);
    }
}
