//! Tool Index System (Smart Tool Discovery)
//!
//! Lightweight tool index for efficient LLM prompt injection.
//! Provides minimal representations of tools to minimize token consumption.
//!
//! Contains:
//! - ToolIndexEntry: Minimal tool representation
//! - ToolIndexCategory: Simplified category for grouping
//! - ToolIndex: Collection of tool entries by category

use super::conflict::ToolSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

// =============================================================================
// Tool Index Entry
// =============================================================================

/// Tool index entry for lightweight tool discovery
///
/// A minimal representation of a tool for LLM prompt injection.
/// Contains only essential metadata to minimize token consumption.
///
/// # Token Efficiency
///
/// Full UnifiedTool with schema: ~200-500 tokens
/// ToolIndexEntry: ~20-30 tokens
///
/// # Usage
///
/// ```rust,ignore
/// let index = registry.generate_tool_index().await;
/// let prompt = ToolIndex::to_prompt(&index);
/// // Inject prompt into LLM context
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolIndexEntry {
    /// Tool command name (e.g., "github:pr_list")
    pub name: String,

    /// Tool category for grouping
    pub category: ToolIndexCategory,

    /// One-line summary (max 50 chars)
    pub summary: String,

    /// Search keywords for relevance matching
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,

    /// Whether this is a core tool (always has full schema)
    #[serde(default)]
    pub is_core: bool,
}

impl ToolIndexEntry {
    /// Create a new tool index entry
    pub fn new(
        name: impl Into<String>,
        category: ToolIndexCategory,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            category,
            summary: summary.into(),
            keywords: Vec::new(),
            is_core: false,
        }
    }

    /// Builder: add keywords
    pub fn with_keywords(mut self, keywords: Vec<String>) -> Self {
        self.keywords = keywords;
        self
    }

    /// Builder: mark as core tool
    pub fn with_core(mut self, is_core: bool) -> Self {
        self.is_core = is_core;
        self
    }

    /// Format for LLM prompt (single line)
    pub fn to_prompt_line(&self) -> String {
        format!("- {}: {}", self.name, self.summary)
    }
}

// =============================================================================
// Tool Index Category
// =============================================================================

/// Tool index category for smart discovery
///
/// Simplified category for tool index grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolIndexCategory {
    /// Core tools (always available with full schema)
    Core,
    /// Built-in tools (search, file_ops, etc.)
    Builtin,
    /// MCP server tools
    Mcp,
    /// Claude Agent skills
    Skill,
    /// User-defined custom commands
    Custom,
}

impl ToolIndexCategory {
    /// Get display name
    pub fn display_name(&self) -> &'static str {
        match self {
            ToolIndexCategory::Core => "Core",
            ToolIndexCategory::Builtin => "Builtin",
            ToolIndexCategory::Mcp => "MCP",
            ToolIndexCategory::Skill => "Skill",
            ToolIndexCategory::Custom => "Custom",
        }
    }
}

impl fmt::Display for ToolIndexCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

impl From<&ToolSource> for ToolIndexCategory {
    fn from(source: &ToolSource) -> Self {
        match source {
            ToolSource::Builtin => ToolIndexCategory::Builtin,
            ToolSource::Native => ToolIndexCategory::Builtin, // Treat native as builtin
            ToolSource::Mcp { .. } => ToolIndexCategory::Mcp,
            ToolSource::Skill { .. } => ToolIndexCategory::Skill,
            ToolSource::Custom { .. } => ToolIndexCategory::Custom,
        }
    }
}

// =============================================================================
// Tool Index
// =============================================================================

/// Tool index for smart discovery
///
/// Contains all tool index entries grouped by category.
/// Used to generate compact tool lists for LLM prompts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolIndex {
    /// Core tools (always available)
    pub core: Vec<ToolIndexEntry>,
    /// Builtin tools
    pub builtin: Vec<ToolIndexEntry>,
    /// MCP tools
    pub mcp: Vec<ToolIndexEntry>,
    /// Skill tools
    pub skill: Vec<ToolIndexEntry>,
    /// Custom tools
    pub custom: Vec<ToolIndexEntry>,
}

impl ToolIndex {
    /// Create empty tool index
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an entry to the appropriate category
    pub fn add(&mut self, entry: ToolIndexEntry) {
        match entry.category {
            ToolIndexCategory::Core => self.core.push(entry),
            ToolIndexCategory::Builtin => self.builtin.push(entry),
            ToolIndexCategory::Mcp => self.mcp.push(entry),
            ToolIndexCategory::Skill => self.skill.push(entry),
            ToolIndexCategory::Custom => self.custom.push(entry),
        }
    }

    /// Get total tool count
    pub fn total_count(&self) -> usize {
        self.core.len() + self.builtin.len() + self.mcp.len() + self.skill.len() + self.custom.len()
    }

