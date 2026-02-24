//! Security context for BDD tests (VirtualFs sandbox testing)

use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

use alephcore::tools::markdown_skill::MarkdownCliTool;
use alephcore::tools::AlephToolServer;

/// Context for security/sandbox tests
#[derive(Default)]
pub struct SecurityContext {
    /// Temporary directory for skill files
    pub temp_dir: Option<TempDir>,
    /// Loaded markdown skill tools
    pub loaded_tools: Vec<MarkdownCliTool>,
    /// Tool server for integration tests
    pub tool_server: Option<Arc<AlephToolServer>>,
    /// Last execution result
    pub execution_result: Option<Result<SkillExecutionResult, String>>,
    /// Tool server call result
    pub tool_server_result: Option<serde_json::Value>,
    /// Skill directory path
    pub skill_dir: Option<PathBuf>,
    /// Sandbox directory count before execution
    pub sandbox_count_before: usize,
    /// Sandbox directory count after execution
    pub sandbox_count_after: usize,
}

/// Simplified execution result for testing
#[derive(Debug, Clone)]
pub struct SkillExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl std::fmt::Debug for SecurityContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityContext")
            .field("temp_dir", &self.temp_dir.as_ref().map(|_| "TempDir"))
            .field("loaded_tools", &self.loaded_tools.len())
            .field("tool_server", &self.tool_server.as_ref().map(|_| "AlephToolServer"))
            .field("execution_result", &self.execution_result)
            .field("tool_server_result", &self.tool_server_result.as_ref().map(|_| "Value"))
            .field("skill_dir", &self.skill_dir)
            .field("sandbox_count_before", &self.sandbox_count_before)
            .field("sandbox_count_after", &self.sandbox_count_after)
            .finish()
    }
}
