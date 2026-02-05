//! Smart Tool Filtering for Intent-Based Discovery
//!
//! This module provides intent-based tool filtering to reduce the number of tools
//! passed to LLM, improving token efficiency and response quality.
//!
//! # Architecture
//!
//! ```text
//! User Input → Intent Detection → TaskCategory
//!                                      ↓
//!                            Tool Filter (this module)
//!                                      ↓
//!                     ┌────────────────┴────────────────┐
//!                     ↓                                 ↓
//!             Core Tools (always)              Filtered Tools (by intent)
//!                     ↓                                 ↓
//!                     └────────────────┬────────────────┘
//!                                      ↓
//!                              LLM (reduced tools)
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::dispatcher::{ToolFilter, ToolFilterConfig};
//! use alephcore::intent::types::TaskCategory;
//!
//! let filter = ToolFilter::new(ToolFilterConfig::default());
//! let relevant_tools = filter.filter_by_category(&all_tools, TaskCategory::WebSearch);
//! ```

use std::collections::HashSet;

use crate::config::ProfileConfig;
use crate::dispatcher::{ToolIndexCategory, UnifiedTool};
use crate::intent::types::TaskCategory;

/// Configuration for tool filtering
#[derive(Debug, Clone)]
pub struct ToolFilterConfig {
    /// Core tools that are always included (with full schema)
    pub core_tools: Vec<String>,
    /// Maximum number of filtered tools to include
    pub max_filtered_tools: usize,
    /// Whether to include MCP tools in filtering
    pub include_mcp: bool,
    /// Whether to include Skill tools in filtering
    pub include_skills: bool,
}

impl Default for ToolFilterConfig {
    fn default() -> Self {
        Self {
            core_tools: vec![
                "search".to_string(),
                "file_ops".to_string(),
                "list_tools".to_string(),
                "get_tool_schema".to_string(),
            ],
            max_filtered_tools: 10,
            include_mcp: true,
            include_skills: true,
        }
    }
}

impl ToolFilterConfig {
    /// Create config with custom core tools
    pub fn with_core_tools(mut self, tools: Vec<String>) -> Self {
        self.core_tools = tools;
        self
    }

    /// Set max filtered tools
    pub fn with_max_filtered(mut self, max: usize) -> Self {
        self.max_filtered_tools = max;
        self
    }
}

/// Tool filter for intent-based discovery
pub struct ToolFilter {
    config: ToolFilterConfig,
}

