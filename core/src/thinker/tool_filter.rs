//! Tool filtering for Agent Loop
//!
//! This module provides intelligent tool filtering to reduce
//! the number of tools presented to the LLM based on context.
//!
//! Observation-based filtering runs within the Agent Loop,
//! filtering based on runtime context (history, attachments)
//! and refining the tool set based on the current step.

use std::collections::{HashMap, HashSet};

use crate::agent_loop::{Observation, ToolInfo};
use crate::config::ProfileConfig;
use crate::dispatcher::UnifiedTool;

/// Tool filter configuration
#[derive(Debug, Clone)]
pub struct ToolFilterConfig {
    /// Maximum number of tools to include
    pub max_tools: usize,
    /// Always include these tools regardless of context
    pub always_include: Vec<String>,
    /// Category to tool mappings (string keys for flexibility)
    pub category_tools: HashMap<String, Vec<String>>,
    /// Enable dynamic filtering based on history
    pub dynamic_filtering: bool,
}

impl Default for ToolFilterConfig {
    fn default() -> Self {
        Self {
            max_tools: 128,
            always_include: vec!["complete".to_string(), "ask_user".to_string()],
            category_tools: Self::default_category_tools(),
            dynamic_filtering: true,
        }
    }
}

impl ToolFilterConfig {
    fn default_category_tools() -> HashMap<String, Vec<String>> {
        let mut map = HashMap::new();

        map.insert(
            "file_operation".to_string(),
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
            "web_search".to_string(),
            vec!["web_search".to_string(), "web_fetch".to_string()],
        );

        map.insert(
            "code_execution".to_string(),
            vec!["execute_code".to_string(), "run_command".to_string()],
        );

        map.insert(
            "image_generation".to_string(),
            vec!["generate_image".to_string(), "edit_image".to_string()],
        );

        map
    }
}

/// Tool filter for reducing tool set based on context
pub struct ToolFilter {
    config: ToolFilterConfig,
    /// Optional profile for workspace-based filtering
    profile: Option<ProfileConfig>,
}

impl ToolFilter {
    /// Create a new tool filter
    pub fn new(config: ToolFilterConfig) -> Self {
        Self {
            config,
            profile: None,
        }
    }

    /// Set the profile for workspace-based filtering
    pub fn with_profile(mut self, profile: Option<ProfileConfig>) -> Self {
        self.profile = profile;
        self
    }

    /// Update the profile at runtime
    pub fn set_profile(&mut self, profile: Option<ProfileConfig>) {
        self.profile = profile;
    }

    /// Check if a tool is allowed by the current profile
    fn is_tool_allowed_by_profile(&self, tool_name: &str) -> bool {
        match &self.profile {
            Some(p) if !p.tools.is_empty() => p.is_tool_allowed(tool_name),
            _ => true, // No profile or empty whitelist = all tools allowed
        }
    }

    /// Filter tools based on observation context
    pub fn filter(&self, tools: &[UnifiedTool], observation: &Observation) -> Vec<ToolInfo> {
        // 0. First apply profile whitelist filter (The Lens - Layer 1)
        let profile_filtered: Vec<&UnifiedTool> = tools
            .iter()
            .filter(|tool| self.is_tool_allowed_by_profile(&tool.name))
            .collect();

        let mut selected: HashSet<String> = HashSet::new();
        let mut result: Vec<ToolInfo> = Vec::new();

        // 1. Always include essential tools (if allowed by profile)
        for tool_name in &self.config.always_include {
            if let Some(tool) = profile_filtered.iter().find(|t| &t.name == tool_name) {
                if selected.insert(tool.name.clone()) {
                    result.push(self.to_tool_info(tool));
                }
            }
        }

        // 2. Include tools from recent history (user likely needs them again)
        if self.config.dynamic_filtering {
            for step in &observation.recent_steps {
                if let Some(tool_name) = extract_tool_name(&step.action_type) {
                    if let Some(tool) = profile_filtered.iter().find(|t| t.name == tool_name) {
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
                    if let Some(tool) = profile_filtered.iter().find(|t| &t.name == tool_name) {
                        if selected.insert(tool.name.clone()) {
                            result.push(self.to_tool_info(tool));
                        }
                    }
                }
            }
        }

        // 4. Fill remaining slots with profile-allowed tools
        for tool in &profile_filtered {
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
    fn detect_categories(&self, observation: &Observation) -> Vec<String> {
        let mut categories = Vec::new();

        // Check attachments for hints (MediaAttachment is a struct, not enum)
        if observation
            .attachments
            .iter()
            .any(|a| a.media_type == "image" || a.mime_type.starts_with("image/"))
        {
            categories.push("image_generation".to_string());
        }

        if observation
            .attachments
            .iter()
            .any(|a| a.media_type == "file" || a.media_type == "document")
        {
            categories.push("file_operation".to_string());
        }

        // URL detection from mime_type or filename
        if observation.attachments.iter().any(|a| {
            a.mime_type.contains("url")
                || a.filename.as_ref().is_some_and(|f| f.starts_with("http"))
        }) {
            categories.push("web_fetch".to_string());
        }

        // Check recent steps for patterns
        for step in &observation.recent_steps {
            if step.action_type.contains("search") {
                categories.push("web_search".to_string());
            }
            if step.action_type.contains("file") {
                categories.push("file_operation".to_string());
            }
            if step.action_type.contains("code") || step.action_type.contains("execute") {
                categories.push("code_execution".to_string());
            }
        }

        // Deduplicate
        categories.sort();
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
    action_type.strip_prefix("tool:").map(|s| s.to_string())
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
                tool_call_id: None,
                tool_results: Vec::new(),
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
        let config = ToolFilterConfig {
            max_tools: 3,
            ..ToolFilterConfig::default()
        };

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
