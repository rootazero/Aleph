//! Task planner component - decomposes complex tasks into steps.
//!
//! Subscribes to: PlanRequested
//! Publishes: PlanCreated

use async_trait::async_trait;
use serde_json::Value;

use crate::event::{
    AlephEvent, EventContext, EventHandler, EventType, HandlerError, PlanRequest, PlanStep,
    StepStatus, TaskPlan,
};

// ============================================================================
// TaskPlanner Component
// ============================================================================

/// Task Planner - decomposes complex requests into executable steps
///
/// This component:
/// - Subscribes to PlanRequested events
/// - Analyzes the request and detected steps
/// - Creates a structured TaskPlan with dependencies and parallel groups
/// - Publishes PlanCreated events
pub struct TaskPlanner {
    /// Whether to use LLM-based planning (false = rule-based planning)
    use_llm: bool,
}

impl Default for TaskPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskPlanner {
    /// Create a new TaskPlanner with rule-based planning
    pub fn new() -> Self {
        Self { use_llm: false }
    }

    /// Create a TaskPlanner with LLM-based planning enabled
    pub fn with_llm() -> Self {
        Self { use_llm: true }
    }

    // ========================================================================
    // Plan Generation Methods
    // ========================================================================

    /// Generate a plan from the detected steps in a PlanRequest
    ///
    /// Uses rule-based planning to create a TaskPlan with:
    /// - Sequential dependencies (step N depends on step N-1)
    /// - Tool inference based on step description
    /// - Parameter extraction from description
    pub fn generate_plan_from_steps(&self, request: &PlanRequest) -> TaskPlan {
        let plan_id = uuid::Uuid::new_v4().to_string();

        // If no detected steps, create a single step from the original input
        let steps_to_process = if request.detected_steps.is_empty() {
            vec![request.input.text.clone()]
        } else {
            request.detected_steps.clone()
        };

        // Create plan steps with sequential dependencies
        let mut steps: Vec<PlanStep> = Vec::new();
        let mut previous_step_id: Option<String> = None;

        for (index, description) in steps_to_process.iter().enumerate() {
            let step_id = format!("step_{}", index + 1);
            let tool = self.infer_tool(description);
            let parameters = self.extract_parameters(description);

            // Each step depends on the previous one (sequential execution)
            let depends_on = previous_step_id.map(|id| vec![id]).unwrap_or_default();

            let step = PlanStep {
                id: step_id.clone(),
                description: description.clone(),
                tool,
                parameters,
                depends_on,
                status: StepStatus::Pending,
            };

            steps.push(step);
            previous_step_id = Some(step_id);
        }

        // Identify parallel groups (steps that can run simultaneously)
        let parallel_groups = self.identify_parallel_groups(&steps);

        TaskPlan {
            id: plan_id,
            steps,
            parallel_groups,
            current_step_index: 0,
        }
    }

    /// Infer the tool name from step description
    ///
    /// Maps keywords (Chinese and English) to tool names:
    /// - "search" / "搜索" / "查找" -> "search"
    /// - "delete" / "删除" -> "file_delete"
    /// - "copy" / "复制" -> "file_copy"
    /// - "move" / "移动" -> "file_move"
    /// - "create" / "创建" -> "file_write"
    /// - "read" / "读取" -> "file_read"
    /// - "fetch" / "download" / "下载" -> "web_fetch"
    /// - Default -> "chat"
    pub fn infer_tool(&self, description: &str) -> String {
        let desc_lower = description.to_lowercase();

        // Search operations
        if desc_lower.contains("search")
            || description.contains("搜索")
            || description.contains("查找")
        {
            return "search".to_string();
        }

        // Delete operations
        if desc_lower.contains("delete")
            || desc_lower.contains("remove")
            || description.contains("删除")
        {
            return "file_delete".to_string();
        }

        // Copy operations
        if desc_lower.contains("copy") || description.contains("复制") {
            return "file_copy".to_string();
        }

        // Move operations
        if desc_lower.contains("move") || description.contains("移动") {
            return "file_move".to_string();
        }

        // Create/Write operations
        if desc_lower.contains("create")
            || desc_lower.contains("write")
            || description.contains("创建")
            || description.contains("新建")
        {
            return "file_write".to_string();
        }

        // Read operations
        if desc_lower.contains("read")
            || desc_lower.contains("open")
            || description.contains("读取")
            || description.contains("打开")
        {
            return "file_read".to_string();
        }

        // Web fetch/download operations
        if desc_lower.contains("fetch")
            || desc_lower.contains("download")
            || description.contains("下载")
            || description.contains("获取")
        {
            return "web_fetch".to_string();
        }

        // Default to chat for unrecognized operations
        "chat".to_string()
    }

