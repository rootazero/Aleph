//! LLM-based task planner implementation

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::prompt::{build_user_prompt, PLANNING_SYSTEM_PROMPT};
use super::TaskPlanner;
use crate::cowork::types::{
    AiTask, AppAuto, CodeExec, DocGen, FileOp, Language, Task, TaskDependency, TaskGraph, TaskType,
};
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;

/// LLM-based task planner
pub struct LlmTaskPlanner {
    provider: Arc<dyn AiProvider>,
}

impl LlmTaskPlanner {
    /// Create a new LLM task planner
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Parse the LLM response into a TaskGraph
    fn parse_response(&self, response: &str, original_request: &str) -> Result<TaskGraph> {
        // Extract JSON from response (may be wrapped in markdown code blocks)
        let json_str = extract_json(response)?;

        // Parse the JSON
        let plan: PlanResponse = serde_json::from_str(&json_str).map_err(|e| {
            error!("Failed to parse LLM response as JSON: {}", e);
            AetherError::Other {
                message: format!("Invalid task plan JSON: {}", e),
                suggestion: Some(
                    "The AI response was not valid JSON. Try rephrasing your request.".to_string(),
                ),
            }
        })?;

        // Convert to TaskGraph
        self.build_task_graph(plan, original_request)
    }

    /// Build a TaskGraph from the parsed plan
    fn build_task_graph(&self, plan: PlanResponse, original_request: &str) -> Result<TaskGraph> {
        let graph_id = Uuid::new_v4().to_string();

        let mut graph = TaskGraph::new(&graph_id, &plan.title);
        graph.metadata.original_request = Some(original_request.to_string());

        // Convert tasks
        for task_def in plan.tasks {
            let task_type = self.parse_task_type(&task_def.task_type)?;

            let task = Task::new(&task_def.id, &task_def.name, task_type)
                .with_parameters(serde_json::to_value(&task_def.task_type).unwrap_or_default());

            if let Some(desc) = task_def.description {
                graph.add_task(task.with_description(desc));
            } else {
                graph.add_task(task);
            }

            // Add dependencies
            if let Some(deps) = task_def.depends_on {
                for dep in deps {
                    graph.edges.push(TaskDependency::new(&dep, &task_def.id));
                }
            }
        }

        // Validate the graph
        graph.validate().map_err(|e| {
            error!("Generated task graph is invalid: {}", e);
            AetherError::Other {
                message: format!("Invalid task graph: {}", e),
                suggestion: Some(
                    "The generated task graph has issues. Try rephrasing your request.".to_string(),
                ),
            }
        })?;

        info!(
            "Created task graph '{}' with {} tasks",
            graph.id,
            graph.tasks.len()
        );

        Ok(graph)
    }

    /// Parse a task type from the JSON definition
    fn parse_task_type(&self, type_def: &TaskTypeDef) -> Result<TaskType> {
        match type_def.type_name.as_str() {
            "file_operation" => self.parse_file_op(type_def),
            "code_execution" => self.parse_code_exec(type_def),
            "document_generation" => self.parse_doc_gen(type_def),
            "app_automation" => self.parse_app_auto(type_def),
            "ai_inference" => self.parse_ai_inference(type_def),
            other => {
                warn!("Unknown task type: {}", other);
                // Default to AI inference for unknown types
                Ok(TaskType::AiInference(AiTask {
                    prompt: format!("Execute task: {}", other),
                    requires_privacy: false,
                    has_images: false,
                    output_format: None,
                }))
            }
        }
    }