    /// Generate markdown prompt for LLM
    ///
    /// Format:
    /// ```markdown
    /// ## Available Tools
    ///
    /// ### Core (always available)
    /// - search: Web search for information
    /// - file_ops: File read/write/delete
    ///
    /// ### MCP (use get_tool_schema for details)
    /// - github:pr_list: List pull requests
    /// ```
    pub fn to_prompt(&self) -> String {
        let mut lines = vec!["## Available Tools".to_string(), String::new()];

        // Core tools
        if !self.core.is_empty() {
            lines.push("### Core (always available with full schema)".to_string());
            for entry in &self.core {
                lines.push(entry.to_prompt_line());
            }
            lines.push(String::new());
        }

        // Builtin tools
        if !self.builtin.is_empty() {
            lines.push("### Builtin".to_string());
            for entry in &self.builtin {
                lines.push(entry.to_prompt_line());
            }
            lines.push(String::new());
        }

        // MCP tools
        if !self.mcp.is_empty() {
            lines.push("### MCP (use get_tool_schema for details)".to_string());
            for entry in &self.mcp {
                lines.push(entry.to_prompt_line());
            }
            lines.push(String::new());
        }

        // Skill tools
        if !self.skill.is_empty() {
            lines.push("### Skills (use get_tool_schema for details)".to_string());
            for entry in &self.skill {
                lines.push(entry.to_prompt_line());
            }
            lines.push(String::new());
        }

        // Custom tools
        if !self.custom.is_empty() {
            lines.push("### Custom".to_string());
            for entry in &self.custom {
                lines.push(entry.to_prompt_line());
            }
            lines.push(String::new());
        }

        lines.join("\n")
    }

    /// Generate compact JSON for programmatic access
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Truncate a string to max length, adding ellipsis if needed
pub fn truncate_string(s: &str, max_len: usize) -> String {
    let s = s.trim();
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 3 {
        format!("{}...", &s[..max_len - 3])
    } else {
        s[..max_len].to_string()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_index_entry_creation() {
        let entry = ToolIndexEntry::new("search", ToolIndexCategory::Core, "Web search");
        assert_eq!(entry.name, "search");
        assert_eq!(entry.category, ToolIndexCategory::Core);
        assert_eq!(entry.summary, "Web search");
        assert!(entry.keywords.is_empty());
        assert!(!entry.is_core);
    }

    #[test]
    fn test_tool_index_entry_with_keywords() {
        let entry = ToolIndexEntry::new("github:pr_list", ToolIndexCategory::Mcp, "List PRs")
            .with_keywords(vec!["github".to_string(), "pr".to_string()])
            .with_core(true);

        assert_eq!(entry.keywords, vec!["github", "pr"]);
        assert!(entry.is_core);
    }

    #[test]
    fn test_tool_index_entry_to_prompt_line() {
        let entry = ToolIndexEntry::new("search", ToolIndexCategory::Core, "Web search");
        assert_eq!(entry.to_prompt_line(), "- search: Web search");
    }

    #[test]
    fn test_tool_index_category_display() {
        assert_eq!(ToolIndexCategory::Core.display_name(), "Core");
        assert_eq!(ToolIndexCategory::Mcp.display_name(), "MCP");
        assert_eq!(ToolIndexCategory::Skill.display_name(), "Skill");
    }

    #[test]
    fn test_tool_index_category_from_source() {
        assert_eq!(
            ToolIndexCategory::from(&ToolSource::Builtin),
            ToolIndexCategory::Builtin
        );
        assert_eq!(
            ToolIndexCategory::from(&ToolSource::Mcp {
                server: "test".into()
            }),
            ToolIndexCategory::Mcp
        );
        assert_eq!(
            ToolIndexCategory::from(&ToolSource::Skill { id: "test".into() }),
            ToolIndexCategory::Skill
        );
    }

    #[test]
    fn test_tool_index_new() {
        let index = ToolIndex::new();
        assert_eq!(index.total_count(), 0);
        assert!(index.core.is_empty());
        assert!(index.mcp.is_empty());
    }

    #[test]
    fn test_tool_index_add() {
        let mut index = ToolIndex::new();

        index.add(ToolIndexEntry::new(
            "search",
            ToolIndexCategory::Core,
            "Web search",
        ));
        index.add(ToolIndexEntry::new(
            "github:pr_list",
            ToolIndexCategory::Mcp,
            "List PRs",
        ));
        index.add(ToolIndexEntry::new(
            "code-review",
            ToolIndexCategory::Skill,
            "Review code",
        ));

        assert_eq!(index.total_count(), 3);
        assert_eq!(index.core.len(), 1);
        assert_eq!(index.mcp.len(), 1);
        assert_eq!(index.skill.len(), 1);
    }

    #[test]
    fn test_tool_index_to_prompt() {
        let mut index = ToolIndex::new();
        index.add(ToolIndexEntry::new(
            "search",
            ToolIndexCategory::Core,
            "Web search",
        ));
        index.add(ToolIndexEntry::new(
            "github:pr_list",
            ToolIndexCategory::Mcp,
            "List PRs",
        ));

        let prompt = index.to_prompt();
        assert!(prompt.contains("## Available Tools"));
        assert!(prompt.contains("### Core"));
        assert!(prompt.contains("- search: Web search"));
        assert!(prompt.contains("### MCP"));
        assert!(prompt.contains("- github:pr_list: List PRs"));
    }

    #[test]
    fn test_truncate_string_short() {
        assert_eq!(truncate_string("Hello", 10), "Hello");
        assert_eq!(truncate_string("  Hello  ", 10), "Hello");
    }

    #[test]
    fn test_truncate_string_long() {
        assert_eq!(truncate_string("Hello World", 8), "Hello...");
        assert_eq!(
            truncate_string("This is a very long string", 10),
            "This is..."
        );
    }

    #[test]
    fn test_truncate_string_exact() {
        assert_eq!(truncate_string("Hello", 5), "Hello");
    }
}
