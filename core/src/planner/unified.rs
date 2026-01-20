//! Unified planner implementation
//!
//! This module implements the `UnifiedPlanner` that generates execution plans
//! from user input using an LLM-based planning approach.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::time::timeout;
use tracing::{debug, info};

use super::prompt::{build_planning_prompt, get_system_prompt_with_tools, ToolInfo};
use super::types::{ExecutionPlan, PlannedTask, PlannerError};
use crate::cowork::types::{AiTask, AppAuto, CodeExec, DocGen, FileOp, Language, TaskType};
use crate::providers::AiProvider;

/// Configuration for the unified planner
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Timeout for LLM planning calls
    pub timeout: Duration,
    /// Whether to require confirmation for destructive operations
    pub require_confirmation_for_destructive: bool,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            require_confirmation_for_destructive: true,
        }
    }
}

/// Unified planner that generates execution plans from user input
///
/// The planner uses an LLM to analyze user requests and determine the
/// appropriate execution strategy: conversational, single action, or task graph.
pub struct UnifiedPlanner {
    provider: Arc<dyn AiProvider>,
    config: PlannerConfig,
    tools: Vec<ToolInfo>,
}

impl UnifiedPlanner {
    /// Create a new planner with default configuration
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            config: PlannerConfig::default(),
            tools: Vec::new(),
        }
    }

    /// Create a new planner with custom configuration
    pub fn with_config(provider: Arc<dyn AiProvider>, config: PlannerConfig) -> Self {
        Self {
            provider,
            config,
            tools: Vec::new(),
        }
    }

    /// Builder: set available tools
    pub fn with_tools(mut self, tools: Vec<ToolInfo>) -> Self {
        self.tools = tools;
        self
    }

    /// Set available tools
    pub fn set_tools(&mut self, tools: Vec<ToolInfo>) {
        self.tools = tools;
    }

    /// Plan an execution strategy for user input
    ///
    /// This method:
    /// 1. Builds prompts for the LLM with tool information
    /// 2. Calls the LLM with a timeout
    /// 3. Parses the response into an ExecutionPlan
    ///
    /// # Arguments
    ///
    /// * `user_input` - The user's request to plan for
    ///
    /// # Returns
    ///
    /// * `Ok(ExecutionPlan)` - The generated execution plan
    /// * `Err(PlannerError)` - Planning failed (timeout, LLM error, or parse error)
    pub async fn plan(&self, user_input: &str) -> Result<ExecutionPlan, PlannerError> {
        info!("Planning execution for: {}", user_input);

        // Build prompts
        let system_prompt = get_system_prompt_with_tools(&self.tools);
        let user_prompt = build_planning_prompt(user_input, "");

        debug!("System prompt length: {}", system_prompt.len());
        debug!("User prompt: {}", user_prompt);

        // Call LLM with timeout
        let result = timeout(
            self.config.timeout,
            self.provider.process(&user_prompt, Some(&system_prompt)),
        )
        .await;

        let response = match result {
            Ok(Ok(response)) => {
                info!(response_len = response.len(), "Planner LLM response received");
                debug!("LLM response content: {}", response);
                response
            }
            Ok(Err(e)) => {
                return Err(PlannerError::llm_error(e.to_string()));
            }
            Err(_) => {
                return Err(PlannerError::Timeout);
            }
        };

        // Parse response
        self.parse_response(&response)
    }

    /// Parse LLM response into an ExecutionPlan
    fn parse_response(&self, response: &str) -> Result<ExecutionPlan, PlannerError> {
        // Extract JSON from response
        let json_str = extract_json(response).map_err(|e| PlannerError::parse_error(e))?;

        info!(json_len = json_str.len(), "Extracted plan JSON from response");
        debug!("Extracted JSON content: {}", json_str);

        // Parse as RawPlanResponse
        let raw: RawPlanResponse =
            serde_json::from_str(&json_str).map_err(|e| PlannerError::parse_error(e.to_string()))?;

        self.convert_raw_plan(raw)
    }

    /// Convert raw JSON plan to ExecutionPlan
    fn convert_raw_plan(&self, raw: RawPlanResponse) -> Result<ExecutionPlan, PlannerError> {
        match raw.plan_type.as_str() {
            "conversational" => Ok(ExecutionPlan::Conversational {
                enhanced_prompt: raw.enhanced_prompt,
            }),

            "single_action" => {
                let tool_name = raw
                    .tool_name
                    .ok_or_else(|| PlannerError::parse_error("Missing tool_name for single_action"))?;
                let parameters = raw.parameters.unwrap_or(serde_json::Value::Null);
                let requires_confirmation = raw.requires_confirmation.unwrap_or(false);

                info!(
                    tool_name = %tool_name,
                    parameters = %parameters,
                    "Planner decided: single_action"
                );

                // Apply config override for destructive operations
                let requires_confirmation = if self.config.require_confirmation_for_destructive {
                    requires_confirmation || is_destructive_tool(&tool_name)
                } else {
                    requires_confirmation
                };

                Ok(ExecutionPlan::SingleAction {
                    tool_name,
                    parameters,
                    requires_confirmation,
                })
            }

            "task_graph" => {
                let raw_tasks = raw
                    .tasks
                    .ok_or_else(|| PlannerError::parse_error("Missing tasks for task_graph"))?;

                let mut tasks = Vec::with_capacity(raw_tasks.len());
                for raw_task in raw_tasks {
                    tasks.push(self.convert_raw_task(raw_task)?);
                }

                let dependencies = raw.dependencies.unwrap_or_default();
                let mut requires_confirmation = raw.requires_confirmation.unwrap_or(false);

                // Apply config override for destructive operations in task graph
                if self.config.require_confirmation_for_destructive {
                    for task in &tasks {
                        if is_destructive_task_type(&task.task_type) {
                            requires_confirmation = true;
                            break;
                        }
                    }
                }

                Ok(ExecutionPlan::TaskGraph {
                    tasks,
                    dependencies,
                    requires_confirmation,
                })
            }

            unknown => Err(PlannerError::parse_error(format!(
                "Unknown plan type: {}",
                unknown
            ))),
        }
    }

    /// Convert a raw task to PlannedTask
    fn convert_raw_task(&self, raw: RawTask) -> Result<PlannedTask, PlannerError> {
        let task_type = parse_task_type(&raw.task_type)?;

        Ok(PlannedTask {
            id: raw.id,
            description: raw.description,
            task_type,
            tool_hint: raw.tool_hint,
            parameters: serde_json::Value::Null,
        })
    }
}

