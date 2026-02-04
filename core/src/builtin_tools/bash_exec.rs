//! Bash execution tool - a convenience wrapper around CodeExecTool
//!
//! This tool provides a simplified interface for executing bash commands,
//! automatically routing to CodeExecTool with language=shell.
//!
//! This exists to maintain compatibility with AI prompts and skills that
//! reference "bash" as a tool name instead of "code_exec".

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::code_exec::{CodeExecArgs, CodeExecTool, Language};
use crate::error::Result;
use crate::tools::AetherTool;

/// Arguments for bash execution tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BashExecArgs {
    /// The bash command to execute
    pub cmd: String,
    /// Working directory (optional, defaults to temp directory)
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Timeout in seconds (optional, defaults to 60)
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Bash execution tool - wraps CodeExecTool for bash/shell commands
#[derive(Clone)]
pub struct BashExecTool {
    inner: CodeExecTool,
}

impl BashExecTool {
    /// Create a new bash execution tool
    pub fn new() -> Self {
        Self {
            inner: CodeExecTool::new(),
        }
    }
}

impl Default for BashExecTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of AetherTool trait for BashExecTool
#[async_trait]
impl AetherTool for BashExecTool {
    const NAME: &'static str = "bash";
    const DESCRIPTION: &'static str = r#"Execute bash/shell commands.

This is a convenience wrapper that automatically routes to the code_exec tool with language=shell.

Safety: Dangerous commands (sudo, rm -rf /, etc.) are blocked.
Timeout: Default 60 seconds, configurable.

Example:
{"cmd": "echo 'Hello, World!' && ls -la"}"#;

    type Args = BashExecArgs;
    type Output = super::code_exec::CodeExecOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "bash(cmd='ls -la /tmp')".to_string(),
            "bash(cmd='echo \"Hello World\" > /tmp/test.txt')".to_string(),
            "bash(cmd='pwd && ls -l', working_dir='/home/user')".to_string(),
            "bash(cmd='find . -name \"*.rs\" | wc -l', timeout=30)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Convert BashExecArgs to CodeExecArgs
        let code_exec_args = CodeExecArgs {
            language: Language::Shell,
            code: args.cmd,
            working_dir: args.working_dir,
            timeout: args.timeout,
        };

        // Delegate to CodeExecTool
        self.inner.call(code_exec_args).await
    }
}
