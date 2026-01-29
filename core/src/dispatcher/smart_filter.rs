//! Smart Tool Filtering (Unified Strategy)
//!
//! Combines intent-based filtering with content analysis for optimal tool selection.
//!
//! # Architecture
//!
//! ```text
//! User Input + Skill Instructions
//!            ↓
//! ┌──────────────────────────────────────────┐
//! │           SmartToolFilter                 │
//! │                                           │
//! │  ┌─────────────────┐  ┌────────────────┐ │
//! │  │  Intent Filter  │  │ Content Filter │ │
//! │  │  (TaskCategory) │  │(infer_required)│ │
//! │  └────────┬────────┘  └───────┬────────┘ │
//! │           │                   │          │
//! │           └─────────┬─────────┘          │
//! │                     ↓                    │
//! │           Combined FilterResult          │
//! └──────────────────────────────────────────┘
//!            ↓
//! ┌─────────────────────────────────────────┐
//! │ core_tools (always)                      │
//! │ filtered_tools (intent + content match)  │
//! │ indexed_tools (name + summary only)      │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{SmartToolFilter, ToolFilterConfig};
//! use aethecore::intent::TaskCategory;
//!
//! let filter = SmartToolFilter::new(ToolFilterConfig::default());
//! let result = filter.filter(
//!     &all_tools,
//!     TaskCategory::FileOrganize,
//!     "organize my downloads folder",
//!     Some("skill instructions here"),
//! );
//!
//! // result.full_schema_tools() - tools with complete parameters
//! // result.indexed_tools - tools with name + summary only
//! ```

use crate::dispatcher::tool_filter::{FilterResult, ToolFilter, ToolFilterConfig};
use crate::dispatcher::UnifiedTool;
use crate::dispatcher::content_category::{infer_required_tools, ContentCategory};
use crate::intent::TaskCategory;

use std::collections::HashSet;
use tracing::info;

/// Smart filter that combines intent classification with content analysis
pub struct SmartToolFilter {
    intent_filter: ToolFilter,
    enable_content_analysis: bool,
}

impl SmartToolFilter {
    /// Create a new smart filter with configuration
    pub fn new(config: ToolFilterConfig) -> Self {
        Self {
            intent_filter: ToolFilter::new(config),
            enable_content_analysis: true,
        }
    }

    /// Create with default configuration
    pub fn default_config() -> Self {
        Self::new(ToolFilterConfig::default())
    }

    /// Enable or disable content analysis
    pub fn with_content_analysis(mut self, enabled: bool) -> Self {
        self.enable_content_analysis = enabled;
        self
    }

    /// Filter tools using combined strategy
    ///
    /// # Arguments
    ///
    /// * `tools` - All available tools
    /// * `task_category` - Intent-based task category
    /// * `user_request` - The user's input request
    /// * `skill_instructions` - Optional skill workflow instructions
    ///
    /// # Returns
    ///
    /// FilterResult with core, filtered, and indexed tools
    pub fn filter(
        &self,
        tools: &[UnifiedTool],
        task_category: TaskCategory,
        user_request: &str,
        skill_instructions: Option<&str>,
    ) -> FilterResult {
        // Step 1: Intent-based filtering
        let mut result = self.intent_filter.filter_by_category(tools, task_category);

        // Step 2: Content-based enhancement
        if self.enable_content_analysis {
            let instructions = skill_instructions.unwrap_or("");
            let content_categories = infer_required_tools(instructions, user_request);

            if !content_categories.is_empty() {
                info!(
                    content_categories = ?content_categories,
                    "Content analysis detected additional tool categories"
                );
                self.enhance_with_content(&mut result, tools, &content_categories);
            }
        }

        info!(
            core_tools = result.core_tools.len(),
            filtered_tools = result.filtered_tools.len(),
            indexed_tools = result.indexed_tools.len(),
            "Smart filter result"
        );

        result
    }

    /// Enhance filter result with content-based tool detection
    fn enhance_with_content(
        &self,
        result: &mut FilterResult,
        all_tools: &[UnifiedTool],
        content_categories: &[ContentCategory],
    ) {
        // Collect existing full-schema tool names
        let existing: HashSet<String> = result
            .full_schema_tools()
            .iter()
            .map(|t| t.name.clone())
            .collect();

        // Check each tool against content categories
        for tool in all_tools {
            if existing.contains(&tool.name) {
                continue;
            }

            let matches = content_categories.iter().any(|cat| Self::tool_matches_category(tool, cat));

            if matches {
                // Move from indexed_tools to filtered_tools
                if let Some(pos) = result.indexed_tools.iter().position(|t| t.name == tool.name) {
                    let tool = result.indexed_tools.remove(pos);
                    info!(
                        tool_name = %tool.name,
                        "Content analysis promoted tool to full schema"
                    );
                    result.filtered_tools.push(tool);
                }
            }
        }
    }