    fn parse_file_op(&self, def: &TaskTypeDef) -> Result<TaskType> {
        let op = def.op.as_deref().unwrap_or("list");
        let path = def.path.clone().unwrap_or_else(|| ".".to_string()).into();

        let file_op = match op {
            "read" => FileOp::Read { path },
            "write" => FileOp::Write { path },
            "move" => FileOp::Move {
                from: def.from.clone().unwrap_or_default().into(),
                to: def.to.clone().unwrap_or_default().into(),
            },
            "copy" => FileOp::Copy {
                from: def.from.clone().unwrap_or_default().into(),
                to: def.to.clone().unwrap_or_default().into(),
            },
            "delete" => FileOp::Delete { path },
            "search" => FileOp::Search {
                pattern: def.pattern.clone().unwrap_or_else(|| "*".to_string()),
                dir: path,
            },
            "list" => FileOp::List { path },
            "batch_move" => FileOp::BatchMove { operations: vec![] },
            _ => FileOp::List { path },
        };

        Ok(TaskType::FileOperation(file_op))
    }

    fn parse_code_exec(&self, def: &TaskTypeDef) -> Result<TaskType> {
        let exec = def.exec.as_deref().unwrap_or("command");

        let code_exec = match exec {
            "script" => CodeExec::Script {
                code: def.code.clone().unwrap_or_default(),
                language: parse_language(def.language.as_deref()),
            },
            "file" => CodeExec::File {
                path: def.path.clone().unwrap_or_default().into(),
            },
            "command" => CodeExec::Command {
                cmd: def.cmd.clone().unwrap_or_default(),
                args: def.args.clone().unwrap_or_default(),
            },
            _ => CodeExec::Command {
                cmd: "echo".to_string(),
                args: vec!["Unknown command".to_string()],
            },
        };

        Ok(TaskType::CodeExecution(code_exec))
    }

    fn parse_doc_gen(&self, def: &TaskTypeDef) -> Result<TaskType> {
        let format = def.format.as_deref().unwrap_or("markdown");
        let output = def
            .output
            .clone()
            .unwrap_or_else(|| "output.md".to_string())
            .into();
        let template = def.template.clone().map(|s| s.into());

        let doc_gen = match format {
            "excel" => DocGen::Excel { template, output },
            "power_point" | "powerpoint" | "pptx" => DocGen::PowerPoint { template, output },
            "pdf" => DocGen::Pdf {
                style: def.style.clone(),
                output,
            },
            "markdown" | "md" => DocGen::Markdown { output },
            _ => DocGen::Markdown { output },
        };

        Ok(TaskType::DocumentGeneration(doc_gen))
    }

    fn parse_app_auto(&self, def: &TaskTypeDef) -> Result<TaskType> {
        let action = def.action.as_deref().unwrap_or("launch");

        let app_auto = match action {
            "launch" => AppAuto::Launch {
                bundle_id: def.bundle_id.clone().unwrap_or_default(),
            },
            "apple_script" | "applescript" => AppAuto::AppleScript {
                script: def.script.clone().unwrap_or_default(),
            },
            "ui_action" => AppAuto::UiAction {
                ui_action: "click".to_string(),
                target: def.target.clone().unwrap_or_default(),
            },
            _ => AppAuto::Launch {
                bundle_id: "com.apple.finder".to_string(),
            },
        };

        Ok(TaskType::AppAutomation(app_auto))
    }

    fn parse_ai_inference(&self, def: &TaskTypeDef) -> Result<TaskType> {
        Ok(TaskType::AiInference(AiTask {
            prompt: def.prompt.clone().unwrap_or_else(|| "Analyze".to_string()),
            requires_privacy: def.requires_privacy.unwrap_or(false),
            has_images: def.has_images.unwrap_or(false),
            output_format: def.output_format.clone(),
        }))
    }
}

#[async_trait]
impl TaskPlanner for LlmTaskPlanner {
    async fn plan(&self, request: &str) -> Result<TaskGraph> {
        info!("Planning task from request: {}", request);

        // Build the prompt
        let user_prompt = build_user_prompt(request);

        debug!("Sending planning request to LLM");

        // Call the LLM
        let response = self
            .provider
            .process(&user_prompt, Some(PLANNING_SYSTEM_PROMPT))
            .await
            .map_err(|e| {
                error!("LLM planning request failed: {}", e);
                AetherError::provider(format!("Planning failed: {}", e))
            })?;

        debug!("Received LLM response, parsing...");

        // Parse the response
        self.parse_response(&response, request)
    }