/// Check if a tool name represents a destructive operation
fn is_destructive_tool(tool_name: &str) -> bool {
    let destructive_tools = [
        "delete",
        "remove",
        "rm",
        "unlink",
        "drop",
        "truncate",
        "destroy",
        "wipe",
        "erase",
        "purge",
    ];

    let tool_lower = tool_name.to_lowercase();
    destructive_tools
        .iter()
        .any(|d| tool_lower.contains(d))
}

/// Check if a task type represents a destructive operation
fn is_destructive_task_type(task_type: &TaskType) -> bool {
    matches!(task_type, TaskType::FileOperation(FileOp::Delete { .. }))
}

/// Extract JSON from response (may be wrapped in markdown code blocks)
///
/// Handles various formats:
/// - Direct JSON: `{"type": "conversational"}`
/// - Code block: ` ```json\n{...}\n``` `
/// - Code block without language: ` ```\n{...}\n``` `
/// - Embedded JSON: `Here's the plan: {...} Done!`
fn extract_json(response: &str) -> Result<String, String> {
    let trimmed = response.trim();

    // Try ```json block
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7; // Skip "```json"
        if let Some(end) = trimmed[json_start..].find("```") {
            let json_content = trimmed[json_start..json_start + end].trim();
            return Ok(json_content.to_string());
        }
    }

    // Try ``` block without language specifier
    if let Some(start) = trimmed.find("```") {
        // Make sure it's not ```json (already handled above)
        let after_ticks = &trimmed[start + 3..];
        if !after_ticks.starts_with("json") {
            // Find the content between ``` markers
            if let Some(end) = after_ticks.find("```") {
                // Skip any language identifier on the first line
                let content = &after_ticks[..end];
                let json_content = if let Some(newline_pos) = content.find('\n') {
                    content[newline_pos + 1..].trim()
                } else {
                    content.trim()
                };
                // Validate it looks like JSON
                if json_content.starts_with('{') {
                    return Ok(json_content.to_string());
                }
            }
        }
    }

    // Try direct JSON (starts with {)
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Ok(trimmed.to_string());
    }

    // Find first { and last }
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if start < end {
                return Ok(trimmed[start..=end].to_string());
            }
        }
    }

    Err("No valid JSON found in response".to_string())
}

