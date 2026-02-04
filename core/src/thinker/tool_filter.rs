//! Tool filtering for Agent Loop
//!
//! This module provides intelligent tool filtering to reduce
//! the number of tools presented to the LLM based on context.
//!
//! # Two-Level Filtering
//!
//! Tool filtering happens at two levels:
//!
//! 1. **Intent-based filtering** (`dispatcher::tool_filter`):
//!    - Runs before Agent Loop
//!    - Filters based on detected TaskCategory
//!    - Produces full_schema_tools + indexed_tools
//!
//! 2. **Observation-based filtering** (this module):
//!    - Runs within Agent Loop
//!    - Filters based on runtime context (history, attachments)
//!    - Further refines the tool set based on current step
//!
//! The two filters work together:
//! - Dispatcher filter determines what tools are available
//! - Thinker filter refines based on what's relevant for current step

use std::collections::{HashMap, HashSet};

use crate::agent_loop::{Observation, ToolInfo};
use crate::dispatcher::{tool_filter as intent_filter, UnifiedTool};
use crate::intent::TaskCategory;

// Re-export intent filter types for convenience
pub use intent_filter::{FilterResult as IntentFilterResult, ToolFilterConfig as IntentFilterConfig};

/// Tool filter configuration
#[derive(Debug, Clone)]
pub struct ToolFilterConfig {
    /// Maximum number of tools to include
    pub max_tools: usize,
    /// Always include these tools regardless of context
    pub always_include: Vec<String>,
    /// Category to tool mappings
    pub category_tools: HashMap<TaskCategory, Vec<String>>,
    /// Enable dynamic filtering based on history
    pub dynamic_filtering: bool,
}

impl Default for ToolFilterConfig {
    fn default() -> Self {
        Self {
            max_tools: 20,
            always_include: vec![
                "complete".to_string(),
                "ask_user".to_string(),
            ],
            category_tools: Self::default_category_tools(),
            dynamic_filtering: true,
        }
    }
}

impl ToolFilterConfig {
    fn default_category_tools() -> HashMap<TaskCategory, Vec<String>> {
        let mut map = HashMap::new();

        map.insert(
            TaskCategory::FileOperation,
            vec![
                "read_file".to_string(),
                "write_file".to_string(),
                "list_directory".to_string(),
                "delete_file".to_string(),
                "copy_file".to_string(),
                "move_file".to_string(),
            ],
        );

        map.insert(
            TaskCategory::WebSearch,
            vec![
                "web_search".to_string(),
                "web_fetch".to_string(),
            ],
        );

        map.insert(
            TaskCategory::CodeExecution,
            vec![
                "execute_code".to_string(),
                "run_command".to_string(),
            ],
        );

        map.insert(
            TaskCategory::ImageGeneration,
            vec![
                "generate_image".to_string(),
                "edit_image".to_string(),
            ],
        );

        map
    }
}

/// Tool filter for reducing tool set based on context
pub struct ToolFilter {
    config: ToolFilterConfig,
    /// Optional intent-based filter for pre-filtering
    intent_filter: Option<intent_filter::ToolFilter>,
}

impl ToolFilter {
    /// Create a new tool filter
    pub fn new(config: ToolFilterConfig) -> Self {
        Self {
            config,
            intent_filter: None,
        }
    }

    /// Create a new tool filter with intent-based pre-filtering
    pub fn with_intent_filter(config: ToolFilterConfig, intent_config: intent_filter::ToolFilterConfig) -> Self {
        Self {
            config,
            intent_filter: Some(intent_filter::ToolFilter::new(intent_config)),
        }
    }

    /// Pre-filter tools by task category using intent-based filter
    ///
    /// Returns a FilterResult containing:
    /// - core_tools: Always available with full schema
    /// - filtered_tools: Relevant to the category with full schema
    /// - indexed_tools: Available but need get_tool_schema to use
    pub fn pre_filter_by_category(
        &self,
        tools: &[UnifiedTool],
        category: TaskCategory,
    ) -> intent_filter::FilterResult {
        if let Some(ref filter) = self.intent_filter {
            filter.filter_by_category(tools, category)
        } else {
            // Fallback: use default intent filter
            let default_filter = intent_filter::ToolFilter::default_config();
            default_filter.filter_by_category(tools, category)
        }
    }