impl ToolFilter {
    /// Create a new tool filter with configuration
    pub fn new(config: ToolFilterConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn default_config() -> Self {
        Self::new(ToolFilterConfig::default())
    }

    /// Get core tool names
    pub fn core_tools(&self) -> &[String] {
        &self.config.core_tools
    }

    /// Check if a tool is a core tool
    pub fn is_core_tool(&self, tool_name: &str) -> bool {
        self.config.core_tools.iter().any(|t| t == tool_name)
    }

    /// Filter tools by task category
    ///
    /// Returns tools relevant to the given task category, plus core tools.
    /// Results are limited by `max_filtered_tools` config.
    pub fn filter_by_category(
        &self,
        tools: &[UnifiedTool],
        category: TaskCategory,
    ) -> FilterResult {
        let relevant_keywords = Self::category_to_keywords(category);
        let relevant_tool_names = Self::category_to_tool_names(category);

        let mut core_tools = Vec::new();
        let mut filtered_tools = Vec::new();
        let mut indexed_tools = Vec::new();

        for tool in tools {
            if self.is_core_tool(&tool.name) {
                // Core tools always included with full schema
                core_tools.push(tool.clone());
            } else if self.is_tool_relevant(tool, &relevant_keywords, &relevant_tool_names) {
                // Relevant tools included with full schema (up to limit)
                if filtered_tools.len() < self.config.max_filtered_tools {
                    filtered_tools.push(tool.clone());
                } else {
                    indexed_tools.push(tool.clone());
                }
            } else {
                // Non-relevant tools go to index
                indexed_tools.push(tool.clone());
            }
        }

        FilterResult {
            core_tools,
            filtered_tools,
            indexed_tools,
            category,
        }
    }

    /// Filter tools by category with profile-based whitelist
    ///
    /// This is the primary method for workspace-aware filtering.
    /// It first applies the profile whitelist, then filters by category.
    pub fn filter_by_category_with_profile(
        &self,
        tools: &[UnifiedTool],
        category: TaskCategory,
        profile: Option<&ProfileConfig>,
    ) -> FilterResult {
        // First, apply profile whitelist filter
        let profile_filtered: Vec<UnifiedTool> = match profile {
            Some(p) if !p.tools.is_empty() => tools
                .iter()
                .filter(|tool| p.is_tool_allowed(&tool.name))
                .cloned()
                .collect(),
            _ => tools.to_vec(),
        };

        // Then apply category-based filtering
        self.filter_by_category(&profile_filtered, category)
    }

    /// Filter tools by multiple categories with profile-based whitelist
    pub fn filter_by_categories_with_profile(
        &self,
        tools: &[UnifiedTool],
        categories: &[TaskCategory],
        profile: Option<&ProfileConfig>,
    ) -> FilterResult {
        // First, apply profile whitelist filter
        let profile_filtered: Vec<UnifiedTool> = match profile {
            Some(p) if !p.tools.is_empty() => tools
                .iter()
                .filter(|tool| p.is_tool_allowed(&tool.name))
                .cloned()
                .collect(),
            _ => tools.to_vec(),
        };

        // Then apply category-based filtering
        self.filter_by_categories(&profile_filtered, categories)
    }

    /// Filter tools by multiple categories
    pub fn filter_by_categories(
        &self,
        tools: &[UnifiedTool],
        categories: &[TaskCategory],
    ) -> FilterResult {
        // Combine keywords from all categories
        let mut all_keywords = HashSet::new();
        let mut all_tool_names = HashSet::new();

        for category in categories {
            for kw in Self::category_to_keywords(*category) {
                all_keywords.insert(kw);
            }
            for name in Self::category_to_tool_names(*category) {
                all_tool_names.insert(name);
            }
        }

        let mut core_tools = Vec::new();
        let mut filtered_tools = Vec::new();
        let mut indexed_tools = Vec::new();

        for tool in tools {
            if self.is_core_tool(&tool.name) {
                core_tools.push(tool.clone());
            } else if self.is_tool_relevant_multi(tool, &all_keywords, &all_tool_names) {
                if filtered_tools.len() < self.config.max_filtered_tools {
                    filtered_tools.push(tool.clone());
                } else {
                    indexed_tools.push(tool.clone());
                }
            } else {
                indexed_tools.push(tool.clone());
            }
        }

        FilterResult {
            core_tools,
            filtered_tools,
            indexed_tools,
            category: categories.first().copied().unwrap_or(TaskCategory::General),
        }
    }

    /// Map TaskCategory to relevant keywords for tool matching
    fn category_to_keywords(category: TaskCategory) -> Vec<&'static str> {
        match category {
            // File operations
            TaskCategory::FileOrganize => vec!["file", "organize", "sort", "classify", "folder"],
            TaskCategory::FileOperation => vec!["file", "read", "write", "list", "search", "find"],
            TaskCategory::FileTransfer => vec!["file", "move", "copy", "transfer"],
            TaskCategory::FileCleanup => vec!["file", "delete", "remove", "clean", "archive"],

            // Web operations
            TaskCategory::WebSearch => vec!["search", "web", "find", "query", "lookup"],
            TaskCategory::WebFetch => vec!["fetch", "web", "http", "url", "page", "content"],

            // Media operations
            TaskCategory::MediaDownload => vec!["youtube", "video", "download", "media", "audio"],
            TaskCategory::ImageGeneration => vec!["image", "generate", "draw", "picture", "photo"],
            TaskCategory::VideoGeneration => vec!["video", "generate", "animate", "movie"],
            TaskCategory::AudioGeneration => vec!["audio", "generate", "music", "sound"],
            TaskCategory::SpeechGeneration => vec!["speech", "tts", "voice", "speak"],

            // Code & execution
            TaskCategory::CodeExecution => vec!["code", "execute", "run", "script", "shell", "python"],
            TaskCategory::AppLaunch => vec!["app", "launch", "open", "start"],
            TaskCategory::AppAutomation => vec!["app", "automate", "ui", "click", "type"],

            // Documents
            TaskCategory::DocumentGeneration | TaskCategory::DocumentGenerate => {
                vec!["document", "pdf", "doc", "report", "generate"]
            }

            // Text & data
            TaskCategory::TextProcessing => vec!["text", "translate", "summarize", "process"],
            TaskCategory::DataProcess => vec!["data", "process", "transform", "analyze"],

            // System
            TaskCategory::SystemInfo => vec!["system", "info", "status", "monitor"],

            // General
            TaskCategory::General => vec![],
        }
    }