/// Parse task type from JSON value
///
/// Handles the following task types:
/// - file_operation: read, write, move, copy, delete, search, list
/// - code_execution: script, command, file
/// - document_generation: excel, powerpoint, pdf, markdown
/// - app_automation: launch, apple_script, ui_action
/// - ai_inference: default AI processing task
fn parse_task_type(value: &serde_json::Value) -> Result<TaskType, PlannerError> {
    // Handle string format: "file_operation" or "ai_inference"
    if let Some(type_str) = value.as_str() {
        return match type_str {
            "file_operation" => Ok(TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("."),
            })),
            "code_execution" => Ok(TaskType::CodeExecution(CodeExec::Command {
                cmd: String::new(),
                args: Vec::new(),
            })),
            "document_generation" => Ok(TaskType::DocumentGeneration(DocGen::Markdown {
                output: PathBuf::from("output.md"),
            })),
            "app_automation" => Ok(TaskType::AppAutomation(AppAuto::Launch {
                bundle_id: String::new(),
            })),
            "ai_inference" | _ => Ok(TaskType::AiInference(AiTask {
                prompt: String::new(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            })),
        };
    }

    // Handle object format with "type" field
    let obj = value.as_object().ok_or_else(|| {
        PlannerError::parse_error("task_type must be string or object")
    })?;

    let type_field = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PlannerError::parse_error("task_type object missing 'type' field"))?;

    match type_field {
        "file_operation" => parse_file_operation(obj),
        "code_execution" => parse_code_execution(obj),
        "document_generation" => parse_document_generation(obj),
        "app_automation" => parse_app_automation(obj),
        "ai_inference" => parse_ai_inference(obj),
        _ => {
            // Default to AI inference for unknown types
            Ok(TaskType::AiInference(AiTask {
                prompt: String::new(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }))
        }
    }
}