    /// Filter tools based on observation context
    pub fn filter(&self, tools: &[UnifiedTool], observation: &Observation) -> Vec<ToolInfo> {
        let mut selected: HashSet<String> = HashSet::new();
        let mut result: Vec<ToolInfo> = Vec::new();

        // 1. Always include essential tools
        for tool_name in &self.config.always_include {
            if let Some(tool) = tools.iter().find(|t| &t.name == tool_name) {
                if selected.insert(tool.name.clone()) {
                    result.push(self.to_tool_info(tool));
                }
            }
        }

        // 2. Include tools from recent history (user likely needs them again)
        if self.config.dynamic_filtering {
            for step in &observation.recent_steps {
                if let Some(tool_name) = extract_tool_name(&step.action_type) {
                    if let Some(tool) = tools.iter().find(|t| t.name == tool_name) {
                        if selected.insert(tool.name.clone()) {
                            result.push(self.to_tool_info(tool));
                        }
                    }
                }
            }
        }

        // 3. Detect likely categories from request/context and add relevant tools
        let detected_categories = self.detect_categories(observation);
        for category in detected_categories {
            if let Some(tool_names) = self.config.category_tools.get(&category) {
                for tool_name in tool_names {
                    if result.len() >= self.config.max_tools {
                        break;
                    }
                    if let Some(tool) = tools.iter().find(|t| &t.name == tool_name) {
                        if selected.insert(tool.name.clone()) {
                            result.push(self.to_tool_info(tool));
                        }
                    }
                }
            }
        }

        // 4. Fill remaining slots with general-purpose tools
        for tool in tools {
            if result.len() >= self.config.max_tools {
                break;
            }
            if selected.insert(tool.name.clone()) {
                result.push(self.to_tool_info(tool));
            }
        }

        result
    }

    /// Detect likely task categories from observation
    fn detect_categories(&self, observation: &Observation) -> Vec<TaskCategory> {
        let mut categories = Vec::new();

        // Check attachments for hints (MediaAttachment is a struct, not enum)
        if observation
            .attachments
            .iter()
            .any(|a| a.media_type == "image" || a.mime_type.starts_with("image/"))
        {
            categories.push(TaskCategory::ImageGeneration);
        }

        if observation
            .attachments
            .iter()
            .any(|a| a.media_type == "file" || a.media_type == "document")
        {
            categories.push(TaskCategory::FileOperation);
        }

        // URL detection from mime_type or filename
        if observation
            .attachments
            .iter()
            .any(|a| a.mime_type.contains("url") || a.filename.as_ref().is_some_and(|f| f.starts_with("http")))
        {
            categories.push(TaskCategory::WebFetch);
        }

        // Check recent steps for patterns
        for step in &observation.recent_steps {
            if step.action_type.contains("search") {
                categories.push(TaskCategory::WebSearch);
            }
            if step.action_type.contains("file") {
                categories.push(TaskCategory::FileOperation);
            }
            if step.action_type.contains("code") || step.action_type.contains("execute") {
                categories.push(TaskCategory::CodeExecution);
            }
        }

        // Deduplicate
        categories.sort_by_key(|c| format!("{:?}", c));
        categories.dedup();

        categories
    }

    /// Convert UnifiedTool to ToolInfo
    fn to_tool_info(&self, tool: &UnifiedTool) -> ToolInfo {
        ToolInfo {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters_schema: tool
                .parameters_schema
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
                .unwrap_or_else(|| "{}".to_string()),
            category: Some(format!("{:?}", &tool.source)),
        }
    }
}

/// Extract tool name from action type string (e.g., "tool:search" -> "search")
fn extract_tool_name(action_type: &str) -> Option<String> {
    if action_type.starts_with("tool:") {
        Some(action_type[5..].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::StepSummary;
    use crate::dispatcher::ToolSource;

    fn create_test_tool(name: &str, description: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("builtin:{}", name),
            name,
            description,
            ToolSource::Builtin,
        )
    }

    #[test]
    fn test_always_include_tools() {
        let filter = ToolFilter::new(ToolFilterConfig::default());

        let tools = vec![
            create_test_tool("complete", "Complete task"),
            create_test_tool("ask_user", "Ask user"),
            create_test_tool("search", "Search"),
        ];

        let observation = Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![],
            current_step: 0,
            total_tokens: 0,
        };

        let filtered = filter.filter(&tools, &observation);

        assert!(filtered.iter().any(|t| t.name == "complete"));
        assert!(filtered.iter().any(|t| t.name == "ask_user"));
    }

    #[test]
    fn test_history_based_filtering() {
        let filter = ToolFilter::new(ToolFilterConfig::default());

        let tools = vec![
            create_test_tool("complete", "Complete task"),
            create_test_tool("search", "Search"),
            create_test_tool("read_file", "Read file"),
            create_test_tool("write_file", "Write file"),
        ];

        let observation = Observation {
            history_summary: String::new(),
            recent_steps: vec![StepSummary {
                step_id: 0,
                reasoning: "Search".to_string(),
                action_type: "tool:search".to_string(),
                action_args: "{}".to_string(),
                result_summary: "Found results".to_string(),
                result_output: r#"{"results": ["item1", "item2"]}"#.to_string(),
                success: true,
            }],
            available_tools: vec![],
            attachments: vec![],
            current_step: 1,
            total_tokens: 0,
        };

        let filtered = filter.filter(&tools, &observation);

        // Search should be included because it was used recently
        assert!(filtered.iter().any(|t| t.name == "search"));
    }

    #[test]
    fn test_max_tools_limit() {
        let mut config = ToolFilterConfig::default();
        config.max_tools = 3;

        let filter = ToolFilter::new(config);

        let tools: Vec<UnifiedTool> = (0..10)
            .map(|i| create_test_tool(&format!("tool_{}", i), &format!("Tool {}", i)))
            .collect();

        let observation = Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![],
            current_step: 0,
            total_tokens: 0,
        };

        let filtered = filter.filter(&tools, &observation);

        assert!(filtered.len() <= 3);
    }
}