    /// Map TaskCategory to specific tool names
    fn category_to_tool_names(category: TaskCategory) -> Vec<&'static str> {
        match category {
            TaskCategory::FileOrganize
            | TaskCategory::FileOperation
            | TaskCategory::FileTransfer
            | TaskCategory::FileCleanup => vec!["file_ops"],

            TaskCategory::WebSearch => vec!["search"],
            TaskCategory::WebFetch => vec!["web_fetch"],
            TaskCategory::MediaDownload => vec!["youtube"],

            TaskCategory::ImageGeneration => vec!["generate_image"],
            TaskCategory::VideoGeneration => vec!["generate_video"],
            TaskCategory::AudioGeneration | TaskCategory::SpeechGeneration => vec!["generate_audio"],

            TaskCategory::CodeExecution => vec!["code_exec"],

            TaskCategory::DocumentGeneration | TaskCategory::DocumentGenerate => {
                vec!["pdf_generate"]
            }

            _ => vec![],
        }
    }

    /// Check if a tool is relevant based on keywords and names
    fn is_tool_relevant(
        &self,
        tool: &UnifiedTool,
        keywords: &[&str],
        tool_names: &[&str],
    ) -> bool {
        // Check source type filter
        let source_category = ToolIndexCategory::from(&tool.source);
        match source_category {
            ToolIndexCategory::Mcp if !self.config.include_mcp => return false,
            ToolIndexCategory::Skill if !self.config.include_skills => return false,
            _ => {}
        }

        // Direct name match
        if tool_names.contains(&tool.name.as_str()) {
            return true;
        }

        // Keyword matching in name and description
        let name_lower = tool.name.to_lowercase();
        let desc_lower = tool.description.to_lowercase();

        for keyword in keywords {
            if name_lower.contains(keyword) || desc_lower.contains(keyword) {
                return true;
            }
        }

        false
    }

    /// Check tool relevance with HashSet for efficiency
    fn is_tool_relevant_multi(
        &self,
        tool: &UnifiedTool,
        keywords: &HashSet<&str>,
        tool_names: &HashSet<&str>,
    ) -> bool {
        // Check source type filter
        let source_category = ToolIndexCategory::from(&tool.source);
        match source_category {
            ToolIndexCategory::Mcp if !self.config.include_mcp => return false,
            ToolIndexCategory::Skill if !self.config.include_skills => return false,
            _ => {}
        }

        // Direct name match
        if tool_names.contains(tool.name.as_str()) {
            return true;
        }

        // Keyword matching
        let name_lower = tool.name.to_lowercase();
        let desc_lower = tool.description.to_lowercase();

        for keyword in keywords {
            if name_lower.contains(keyword) || desc_lower.contains(keyword) {
                return true;
            }
        }

        false
    }
}

/// Result of tool filtering
#[derive(Debug, Clone)]
pub struct FilterResult {
    /// Core tools (always included with full schema)
    pub core_tools: Vec<UnifiedTool>,
    /// Filtered tools relevant to intent (included with full schema)
    pub filtered_tools: Vec<UnifiedTool>,
    /// Remaining tools (included in index only)
    pub indexed_tools: Vec<UnifiedTool>,
    /// The primary category used for filtering
    pub category: TaskCategory,
}

impl FilterResult {
    /// Get all tools with full schema (core + filtered)
    pub fn full_schema_tools(&self) -> Vec<UnifiedTool> {
        let mut tools = self.core_tools.clone();
        tools.extend(self.filtered_tools.clone());
        tools
    }