    /// Extract parameters from step description
    ///
    /// Currently extracts:
    /// - The description itself as the input
    /// - File paths if detected (quoted strings or paths)
    /// - URLs if detected
    pub fn extract_parameters(&self, description: &str) -> Value {
        let mut params = serde_json::json!({
            "input": description
        });

        // Try to extract quoted strings (potential file paths or arguments)
        let quoted_strings: Vec<&str> = description
            .split('"')
            .enumerate()
            .filter(|(i, _)| i % 2 == 1)
            .map(|(_, s)| s)
            .collect();

        if !quoted_strings.is_empty() {
            params["arguments"] = serde_json::json!(quoted_strings);
        }

        // Try to extract file paths (patterns like /path/to/file or ~/path)
        let words: Vec<&str> = description.split_whitespace().collect();
        let paths: Vec<&str> = words
            .iter()
            .filter(|w| w.starts_with('/') || w.starts_with("~/") || w.starts_with("./"))
            .copied()
            .collect();

        if !paths.is_empty() {
            params["paths"] = serde_json::json!(paths);
        }

        // Try to extract URLs
        let urls: Vec<&str> = words
            .iter()
            .filter(|w| w.starts_with("http://") || w.starts_with("https://"))
            .copied()
            .collect();

        if !urls.is_empty() {
            params["urls"] = serde_json::json!(urls);
        }

        params
    }

    /// Identify groups of steps that can run in parallel
    ///
    /// Steps can run in parallel if they:
    /// - Have no dependencies on each other
    /// - Their dependencies are already completed
    ///
    /// Returns a list of step ID groups, where each group can execute simultaneously
    pub fn identify_parallel_groups(&self, steps: &[PlanStep]) -> Vec<Vec<String>> {
        let mut groups: Vec<Vec<String>> = Vec::new();

        // Simple algorithm: group steps by their dependency depth
        // Steps with no dependencies are depth 0
        // Steps depending on depth N steps are depth N+1

        for step in steps {
            // Calculate the depth based on dependencies
            let depth = if step.depends_on.is_empty() {
                0
            } else {
                // Find the max depth of dependencies and add 1
                step.depends_on
                    .iter()
                    .filter_map(|dep_id| steps.iter().position(|s| &s.id == dep_id))
                    .map(|_pos| {
                        // For sequential dependencies, depth = position
                        step.depends_on.len()
                    })
                    .max()
                    .unwrap_or(0)
            };

            // Ensure we have enough groups
            while groups.len() <= depth {
                groups.push(Vec::new());
            }

            groups[depth].push(step.id.clone());
        }

        // Filter out empty groups
        groups.into_iter().filter(|g| !g.is_empty()).collect()
    }
}

// ============================================================================
// EventHandler Implementation
// ============================================================================

