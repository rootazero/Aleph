//! Code execution executor
//!
//! Implements the TaskExecutor trait for code/script execution.
//! Supports Shell, Python, and Node.js runtimes with sandboxing.

mod safety;
mod types;

#[cfg(test)]
mod tests;

pub use safety::{CommandChecker, SandboxConfig};
pub use types::{CodeExecError, CodeExecResult, RuntimeInfo};

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, info};

use super::{ExecutionContext, PathPermissionChecker, TaskExecutor};
use crate::dispatcher::agent_types::{CodeExec, Language, Task, TaskResult, TaskType};
use crate::dispatcher::{MAX_STDERR_SIZE, MAX_STDOUT_SIZE};
use crate::error::{AlephError, Result};

/// Code execution executor
pub struct CodeExecutor {
    /// Whether code execution is enabled
    enabled: bool,

    /// Default runtime (reserved for future use when auto-selecting runtime)
    _default_runtime: String,

    /// Execution timeout in seconds
    timeout_seconds: u64,

    /// Sandbox configuration (reserved for sandbox-exec integration)
    pub(super) _sandbox_config: SandboxConfig,

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

    /// Aleph-prioritized PATH (Aleph runtimes + system PATH)
    /// If None, uses system PATH only
    aleph_path: Option<String>,

    /// Runtime info cache to avoid repeated detection
    runtime_cache:
        std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, RuntimeInfo>>>,
}