    /// Check if a tool matches a content category
    fn tool_matches_category(tool: &UnifiedTool, category: &ContentCategory) -> bool {
        let name = tool.name.as_str();
        let desc_lower = tool.description.to_lowercase();

        match category {
            ContentCategory::FileOps => {
                name == "file_ops" || desc_lower.contains("file") || desc_lower.contains("directory")
            }
            ContentCategory::Search => {
                name == "search" || desc_lower.contains("search") || desc_lower.contains("find")
            }
            ContentCategory::WebFetch => {
                name == "web_fetch" || desc_lower.contains("fetch") || desc_lower.contains("url")
            }
            ContentCategory::YouTube => {
                name == "youtube" || desc_lower.contains("youtube") || desc_lower.contains("video")
            }
            ContentCategory::Bash => {
                name == "bash" || desc_lower.contains("bash") || desc_lower.contains("shell") || desc_lower.contains("command")
            }
            ContentCategory::CodeExec => {
                name == "code_exec" || desc_lower.contains("code") || desc_lower.contains("execute")
            }
            ContentCategory::ImageGen => {
                name == "generate_image" || name.contains("image") || desc_lower.contains("image generation")
            }
            ContentCategory::VideoGen => {
                name == "generate_video" || name.contains("video") || desc_lower.contains("video generation")
            }
            ContentCategory::AudioGen => {
                name == "generate_audio" || name.contains("audio") || desc_lower.contains("audio generation")
            }
            ContentCategory::SpeechGen => {
                name == "generate_speech" || desc_lower.contains("speech") || desc_lower.contains("tts")
            }
        }
    }
}

impl Default for SmartToolFilter {
    fn default() -> Self {
        Self::default_config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use crate::dispatcher::content_category::ContentCategory;

    fn create_test_tool(name: &str, desc: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("test:{}", name),
            name,
            desc,
            ToolSource::Builtin,
        )
    }

    #[test]
    fn test_smart_filter_basic() {
        let filter = SmartToolFilter::default();

        let tools = vec![
            create_test_tool("search", "Web search"),
            create_test_tool("file_ops", "File operations"),
            create_test_tool("youtube", "YouTube video info"),
            create_test_tool("generate_image", "Generate images"),
        ];

        let result = filter.filter(
            &tools,
            TaskCategory::WebSearch,
            "search for rust tutorials",
            None,
        );

        // Core tools should be included
        assert!(result.core_tools.iter().any(|t| t.name == "search"));
        assert!(result.core_tools.iter().any(|t| t.name == "file_ops"));
    }

    #[test]
    fn test_smart_filter_with_content_analysis() {
        let filter = SmartToolFilter::default();

        let tools = vec![
            create_test_tool("search", "Web search"),
            create_test_tool("file_ops", "File operations"),
            create_test_tool("youtube", "YouTube video info"),
            create_test_tool("generate_image", "Generate images"),
        ];

        // Content mentions "image" which should promote generate_image
        let result = filter.filter(
            &tools,
            TaskCategory::General,
            "help me create an image of a sunset",
            None,
        );

        // generate_image should be promoted to filtered_tools due to content analysis
        let full_schema_tools = result.full_schema_tools();
        let all_full_schema: Vec<_> = full_schema_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(all_full_schema.contains(&"generate_image"), "generate_image should be in full schema tools");
    }

    #[test]
    fn test_smart_filter_with_skill_instructions() {
        let filter = SmartToolFilter::default();

        let tools = vec![
            create_test_tool("search", "Web search"),
            create_test_tool("file_ops", "File operations"),
            create_test_tool("youtube", "YouTube video info"),
        ];

        // Skill instructions mention YouTube
        let result = filter.filter(
            &tools,
            TaskCategory::General,
            "process this content",
            Some("Download YouTube video transcript and summarize"),
        );

        // youtube should be included due to skill instructions
        let full_schema_tools = result.full_schema_tools();
        let all_full_schema: Vec<_> = full_schema_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(all_full_schema.contains(&"youtube"), "youtube should be in full schema tools");
    }

    #[test]
    fn test_smart_filter_disable_content_analysis() {
        let filter = SmartToolFilter::default().with_content_analysis(false);

        let tools = vec![
            create_test_tool("search", "Web search"),
            create_test_tool("generate_image", "Generate images"),
        ];

        // Even though content mentions "image", generate_image won't be promoted
        // because content analysis is disabled
        let result = filter.filter(
            &tools,
            TaskCategory::General,
            "create an image",
            None,
        );

        // Without content analysis, non-core tools go to indexed
        // (unless matched by intent category)
        assert!(!result.filtered_tools.iter().any(|t| t.name == "generate_image"));
    }

    #[test]
    fn test_tool_matches_category() {
        let image_tool = create_test_tool("generate_image", "Generate images from prompts");
        let file_tool = create_test_tool("file_ops", "File system operations");
        let search_tool = create_test_tool("search", "Search the web");

        assert!(SmartToolFilter::tool_matches_category(&image_tool, &ContentCategory::ImageGen));
        assert!(SmartToolFilter::tool_matches_category(&file_tool, &ContentCategory::FileOps));
        assert!(SmartToolFilter::tool_matches_category(&search_tool, &ContentCategory::Search));

        assert!(!SmartToolFilter::tool_matches_category(&image_tool, &ContentCategory::FileOps));
        assert!(!SmartToolFilter::tool_matches_category(&file_tool, &ContentCategory::Search));
    }
}
