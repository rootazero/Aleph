//! Bash execution tool - a convenience wrapper around CodeExecTool
//!
//! This tool provides a simplified interface for executing bash commands,
//! automatically routing to CodeExecTool with language=shell.
//!
//! This exists to maintain compatibility with AI prompts and skills that
//! reference "bash" as a tool name instead of "code_exec".
//!
//! Phase 3 Task 8: like `CodeExecTool`, this wrapper now carries the shared
//! `Arc<dyn Sandbox>` transitively — all subprocess execution routes through
//! `WorkspaceSandbox::execute`.

use crate::sync_primitives::Arc;
use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::code_exec::{CodeExecArgs, CodeExecTool, Language};
use crate::error::Result;
use crate::sandbox::Sandbox;
use crate::tools::AlephTool;

/// Arguments for bash execution tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BashExecArgs {
    /// The bash command to execute
    pub cmd: String,
    /// Working directory (optional, defaults to session workspace root)
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Timeout in seconds (optional, defaults to 60)
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Request elevated network access for this call (sandbox approval-gated).
    #[serde(default)]
    pub allow_network: bool,
    /// Request permission to fork subprocesses (sandbox approval-gated).
    #[serde(default)]
    pub allow_subprocess: bool,
    /// Extra writable paths beyond the session workspace (sandbox approval-gated).
    #[serde(default)]
    pub extra_writable_paths: Vec<PathBuf>,
}

/// Bash execution tool - wraps CodeExecTool for bash/shell commands
#[derive(Clone)]
pub struct BashExecTool {
    inner: CodeExecTool,
}

impl BashExecTool {
    /// Create a new bash execution tool without a sandbox wired in yet.
    /// Boot wiring attaches the shared `Arc<dyn Sandbox>` via
    /// [`BashExecTool::with_sandbox`]; unconfigured instances refuse execution
    /// with a structured error (delegated to `CodeExecTool`).
    pub fn new() -> Self {
        Self {
            inner: CodeExecTool::new(),
        }
    }

    /// Attach a shared `Arc<dyn Sandbox>`. Delegates to the inner
    /// `CodeExecTool` — both wrappers share the same sandbox instance.
    pub fn with_sandbox(mut self, sandbox: Arc<dyn Sandbox>) -> Self {
        self.inner = self.inner.with_sandbox(sandbox);
        self
    }
}

impl Default for BashExecTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of AlephTool trait for BashExecTool
#[async_trait]
impl AlephTool for BashExecTool {
    const NAME: &'static str = "bash";
    const DESCRIPTION: &'static str = r#"Execute bash/shell commands in a per-session sandboxed workspace.

Multi-line scripts are first-class — newlines, heredocs (`cat <<'EOF' ... EOF`),
loops, and pipelines all work. If you need 10 commands in a row, write them as
ONE multi-line `cmd` instead of 10 calls; that runs in a single bash process
with shared variables and exit-on-error semantics. Scripts larger than 32 KB
are automatically piped via `bash -s` + stdin so you never hit ARG_MAX.

Sessions are stateless: each call spawns a fresh bash process. `cd`, exported
variables, `set -e`, `source`, etc. do NOT carry over to the next call. If you
need cross-call state, write the state into a file under `working_dir` and
read it back in the next script, or just put everything into one cmd.

`working_dir` (optional) is resolved inside the session workspace; paths
outside the workspace are denied by the sandbox. If omitted the call lands at
the workspace root.

`timeout` defaults to 60s and is capped by the tool budget (180s ceiling).
On timeout we kill the process, drain stdout/stderr for up to 2s, and return
`exit_code = 124` (POSIX `timeout(1)` convention) with whatever the script
printed before the kill preserved in `stdout` and `stderr` — so even a
runaway script tells you what it accomplished.

Capability escalations (`allow_network`, `allow_subprocess`, `extra_writable_paths`)
trigger an approval prompt the first time per session; subsequent same-or-
narrower requests reuse the grant.

Examples:
- One-liner: {"cmd": "ls -la /tmp"}
- Multi-line with set -e: {"cmd": "set -e\ncd src\ncargo check\necho ok"}
- Heredoc: {"cmd": "cat <<'EOF' > /tmp/note\nhello world\nEOF\nwc -l /tmp/note"}
- Large script: {"cmd": "<paste a 50 KB build script — auto-piped via stdin>"}
- Custom timeout: {"cmd": "find . -name '*.rs' | wc -l", "timeout": 30}"#;

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
            allow_network: args.allow_network,
            allow_subprocess: args.allow_subprocess,
            extra_writable_paths: args.extra_writable_paths,
        };

        // Delegate to CodeExecTool
        self.inner.call(code_exec_args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R9 (Intelligence Lives in the Prompt): the bash description is
    /// how the model learns the surface area. Lock the load-bearing
    /// teaching points so future edits can't accidentally drop them.
    #[test]
    fn description_teaches_stateless_sessions_and_partial_output() {
        let d = <BashExecTool as AlephTool>::DESCRIPTION;
        // Stateless reality
        assert!(d.contains("stateless"), "should warn about stateless sessions");
        assert!(d.contains("do NOT carry over"), "should call out lost state");
        // Multi-line + heredoc encouragement
        assert!(d.contains("Multi-line scripts"), "should bless multi-line");
        assert!(d.contains("heredoc"), "should mention heredoc pattern");
        // 32KB stdin auto-pipe
        assert!(
            d.contains("32 KB") || d.contains("32KB"),
            "should mention the stdin-pipe threshold"
        );
        assert!(d.contains("ARG_MAX"), "should explain why the pipe exists");
        // Timeout + POSIX exit-code-124 contract
        assert!(d.contains("60s") || d.contains("60 seconds"), "default timeout");
        assert!(d.contains("180s"), "ceiling");
        assert!(d.contains("124"), "POSIX timeout exit code");
        // Partial-output guarantee
        assert!(
            d.contains("preserved"),
            "should promise partial output on kill"
        );
    }
}