impl CodeExecutor {
    /// Create a new code executor
    #[allow(clippy::too_many_arguments)]
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
        aleph_path: Option<String>,
    ) -> Self {
        let sandbox_config = SandboxConfig {
            enabled: sandbox_enabled,
            allow_network,
            ..Default::default()
        };

        Self {
            enabled,
            _default_runtime: default_runtime,
            timeout_seconds,
            _sandbox_config: sandbox_config,
            allowed_runtimes,
            command_checker: CommandChecker::new(blocked_commands),
            permission_checker,
            working_directory,
            pass_env,
            aleph_path,
            runtime_cache: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
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
    pub(crate) fn is_runtime_allowed(&self, runtime: &str) -> bool {
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
            return Err(AlephError::other(
                CodeExecError::Blocked { reason }.to_string(),
            ));
        }

        // Check runtime availability (using cache)
        let runtime_info = self.get_runtime_info(cmd).await;
        if !runtime_info.available {
            return Err(AlephError::other(
                CodeExecError::RuntimeNotFound(cmd.to_string()).to_string(),
            ));
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
            return Err(AlephError::other(
                CodeExecError::RuntimeNotAllowed(runtime.to_string()).to_string(),
            ));
        }

        // Check for blocked commands in the script
        if let Some(reason) = self.command_checker.is_blocked(code) {
            return Err(AlephError::other(
                CodeExecError::Blocked { reason }.to_string(),
            ));
        }

        // Check runtime availability (using cache)
        let runtime_info = self.get_runtime_info(runtime).await;
        if !runtime_info.available {
            return Err(AlephError::other(
                CodeExecError::RuntimeNotFound(runtime.to_string()).to_string(),
            ));
        }

        // Execute based on language
        let args = match language {
            Language::Python => vec!["-c".to_string(), code.to_string()],
            Language::JavaScript => vec!["-e".to_string(), code.to_string()],
            Language::Shell => vec!["-c".to_string(), code.to_string()],
            Language::Ruby => vec!["-e".to_string(), code.to_string()],
            Language::Rust => {
                // Rust needs compilation, not supported for inline execution
                return Err(AlephError::other(
                    "Inline Rust execution not supported. Use a script file instead.".to_string(),
                ));
            }
        };

        self.run_process(runtime, &args, None, ctx).await
    }

    /// Execute a script file
    async fn execute_file(&self, path: &Path, ctx: &ExecutionContext) -> Result<CodeExecResult> {
        // Check file path permission
        let canonical_path = self.permission_checker.check_path(path).map_err(|_e| {
            AlephError::other(CodeExecError::PathNotAllowed(path.to_path_buf()).to_string())
        })?;

        // Detect language from extension
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let (runtime, args) = match extension {
            "py" => (
                "python3",
                vec![canonical_path.to_string_lossy().to_string()],
            ),
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
            return Err(AlephError::other(
                CodeExecError::RuntimeNotAllowed(runtime.to_string()).to_string(),
            ));
        }

        // Check runtime availability (using cache)
        let runtime_info = self.get_runtime_info(runtime).await;
        if !runtime_info.available {
            return Err(AlephError::other(
                CodeExecError::RuntimeNotFound(runtime.to_string()).to_string(),
            ));
        }

        self.run_process(runtime, &args, Some(&canonical_path), ctx)
            .await
    }

    /// Run a process with timeout and output capture
    async fn run_process(
        &self,
        runtime: &str,
        args: &[String],
        _script_path: Option<&Path>,
        ctx: &ExecutionContext,
    ) -> Result<CodeExecResult> {
        let start = Instant::now();

        // Build command
        let mut cmd = Command::new(runtime);
        cmd.args(args);

        // Set working directory
        if let Some(ref working_dir) = self.working_directory {
            cmd.current_dir(working_dir);
        } else if let Some(ref ctx_working_dir) = ctx.working_directory {
            cmd.current_dir(ctx_working_dir);
        }

        // Set environment variables
        cmd.env_clear();

        // Special handling for PATH: use Aleph PATH if available
        if let Some(ref aleph_path) = self.aleph_path {
            cmd.env("PATH", aleph_path);
        } else if let Ok(system_path) = std::env::var("PATH") {
            cmd.env("PATH", system_path);
        }

        // Pass other environment variables (excluding PATH since we handled it above)
        for var in &self.pass_env {
            if var != "PATH" {
                if let Ok(value) = std::env::var(var) {
                    cmd.env(var, value);
                }
            }
        }

        // Setup pipes
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        // Spawn process
        let mut child = cmd.spawn().map_err(|e| {
            AlephError::other(
                CodeExecError::IoError(format!("Failed to spawn process: {}", e)).to_string(),
            )
        })?;

        // Capture output with timeout
        let timeout = Duration::from_secs(self.timeout_seconds);
        let result = tokio::time::timeout(timeout, async {
            // Read stdout and stderr concurrently to prevent pipe buffer deadlock.
            // Sequential reading can deadlock when a process fills one pipe's buffer
            // (typically 64KB) while we're blocked reading the other pipe.
            let mut stdout_handle = child.stdout.take();
            let mut stderr_handle = child.stderr.take();

            let stdout_fut = async {
                let mut buf_out = Vec::new();
                let mut trunc = false;
                if let Some(ref mut stdout) = stdout_handle {
                    let mut buf = vec![0u8; 8192];
                    loop {
                        match stdout.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                if buf_out.len() + n > MAX_STDOUT_SIZE {
                                    let remaining = MAX_STDOUT_SIZE - buf_out.len();
                                    buf_out.extend_from_slice(&buf[..remaining]);
                                    trunc = true;
                                    break;
                                }
                                buf_out.extend_from_slice(&buf[..n]);
                            }
                            Err(_) => break,
                        }
                    }
                }
                (buf_out, trunc)
            };

            let stderr_fut = async {
                let mut buf_err = Vec::new();
                let mut trunc = false;
                if let Some(ref mut stderr) = stderr_handle {
                    let mut buf = vec![0u8; 8192];
                    loop {
                        match stderr.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                if buf_err.len() + n > MAX_STDERR_SIZE {
                                    let remaining = MAX_STDERR_SIZE - buf_err.len();
                                    buf_err.extend_from_slice(&buf[..remaining]);
                                    trunc = true;
                                    break;
                                }
                                buf_err.extend_from_slice(&buf[..n]);
                            }
                            Err(_) => break,
                        }
                    }
                }
                (buf_err, trunc)
            };

            let ((stdout_buf, stdout_truncated), (stderr_buf, stderr_truncated)) =
                tokio::join!(stdout_fut, stderr_fut);

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
            Ok(Err(e)) => Err(AlephError::other(
                CodeExecError::IoError(e.to_string()).to_string(),
            )),
            Err(_) => {
                // Timeout - kill process
                let _ = child.kill().await;
                Err(AlephError::other(
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
            return Err(AlephError::other(CodeExecError::Disabled.to_string()));
        }

        let code_exec = match &task.task_type {
            TaskType::CodeExecution(ce) => ce,
            _ => {
                return Err(AlephError::other(
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