    /// Get tool names for full schema
    pub fn full_schema_names(&self) -> Vec<String> {
        self.full_schema_tools()
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    /// Get total number of tools with full schema
    pub fn full_schema_count(&self) -> usize {
        self.core_tools.len() + self.filtered_tools.len()
    }

    /// Get total number of indexed tools
    pub fn indexed_count(&self) -> usize {
        self.indexed_tools.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;

    fn create_test_tool(name: &str, desc: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("test:{}", name),
            name,
            desc,
            ToolSource::Builtin,
        )
    }

    #[test]
    fn test_filter_config_default() {
        let config = ToolFilterConfig::default();
        assert!(config.core_tools.contains(&"search".to_string()));
        assert!(config.core_tools.contains(&"file_ops".to_string()));
        assert_eq!(config.max_filtered_tools, 10);
    }

    #[test]
    fn test_is_core_tool() {
        let filter = ToolFilter::default_config();
        assert!(filter.is_core_tool("search"));
        assert!(filter.is_core_tool("file_ops"));
        assert!(!filter.is_core_tool("youtube"));
    }

    #[test]
    fn test_filter_by_category_web_search() {
        let filter = ToolFilter::default_config();

        let tools = vec![
            create_test_tool("search", "Search the web"),
            create_test_tool("file_ops", "File operations"),
            create_test_tool("youtube", "Download YouTube videos"),
            create_test_tool("generate_image", "Generate images"),
            create_test_tool("web_fetch", "Fetch web pages"),
        ];

        let result = filter.filter_by_category(&tools, TaskCategory::WebSearch);

        // Core tools
        assert!(result.core_tools.iter().any(|t| t.name == "search"));
        assert!(result.core_tools.iter().any(|t| t.name == "file_ops"));

        // Filtered tools (web_fetch should be relevant to WebSearch)
        assert!(result.filtered_tools.iter().any(|t| t.name == "web_fetch"));

        // Non-relevant tools should be indexed
        assert!(result.indexed_tools.iter().any(|t| t.name == "generate_image"));
    }

    #[test]
    fn test_filter_by_category_media_download() {
        let filter = ToolFilter::default_config();

        let tools = vec![
            create_test_tool("search", "Search the web"),
            create_test_tool("youtube", "Download YouTube videos"),
            create_test_tool("file_ops", "File operations"),
            create_test_tool("generate_image", "Generate images"),
        ];

        let result = filter.filter_by_category(&tools, TaskCategory::MediaDownload);

        // youtube should be filtered (relevant)
        assert!(result.filtered_tools.iter().any(|t| t.name == "youtube"));
    }

    #[test]
    fn test_filter_by_category_image_generation() {
        let filter = ToolFilter::default_config();

        let tools = vec![
            create_test_tool("search", "Search the web"),
            create_test_tool("generate_image", "Generate images from prompts"),
            create_test_tool("generate_video", "Generate videos"),
        ];

        let result = filter.filter_by_category(&tools, TaskCategory::ImageGeneration);

        // generate_image should be filtered
        assert!(result
            .filtered_tools
            .iter()
            .any(|t| t.name == "generate_image"));
    }

    #[test]
    fn test_filter_by_categories_multiple() {
        let filter = ToolFilter::default_config();

        let tools = vec![
            create_test_tool("search", "Search the web"),
            create_test_tool("file_ops", "File operations"),
            create_test_tool("youtube", "Download YouTube videos"),
            create_test_tool("generate_image", "Generate images"),
        ];

        let result = filter.filter_by_categories(
            &tools,
            &[TaskCategory::WebSearch, TaskCategory::MediaDownload],
        );

        // Both web-related and media-related tools should be filtered
        let filtered_names: Vec<_> = result.filtered_tools.iter().map(|t| &t.name).collect();
        assert!(filtered_names.contains(&&"youtube".to_string()));
    }

    #[test]
    fn test_filter_result_full_schema_tools() {
        let filter = ToolFilter::default_config();

        let tools = vec![
            create_test_tool("search", "Search the web"),
            create_test_tool("youtube", "Download videos"),
        ];

        let result = filter.filter_by_category(&tools, TaskCategory::MediaDownload);
        let full_schema = result.full_schema_tools();

        // Should include core tool (search) and filtered tool (youtube)
        assert!(full_schema.iter().any(|t| t.name == "search"));
        assert!(full_schema.iter().any(|t| t.name == "youtube"));
    }

    #[test]
    fn test_max_filtered_tools_limit() {
        let config = ToolFilterConfig::default().with_max_filtered(2);
        let filter = ToolFilter::new(config);

        // Create many file-related tools
        let tools = vec![
            create_test_tool("search", "Search"),
            create_test_tool("file_read", "Read files"),
            create_test_tool("file_write", "Write files"),
            create_test_tool("file_delete", "Delete files"),
            create_test_tool("file_search", "Search files"),
        ];

        let result = filter.filter_by_category(&tools, TaskCategory::FileOperation);

        // Max 2 filtered tools (excluding core)
        assert!(result.filtered_tools.len() <= 2);
        // Rest should go to indexed
        assert!(!result.indexed_tools.is_empty());
    }

    #[test]
    fn test_category_to_keywords() {
        let keywords = ToolFilter::category_to_keywords(TaskCategory::WebSearch);
        assert!(keywords.contains(&"search"));
        assert!(keywords.contains(&"web"));

        let keywords = ToolFilter::category_to_keywords(TaskCategory::ImageGeneration);
        assert!(keywords.contains(&"image"));
        assert!(keywords.contains(&"generate"));
    }
}