    fn name(&self) -> &str {
        "LlmTaskPlanner"
    }
}

/// Extract JSON from a response that may be wrapped in markdown code blocks
fn extract_json(response: &str) -> Result<String> {
    let trimmed = response.trim();

    // Try to find JSON in code block
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7;
        if let Some(end) = trimmed[json_start..].find("```") {
            return Ok(trimmed[json_start..json_start + end].trim().to_string());
        }
    }

    // Try to find JSON in generic code block
    if let Some(start) = trimmed.find("```") {
        let json_start = trimmed[start + 3..].find('\n').map(|n| start + 4 + n);
        if let Some(json_start) = json_start {
            if let Some(end) = trimmed[json_start..].find("```") {
                return Ok(trimmed[json_start..json_start + end].trim().to_string());
            }
        }
    }

    // Try direct JSON parse
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }

    // Find first { and last }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end > start {
            return Ok(trimmed[start..=end].to_string());
        }
    }

    Err(AetherError::Other {
        message: "Could not find JSON in response".to_string(),
        suggestion: Some(
            "The AI did not return a valid task plan. Try rephrasing your request.".to_string(),
        ),
    })
}

/// Parse language string to Language enum
fn parse_language(lang: Option<&str>) -> Language {
    match lang {
        Some("python") | Some("py") => Language::Python,
        Some("javascript") | Some("js") => Language::JavaScript,
        Some("shell") | Some("bash") | Some("sh") => Language::Shell,
        Some("ruby") | Some("rb") => Language::Ruby,
        Some("rust") | Some("rs") => Language::Rust,
        _ => Language::Shell,
    }
}

// JSON parsing structures

#[derive(Debug, Deserialize)]
struct PlanResponse {
    title: String,
    tasks: Vec<TaskDef>,
}

#[derive(Debug, Deserialize)]
struct TaskDef {
    id: String,
    name: String,
    description: Option<String>,
    #[serde(rename = "type")]
    task_type: TaskTypeDef,
    depends_on: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskTypeDef {
    #[serde(rename = "type")]
    type_name: String,

    // File operation fields
    op: Option<String>,
    path: Option<String>,
    from: Option<String>,
    to: Option<String>,
    pattern: Option<String>,

    // Code execution fields
    exec: Option<String>,
    code: Option<String>,
    language: Option<String>,
    cmd: Option<String>,
    args: Option<Vec<String>>,

    // Document generation fields
    format: Option<String>,
    output: Option<String>,
    template: Option<String>,
    style: Option<String>,

    // App automation fields
    action: Option<String>,
    bundle_id: Option<String>,
    script: Option<String>,
    target: Option<String>,

    // AI inference fields
    prompt: Option<String>,
    requires_privacy: Option<bool>,
    has_images: Option<bool>,
    output_format: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let response = r#"{"title": "Test", "tasks": []}"#;
        let json = extract_json(response).unwrap();
        assert!(json.contains("title"));
    }

    #[test]
    fn test_extract_json_code_block() {
        let response = r#"Here's the plan:

```json
{"title": "Test", "tasks": []}
```

Let me know if you need changes."#;

        let json = extract_json(response).unwrap();
        assert!(json.contains("title"));
    }

    #[test]
    fn test_extract_json_embedded() {
        let response = r#"I'll create a plan: {"title": "Test", "tasks": []} Done!"#;
        let json = extract_json(response).unwrap();
        assert!(json.contains("title"));
    }

    #[test]
    fn test_parse_language() {
        assert_eq!(parse_language(Some("python")), Language::Python);
        assert_eq!(parse_language(Some("js")), Language::JavaScript);
        assert_eq!(parse_language(Some("bash")), Language::Shell);
        assert_eq!(parse_language(None), Language::Shell);
    }
}