/// Parse file operation from JSON object
fn parse_file_operation(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<TaskType, PlannerError> {
    let op = obj.get("op").and_then(|v| v.as_str()).unwrap_or("list");

    let file_op = match op {
        "read" => {
            let path = get_path_from_obj(obj, "path")?;
            FileOp::Read { path }
        }
        "write" => {
            let path = get_path_from_obj(obj, "path")?;
            FileOp::Write { path }
        }
        "move" => {
            let from = get_path_from_obj(obj, "from")?;
            let to = get_path_from_obj(obj, "to")?;
            FileOp::Move { from, to }
        }
        "copy" => {
            let from = get_path_from_obj(obj, "from")?;
            let to = get_path_from_obj(obj, "to")?;
            FileOp::Copy { from, to }
        }
        "delete" => {
            let path = get_path_from_obj(obj, "path")?;
            FileOp::Delete { path }
        }
        "search" => {
            let pattern = obj
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("*")
                .to_string();
            let dir = get_path_from_obj(obj, "dir").unwrap_or_else(|_| PathBuf::from("."));
            FileOp::Search { pattern, dir }
        }
        "list" | _ => {
            let path = get_path_from_obj(obj, "path").unwrap_or_else(|_| PathBuf::from("."));
            FileOp::List { path }
        }
    };

    Ok(TaskType::FileOperation(file_op))
}

/// Parse code execution from JSON object
fn parse_code_execution(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<TaskType, PlannerError> {
    let exec = obj.get("exec").and_then(|v| v.as_str()).unwrap_or("command");

    let code_exec = match exec {
        "script" => {
            let code = obj
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let language = parse_language(obj.get("language"));
            CodeExec::Script { code, language }
        }
        "file" => {
            let path = get_path_from_obj(obj, "path")?;
            CodeExec::File { path }
        }
        "command" | _ => {
            let cmd = obj
                .get("cmd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args = obj
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            CodeExec::Command { cmd, args }
        }
    };

    Ok(TaskType::CodeExecution(code_exec))
}

/// Parse document generation from JSON object
fn parse_document_generation(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<TaskType, PlannerError> {
    let format = obj
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("markdown");

    let doc_gen = match format {
        "excel" => {
            let template = obj.get("template").and_then(|v| v.as_str()).map(PathBuf::from);
            let output = get_path_from_obj(obj, "output")?;
            DocGen::Excel { template, output }
        }
        "powerpoint" => {
            let template = obj.get("template").and_then(|v| v.as_str()).map(PathBuf::from);
            let output = get_path_from_obj(obj, "output")?;
            DocGen::PowerPoint { template, output }
        }
        "pdf" => {
            let style = obj.get("style").and_then(|v| v.as_str()).map(String::from);
            let output = get_path_from_obj(obj, "output")?;
            DocGen::Pdf { style, output }
        }
        "markdown" | _ => {
            let output =
                get_path_from_obj(obj, "output").unwrap_or_else(|_| PathBuf::from("output.md"));
            DocGen::Markdown { output }
        }
    };

    Ok(TaskType::DocumentGeneration(doc_gen))
}

/// Parse app automation from JSON object
fn parse_app_automation(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<TaskType, PlannerError> {
    let auto_type = obj
        .get("auto_type")
        .and_then(|v| v.as_str())
        .unwrap_or("launch");

    let app_auto = match auto_type {
        "launch" => {
            let bundle_id = obj
                .get("bundle_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            AppAuto::Launch { bundle_id }
        }
        "apple_script" => {
            let script = obj
                .get("script")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            AppAuto::AppleScript { script }
        }
        "ui_action" | _ => {
            let ui_action = obj
                .get("ui_action")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let target = obj
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            AppAuto::UiAction { ui_action, target }
        }
    };

    Ok(TaskType::AppAutomation(app_auto))
}

/// Parse AI inference from JSON object
fn parse_ai_inference(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<TaskType, PlannerError> {
    let prompt = obj
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let requires_privacy = obj
        .get("requires_privacy")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_images = obj
        .get("has_images")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let output_format = obj.get("output_format").and_then(|v| v.as_str()).map(String::from);

    Ok(TaskType::AiInference(AiTask {
        prompt,
        requires_privacy,
        has_images,
        output_format,
    }))
}

/// Parse language from JSON value
fn parse_language(value: Option<&serde_json::Value>) -> Language {
    let lang_str = value.and_then(|v| v.as_str()).unwrap_or("shell");

    match lang_str.to_lowercase().as_str() {
        "python" => Language::Python,
        "javascript" | "js" => Language::JavaScript,
        "ruby" => Language::Ruby,
        "rust" => Language::Rust,
        "shell" | "bash" | "sh" | _ => Language::Shell,
    }
}

/// Get a path from JSON object
fn get_path_from_obj(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<PathBuf, PlannerError> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| PlannerError::parse_error(format!("Missing required field: {}", key)))
}

// JSON parsing structures

/// Raw plan response from LLM
#[derive(Debug, Deserialize)]
struct RawPlanResponse {
    #[serde(rename = "type")]
    plan_type: String,
    enhanced_prompt: Option<String>,
    tool_name: Option<String>,
    parameters: Option<serde_json::Value>,
    tasks: Option<Vec<RawTask>>,
    dependencies: Option<Vec<(usize, usize)>>,
    requires_confirmation: Option<bool>,
}

/// Raw task in plan response
#[derive(Debug, Deserialize)]
struct RawTask {
    id: usize,
    description: String,
    #[serde(rename = "task_type")]
    task_type: serde_json::Value,
    tool_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planner_config_default() {
        let config = PlannerConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.require_confirmation_for_destructive);
    }

    #[test]
    fn test_extract_json_direct() {
        let response = r#"{"type": "conversational"}"#;
        let result = extract_json(response);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), r#"{"type": "conversational"}"#);
    }

    #[test]
    fn test_extract_json_code_block() {
        let response = r#"Here's the plan:
```json
{"type": "single_action", "tool_name": "read_file"}
```
Done!"#;
        let result = extract_json(response);
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("single_action"));
    }

    #[test]
    fn test_extract_json_code_block_no_language() {
        let response = r#"Result:
```
{"type": "conversational"}
```"#;
        let result = extract_json(response);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("conversational"));
    }

    #[test]
    fn test_extract_json_embedded() {
        let response = r#"Here's my analysis: {"type": "conversational"} That's it!"#;
        let result = extract_json(response);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("conversational"));
    }

    #[test]
    fn test_extract_json_no_json() {
        let response = "No JSON here at all";
        let result = extract_json(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_task_type_string_file_operation() {
        let value = serde_json::json!("file_operation");
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), TaskType::FileOperation(_)));
    }

    #[test]
    fn test_parse_task_type_string_code_execution() {
        let value = serde_json::json!("code_execution");
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), TaskType::CodeExecution(_)));
    }

    #[test]
    fn test_parse_task_type_string_document_generation() {
        let value = serde_json::json!("document_generation");
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), TaskType::DocumentGeneration(_)));
    }

    #[test]
    fn test_parse_task_type_string_app_automation() {
        let value = serde_json::json!("app_automation");
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), TaskType::AppAutomation(_)));
    }

    #[test]
    fn test_parse_task_type_string_ai_inference() {
        let value = serde_json::json!("ai_inference");
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), TaskType::AiInference(_)));
    }

    #[test]
    fn test_parse_task_type_string_unknown_defaults_to_ai() {
        let value = serde_json::json!("unknown_type");
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), TaskType::AiInference(_)));
    }

    #[test]
    fn test_parse_task_type_object_file_read() {
        let value = serde_json::json!({
            "type": "file_operation",
            "op": "read",
            "path": "/tmp/test.txt"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        if let TaskType::FileOperation(FileOp::Read { path }) = result.unwrap() {
            assert_eq!(path, PathBuf::from("/tmp/test.txt"));
        } else {
            panic!("Expected FileOperation::Read");
        }
    }

    #[test]
    fn test_parse_task_type_object_file_delete() {
        let value = serde_json::json!({
            "type": "file_operation",
            "op": "delete",
            "path": "/tmp/delete_me.txt"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            TaskType::FileOperation(FileOp::Delete { .. })
        ));
    }

    #[test]
    fn test_parse_task_type_object_code_script() {
        let value = serde_json::json!({
            "type": "code_execution",
            "exec": "script",
            "code": "print('hello')",
            "language": "python"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        if let TaskType::CodeExecution(CodeExec::Script { code, language }) = result.unwrap() {
            assert_eq!(code, "print('hello')");
            assert_eq!(language, Language::Python);
        } else {
            panic!("Expected CodeExecution::Script");
        }
    }

    #[test]
    fn test_parse_task_type_object_code_command() {
        let value = serde_json::json!({
            "type": "code_execution",
            "exec": "command",
            "cmd": "ls",
            "args": ["-la", "/tmp"]
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        if let TaskType::CodeExecution(CodeExec::Command { cmd, args }) = result.unwrap() {
            assert_eq!(cmd, "ls");
            assert_eq!(args, vec!["-la", "/tmp"]);
        } else {
            panic!("Expected CodeExecution::Command");
        }
    }

    #[test]
    fn test_parse_task_type_object_doc_excel() {
        let value = serde_json::json!({
            "type": "document_generation",
            "format": "excel",
            "output": "/tmp/report.xlsx"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            TaskType::DocumentGeneration(DocGen::Excel { .. })
        ));
    }

    #[test]
    fn test_parse_task_type_object_doc_pdf() {
        let value = serde_json::json!({
            "type": "document_generation",
            "format": "pdf",
            "output": "/tmp/doc.pdf",
            "style": "formal"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        if let TaskType::DocumentGeneration(DocGen::Pdf { style, output }) = result.unwrap() {
            assert_eq!(style, Some("formal".to_string()));
            assert_eq!(output, PathBuf::from("/tmp/doc.pdf"));
        } else {
            panic!("Expected DocumentGeneration::Pdf");
        }
    }

    #[test]
    fn test_parse_task_type_object_app_launch() {
        let value = serde_json::json!({
            "type": "app_automation",
            "auto_type": "launch",
            "bundle_id": "com.apple.Safari"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        if let TaskType::AppAutomation(AppAuto::Launch { bundle_id }) = result.unwrap() {
            assert_eq!(bundle_id, "com.apple.Safari");
        } else {
            panic!("Expected AppAutomation::Launch");
        }
    }

    #[test]
    fn test_parse_task_type_object_app_applescript() {
        let value = serde_json::json!({
            "type": "app_automation",
            "auto_type": "apple_script",
            "script": "tell application \"Finder\" to activate"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            TaskType::AppAutomation(AppAuto::AppleScript { .. })
        ));
    }

    #[test]
    fn test_parse_task_type_object_ai_inference() {
        let value = serde_json::json!({
            "type": "ai_inference",
            "prompt": "Summarize this text",
            "requires_privacy": true,
            "has_images": false,
            "output_format": "json"
        });
        let result = parse_task_type(&value);
        assert!(result.is_ok());
        if let TaskType::AiInference(task) = result.unwrap() {
            assert_eq!(task.prompt, "Summarize this text");
            assert!(task.requires_privacy);
            assert!(!task.has_images);
            assert_eq!(task.output_format, Some("json".to_string()));
        } else {
            panic!("Expected AiInference");
        }
    }

    #[test]
    fn test_parse_language() {
        assert_eq!(parse_language(Some(&serde_json::json!("python"))), Language::Python);
        assert_eq!(parse_language(Some(&serde_json::json!("javascript"))), Language::JavaScript);
        assert_eq!(parse_language(Some(&serde_json::json!("js"))), Language::JavaScript);
        assert_eq!(parse_language(Some(&serde_json::json!("ruby"))), Language::Ruby);
        assert_eq!(parse_language(Some(&serde_json::json!("rust"))), Language::Rust);
        assert_eq!(parse_language(Some(&serde_json::json!("shell"))), Language::Shell);
        assert_eq!(parse_language(Some(&serde_json::json!("bash"))), Language::Shell);
        assert_eq!(parse_language(None), Language::Shell);
    }

    #[test]
    fn test_is_destructive_tool() {
        assert!(is_destructive_tool("delete_file"));
        assert!(is_destructive_tool("remove_directory"));
        assert!(is_destructive_tool("rm"));
        assert!(is_destructive_tool("DROP_TABLE"));
        assert!(!is_destructive_tool("read_file"));
        assert!(!is_destructive_tool("write_file"));
        assert!(!is_destructive_tool("list_directory"));
    }

    #[test]
    fn test_is_destructive_task_type() {
        let delete = TaskType::FileOperation(FileOp::Delete {
            path: PathBuf::from("/tmp/test"),
        });
        assert!(is_destructive_task_type(&delete));

        let read = TaskType::FileOperation(FileOp::Read {
            path: PathBuf::from("/tmp/test"),
        });
        assert!(!is_destructive_task_type(&read));

        let ai = TaskType::AiInference(AiTask {
            prompt: String::new(),
            requires_privacy: false,
            has_images: false,
            output_format: None,
        });
        assert!(!is_destructive_task_type(&ai));
    }

    #[test]
    fn test_parse_response_conversational() {
        let provider = crate::providers::create_mock_provider();
        let planner = UnifiedPlanner::new(provider);

        let response = r#"{"type": "conversational", "enhanced_prompt": "Let me help you"}"#;
        let result = planner.parse_response(response);

        assert!(result.is_ok());
        if let ExecutionPlan::Conversational { enhanced_prompt } = result.unwrap() {
            assert_eq!(enhanced_prompt, Some("Let me help you".to_string()));
        } else {
            panic!("Expected Conversational plan");
        }
    }

    #[test]
    fn test_parse_response_single_action() {
        let provider = crate::providers::create_mock_provider();
        let planner = UnifiedPlanner::new(provider);

        let response = r#"{"type": "single_action", "tool_name": "read_file", "parameters": {"path": "/tmp/test.txt"}}"#;
        let result = planner.parse_response(response);

        assert!(result.is_ok());
        if let ExecutionPlan::SingleAction {
            tool_name,
            parameters,
            requires_confirmation,
        } = result.unwrap()
        {
            assert_eq!(tool_name, "read_file");
            assert_eq!(parameters["path"], "/tmp/test.txt");
            assert!(!requires_confirmation);
        } else {
            panic!("Expected SingleAction plan");
        }
    }

    #[test]
    fn test_parse_response_single_action_destructive() {
        let provider = crate::providers::create_mock_provider();
        let planner = UnifiedPlanner::new(provider);

        let response = r#"{"type": "single_action", "tool_name": "delete_file", "parameters": {"path": "/tmp/test.txt"}}"#;
        let result = planner.parse_response(response);

        assert!(result.is_ok());
        if let ExecutionPlan::SingleAction {
            requires_confirmation,
            ..
        } = result.unwrap()
        {
            // Should be true due to config default
            assert!(requires_confirmation);
        } else {
            panic!("Expected SingleAction plan");
        }
    }

    #[test]
    fn test_parse_response_task_graph() {
        let provider = crate::providers::create_mock_provider();
        let planner = UnifiedPlanner::new(provider);

        let response = r#"{
            "type": "task_graph",
            "tasks": [
                {"id": 0, "description": "Read config", "task_type": "file_operation"},
                {"id": 1, "description": "Process data", "task_type": "ai_inference"}
            ],
            "dependencies": [[1, 0]]
        }"#;
        let result = planner.parse_response(response);

        assert!(result.is_ok());
        if let ExecutionPlan::TaskGraph {
            tasks,
            dependencies,
            ..
        } = result.unwrap()
        {
            assert_eq!(tasks.len(), 2);
            assert_eq!(tasks[0].id, 0);
            assert_eq!(tasks[0].description, "Read config");
            assert_eq!(tasks[1].id, 1);
            assert_eq!(dependencies, vec![(1, 0)]);
        } else {
            panic!("Expected TaskGraph plan");
        }
    }

    #[test]
    fn test_parse_response_invalid_json() {
        let provider = crate::providers::create_mock_provider();
        let planner = UnifiedPlanner::new(provider);

        let response = "not valid json at all";
        let result = planner.parse_response(response);

        assert!(result.is_err());
        assert!(matches!(result, Err(PlannerError::ParseError(_))));
    }

    #[test]
    fn test_parse_response_missing_type() {
        let provider = crate::providers::create_mock_provider();
        let planner = UnifiedPlanner::new(provider);

        let response = r#"{"tool_name": "read_file"}"#;
        let result = planner.parse_response(response);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_response_unknown_type() {
        let provider = crate::providers::create_mock_provider();
        let planner = UnifiedPlanner::new(provider);

        let response = r#"{"type": "unknown_plan_type"}"#;
        let result = planner.parse_response(response);

        assert!(result.is_err());
        if let Err(PlannerError::ParseError(msg)) = result {
            assert!(msg.contains("Unknown plan type"));
        } else {
            panic!("Expected ParseError");
        }
    }

    #[test]
    fn test_planner_with_tools() {
        let provider = crate::providers::create_mock_provider();
        let tools = vec![
            ToolInfo::new("read_file", "Read a file"),
            ToolInfo::new("write_file", "Write a file"),
        ];

        let planner = UnifiedPlanner::new(provider).with_tools(tools.clone());
        assert_eq!(planner.tools.len(), 2);
    }

    #[test]
    fn test_planner_set_tools() {
        let provider = crate::providers::create_mock_provider();
        let mut planner = UnifiedPlanner::new(provider);

        assert!(planner.tools.is_empty());

        planner.set_tools(vec![ToolInfo::new("test_tool", "A test tool")]);
        assert_eq!(planner.tools.len(), 1);
    }

    #[test]
    fn test_planner_with_config() {
        let provider = crate::providers::create_mock_provider();
        let config = PlannerConfig {
            timeout: Duration::from_secs(30),
            require_confirmation_for_destructive: false,
        };

        let planner = UnifiedPlanner::with_config(provider, config);
        assert_eq!(planner.config.timeout, Duration::from_secs(30));
        assert!(!planner.config.require_confirmation_for_destructive);
    }

    #[test]
    fn test_get_path_from_obj() {
        let mut obj = serde_json::Map::new();
        obj.insert("path".to_string(), serde_json::json!("/tmp/test.txt"));

        let result = get_path_from_obj(&obj, "path");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("/tmp/test.txt"));

        let missing = get_path_from_obj(&obj, "missing");
        assert!(missing.is_err());
    }
}
