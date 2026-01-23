//! Tool filtering for Agent Loop
//!
//! This module provides intelligent tool filtering to reduce
//! the number of tools presented to the LLM based on context.

use std::collections::{HashMap, HashSet};

use crate::agent_loop::{Observation, ToolInfo};
use crate::dispatcher::UnifiedTool;
use crate::intent::TaskCategory;

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
}

impl ToolFilter {
    /// Create a new tool filter
    pub fn new(config: ToolFilterConfig) -> Self {
        Self { config }
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
            .any(|a| a.mime_type.contains("url") || a.filename.as_ref().map_or(false, |f| f.starts_with("http")))
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
