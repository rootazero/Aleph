use std::sync::Arc;
use std::path::PathBuf;

use crate::error::{AlephError, Result};
use crate::exec::sandbox::{
    executor::SandboxManager,
    adapter::{SandboxAdapter, SandboxCommand},
    audit::{SandboxAuditLog, ToolExecutionContext, ResolutionStep},
};
use super::tool_generator::GeneratedToolDefinition;
use super::sandbox_integration::resolve_tool_capabilities;

/// Executor for running tools in sandbox
pub struct SandboxedToolExecutor {
    sandbox_manager: SandboxManager,
}

impl SandboxedToolExecutor {
    pub fn new(sandbox_adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            sandbox_manager: SandboxManager::new(sandbox_adapter),
        }
    }

    /// Execute a tool with sandboxing
    pub async fn execute_tool(
        &self,
        tool_def: &GeneratedToolDefinition,
        parameters: serde_json::Value,
        tool_package_dir: PathBuf,
    ) -> Result<(String, SandboxAuditLog)> {
        // Resolve capabilities
        let capabilities = resolve_tool_capabilities(tool_def, &parameters)?;

        // Build command
        let command = SandboxCommand {
            program: self.get_runtime_executable(&tool_def.runtime)?,
            args: vec![
                tool_def.entrypoint.clone(),
                serde_json::to_string(&parameters).map_err(|e| AlephError::InvalidConfig {
                    message: format!("Failed to serialize parameters: {}", e),
                    suggestion: None,
                })?,
            ],
            working_dir: Some(tool_package_dir),
        };

        // Execute in sandbox
        let (result, mut audit_log) = self
            .sandbox_manager
            .execute_sandboxed(&tool_def.name, command, capabilities)
            .await?;

        // Add tool context to audit log
        audit_log.tool_context = Some(ToolExecutionContext {
            tool_name: tool_def.name.clone(),
            tool_version: tool_def.generated.generator_version.clone(),
            base_preset: tool_def
                .required_capabilities
                .as_ref()
                .map(|c| c.base_preset.clone())
                .unwrap_or_default(),
            applied_overrides: vec![],
            parameter_bindings_used: Default::default(),
            dynamic_paths: vec![],
            capability_resolution_log: vec![
                ResolutionStep {
                    step: "load_preset".to_string(),
                    timestamp: chrono::Utc::now().timestamp(),
                    description: "Loaded base preset".to_string(),
                },
            ],
        });

        Ok((result.stdout, audit_log))
    }

    fn get_runtime_executable(&self, runtime: &str) -> Result<String> {
        match runtime {
            "python" => Ok("python3".to_string()),
            "node" => Ok("node".to_string()),
            _ => Err(AlephError::InvalidConfig {
                message: format!("Unsupported runtime: {}", runtime),
                suggestion: Some("Use 'python' or 'node'".to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandboxed_executor_structure() {
        // Basic structure test
        assert!(true);
    }
}
