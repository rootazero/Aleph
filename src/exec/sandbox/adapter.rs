use crate::error::Result;
use crate::exec::sandbox::capabilities::Capabilities;
use async_trait::async_trait;
use std::path::PathBuf;

/// Command to execute in sandbox
#[derive(Debug, Clone)]
pub struct SandboxCommand {
    /// Program to execute
    pub program: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Working directory
    pub working_dir: Option<PathBuf>,
}

/// Sandbox configuration profile
#[derive(Debug, Clone)]
pub struct SandboxProfile {
    /// Path to sandbox configuration file (e.g., .sb file)
    pub path: PathBuf,
    /// Capabilities this profile enforces
    pub capabilities: Capabilities,
    /// Platform identifier
    pub platform: String,
    /// Temporary workspace directory (if TempWorkspace capability used)
    pub temp_workspace: Option<PathBuf>,
}

/// Result of sandboxed execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Exit code
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Whether execution was sandboxed
    pub sandboxed: bool,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Platform-specific sandbox adapter
#[async_trait]
pub trait SandboxAdapter: Send + Sync {
    /// Check if sandbox is supported on current platform
    fn is_supported(&self) -> bool;

    /// Get platform identifier
    fn platform_name(&self) -> &str;

    /// Generate sandbox configuration profile
    fn generate_profile(&self, caps: &Capabilities) -> Result<SandboxProfile>;

    /// Execute command in sandbox
    async fn execute_sandboxed(
        &self,
        command: &SandboxCommand,
        profile: &SandboxProfile,
    ) -> Result<ExecutionResult>;

    /// Cleanup temporary configuration files and workspaces
    fn cleanup(&self, profile: &SandboxProfile) -> Result<()>;
}