#[async_trait]
impl EventHandler for TaskPlanner {
    fn name(&self) -> &'static str {
        "TaskPlanner"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::PlanRequested]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Only handle PlanRequested events
        let request = match event {
            AlephEvent::PlanRequested(req) => req,
            _ => return Ok(vec![]),
        };

        // Generate plan based on mode
        let plan = if self.use_llm {
            // TODO: Implement LLM-based planning
            // For now, fall back to rule-based planning
            self.generate_plan_from_steps(request)
        } else {
            self.generate_plan_from_steps(request)
        };

        // Return PlanCreated event
        Ok(vec![AlephEvent::PlanCreated(plan)])
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventBus, InputEvent, StopReason};

    fn create_test_input(text: &str) -> InputEvent {
        InputEvent {
            text: text.to_string(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    fn create_plan_request(text: &str, steps: Vec<&str>) -> PlanRequest {
        PlanRequest {
            input: create_test_input(text),
            intent_type: None,
            detected_steps: steps.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    // ========================================================================
    // Plan Generation Tests
    // ========================================================================

    #[test]
    fn test_generate_plan_single_step() {
        let planner = TaskPlanner::new();
        let request = create_plan_request("打开文件", vec!["打开文件"]);

        let plan = planner.generate_plan_from_steps(&request);

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].description, "打开文件");
        assert!(plan.steps[0].depends_on.is_empty());
        assert_eq!(plan.steps[0].status, StepStatus::Pending);
        assert_eq!(plan.current_step_index, 0);
    }

    #[test]
    fn test_generate_plan_multiple_steps() {
        let planner = TaskPlanner::new();
        let request = create_plan_request(
            "打开文件然后复制内容接着保存",
            vec!["打开文件", "复制内容", "保存"],
        );

        let plan = planner.generate_plan_from_steps(&request);

        assert_eq!(plan.steps.len(), 3);

        // First step has no dependencies
        assert!(plan.steps[0].depends_on.is_empty());

        // Second step depends on first
        assert_eq!(plan.steps[1].depends_on, vec!["step_1"]);

        // Third step depends on second
        assert_eq!(plan.steps[2].depends_on, vec!["step_2"]);

        // All steps should be pending
        for step in &plan.steps {
            assert_eq!(step.status, StepStatus::Pending);
        }
    }

    #[test]
    fn test_generate_plan_empty_steps_uses_input() {
        let planner = TaskPlanner::new();
        let request = create_plan_request("搜索文件", vec![]);

        let plan = planner.generate_plan_from_steps(&request);

        // Should create a single step from the original input
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].description, "搜索文件");
    }

    // ========================================================================
    // Tool Inference Tests
    // ========================================================================

    #[test]
    fn test_infer_tool_search() {
        let planner = TaskPlanner::new();

        // English
        assert_eq!(planner.infer_tool("search for files"), "search");
        assert_eq!(planner.infer_tool("Search the document"), "search");

        // Chinese
        assert_eq!(planner.infer_tool("搜索文件"), "search");
        assert_eq!(planner.infer_tool("查找文档"), "search");
    }

    #[test]
    fn test_infer_tool_file_ops() {
        let planner = TaskPlanner::new();

        // Delete
        assert_eq!(planner.infer_tool("delete the file"), "file_delete");
        assert_eq!(planner.infer_tool("删除文件"), "file_delete");
        assert_eq!(planner.infer_tool("remove old files"), "file_delete");

        // Copy
        assert_eq!(planner.infer_tool("copy the file"), "file_copy");
        assert_eq!(planner.infer_tool("复制文件到桌面"), "file_copy");

        // Move
        assert_eq!(planner.infer_tool("move file to folder"), "file_move");
        assert_eq!(planner.infer_tool("移动到下载文件夹"), "file_move");

        // Create/Write
        assert_eq!(planner.infer_tool("create a new file"), "file_write");
        assert_eq!(planner.infer_tool("创建目录"), "file_write");
        assert_eq!(planner.infer_tool("新建文件夹"), "file_write");
        assert_eq!(planner.infer_tool("write content to file"), "file_write");

        // Read
        assert_eq!(planner.infer_tool("read the config file"), "file_read");
        assert_eq!(planner.infer_tool("读取配置"), "file_read");
        assert_eq!(planner.infer_tool("open the document"), "file_read");
        assert_eq!(planner.infer_tool("打开文件"), "file_read");
    }

    #[test]
    fn test_infer_tool_web_fetch() {
        let planner = TaskPlanner::new();

        assert_eq!(planner.infer_tool("fetch the url"), "web_fetch");
        assert_eq!(planner.infer_tool("download the file"), "web_fetch");
        assert_eq!(planner.infer_tool("下载图片"), "web_fetch");
        assert_eq!(planner.infer_tool("获取网页内容"), "web_fetch");
    }

    #[test]
    fn test_infer_tool_default_chat() {
        let planner = TaskPlanner::new();

        // Unrecognized operations should default to chat
        assert_eq!(planner.infer_tool("help me understand this"), "chat");
        assert_eq!(planner.infer_tool("what is the weather"), "chat");
        assert_eq!(planner.infer_tool("解释这段代码"), "chat");
    }

    // ========================================================================
    // Parameter Extraction Tests
    // ========================================================================

    #[test]
    fn test_extract_parameters_basic() {
        let planner = TaskPlanner::new();

        let params = planner.extract_parameters("search for files");

        assert_eq!(params["input"], "search for files");
    }

    #[test]
    fn test_extract_parameters_with_path() {
        let planner = TaskPlanner::new();

        let params = planner.extract_parameters("copy file from /home/user/doc.txt");

        assert!(params["paths"].is_array());
        let paths = params["paths"].as_array().unwrap();
        assert!(paths
            .iter()
            .any(|p| p.as_str() == Some("/home/user/doc.txt")));
    }

    #[test]
    fn test_extract_parameters_with_url() {
        let planner = TaskPlanner::new();

        let params = planner.extract_parameters("fetch https://example.com/api");

        assert!(params["urls"].is_array());
        let urls = params["urls"].as_array().unwrap();
        assert!(urls
            .iter()
            .any(|u| u.as_str() == Some("https://example.com/api")));
    }

    #[test]
    fn test_extract_parameters_with_quotes() {
        let planner = TaskPlanner::new();

        let params = planner.extract_parameters("rename \"old name\" to \"new name\"");

        assert!(params["arguments"].is_array());
        let args = params["arguments"].as_array().unwrap();
        assert!(args.iter().any(|a| a.as_str() == Some("old name")));
        assert!(args.iter().any(|a| a.as_str() == Some("new name")));
    }

    // ========================================================================
    // Parallel Groups Tests
    // ========================================================================

    #[test]
    fn test_identify_parallel_groups_sequential() {
        let planner = TaskPlanner::new();

        let steps = vec![
            PlanStep {
                id: "step_1".to_string(),
                description: "first".to_string(),
                tool: "chat".to_string(),
                parameters: serde_json::json!({}),
                depends_on: vec![],
                status: StepStatus::Pending,
            },
            PlanStep {
                id: "step_2".to_string(),
                description: "second".to_string(),
                tool: "chat".to_string(),
                parameters: serde_json::json!({}),
                depends_on: vec!["step_1".to_string()],
                status: StepStatus::Pending,
            },
            PlanStep {
                id: "step_3".to_string(),
                description: "third".to_string(),
                tool: "chat".to_string(),
                parameters: serde_json::json!({}),
                depends_on: vec!["step_2".to_string()],
                status: StepStatus::Pending,
            },
        ];

        let groups = planner.identify_parallel_groups(&steps);

        // Each step should be in its own group (no parallelism for sequential deps)
        assert!(!groups.is_empty());
        // First group should contain step_1
        assert!(groups[0].contains(&"step_1".to_string()));
    }

    #[test]
    fn test_identify_parallel_groups_independent() {
        let planner = TaskPlanner::new();

        // Two independent steps (no dependencies)
        let steps = vec![
            PlanStep {
                id: "step_1".to_string(),
                description: "first".to_string(),
                tool: "chat".to_string(),
                parameters: serde_json::json!({}),
                depends_on: vec![],
                status: StepStatus::Pending,
            },
            PlanStep {
                id: "step_2".to_string(),
                description: "second".to_string(),
                tool: "chat".to_string(),
                parameters: serde_json::json!({}),
                depends_on: vec![],
                status: StepStatus::Pending,
            },
        ];

        let groups = planner.identify_parallel_groups(&steps);

        // Both steps should be in the same group (can run in parallel)
        assert_eq!(groups.len(), 1);
        assert!(groups[0].contains(&"step_1".to_string()));
        assert!(groups[0].contains(&"step_2".to_string()));
    }

    // ========================================================================
    // EventHandler Implementation Tests
    // ========================================================================

    #[test]
    fn test_handler_name() {
        let planner = TaskPlanner::new();
        assert_eq!(planner.name(), "TaskPlanner");
    }

    #[test]
    fn test_handler_subscriptions() {
        let planner = TaskPlanner::new();
        let subs = planner.subscriptions();

        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0], EventType::PlanRequested);
    }

    #[tokio::test]
    async fn test_handler_ignores_other_events() {
        let planner = TaskPlanner::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // LoopStop event should be ignored
        let event = AlephEvent::LoopStop(StopReason::Completed);
        let result = planner.handle(&event, &ctx).await.unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_processes_plan_request() {
        let planner = TaskPlanner::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let request = create_plan_request("打开文件然后复制", vec!["打开文件", "复制内容"]);
        let event = AlephEvent::PlanRequested(request);
        let result = planner.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AlephEvent::PlanCreated(_)));

        if let AlephEvent::PlanCreated(plan) = &result[0] {
            assert_eq!(plan.steps.len(), 2);
            assert!(!plan.id.is_empty());
        }
    }

    #[tokio::test]
    async fn test_handler_infers_correct_tools() {
        let planner = TaskPlanner::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let request = create_plan_request("搜索文件然后删除", vec!["搜索配置文件", "删除旧文件"]);
        let event = AlephEvent::PlanRequested(request);
        let result = planner.handle(&event, &ctx).await.unwrap();

        if let AlephEvent::PlanCreated(plan) = &result[0] {
            assert_eq!(plan.steps[0].tool, "search");
            assert_eq!(plan.steps[1].tool, "file_delete");
        } else {
            panic!("Expected PlanCreated event");
        }
    }

    // ========================================================================
    // Builder Pattern Tests
    // ========================================================================

    #[test]
    fn test_new_uses_rule_based_planning() {
        let planner = TaskPlanner::new();
        assert!(!planner.use_llm);
    }

    #[test]
    fn test_with_llm_enables_llm_planning() {
        let planner = TaskPlanner::with_llm();
        assert!(planner.use_llm);
    }

    #[test]
    fn test_default_impl() {
        let planner = TaskPlanner::default();
        assert!(!planner.use_llm);
    }
}
