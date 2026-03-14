//! ACP delegate and switch tools
//!
//! Provides builtin tools that delegate tasks to external CLI agents
//! (Claude Code, Codex, Gemini) via the ACP harness system.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{notify_tool_result, notify_tool_start};
use crate::acp::manager::AcpHarnessManager;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Shared Args / Output types for delegate tools
// =============================================================================

/// Arguments for ACP delegate tools (claude_code, codex, gemini_cli).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpDelegateArgs {
    /// The prompt / task description to send to the external CLI agent.
    pub prompt: String,
    /// Working directory for the agent session. Defaults to home directory if not specified.
    pub cwd: Option<String>,
}

/// Output from ACP delegate tools.
#[derive(Debug, Clone, Serialize)]
pub struct AcpDelegateOutput {
    /// Which harness produced the result.
    pub harness: String,
    /// The text response from the external agent.
    pub result: String,
}

// =============================================================================
// Helper: resolve cwd
// =============================================================================

fn resolve_cwd(cwd: Option<&str>) -> String {
    cwd.map(|s| s.to_string()).unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string())
    })
}

// =============================================================================
// ClaudeCodeTool
// =============================================================================

/// Delegate a coding task to Claude Code CLI.
#[derive(Clone)]
pub struct ClaudeCodeTool {
    manager: Arc<AcpHarnessManager>,
}

impl ClaudeCodeTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for ClaudeCodeTool {
    const NAME: &'static str = "claude_code";
    const DESCRIPTION: &'static str =
        "Delegate a coding task to Claude Code CLI. Use when you need Claude Code's specialized coding capabilities with direct file system access.";

    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("Claude Code: {}", truncate(&args.prompt, 80));
        notify_tool_start(Self::NAME, &args_summary);

        let cwd = resolve_cwd(args.cwd.as_deref());
        let result = self.manager.prompt("claude-code", &args.prompt, &cwd).await;

        match result {
            Ok(text) => {
                notify_tool_result(Self::NAME, "completed", true);
                Ok(AcpDelegateOutput {
                    harness: "claude-code".to_string(),
                    result: text,
                })
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
                Err(e)
            }
        }
    }
}

// =============================================================================
// CodexTool
// =============================================================================

/// Delegate a coding task to OpenAI Codex CLI.
#[derive(Clone)]
pub struct CodexTool {
    manager: Arc<AcpHarnessManager>,
}

impl CodexTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for CodexTool {
    const NAME: &'static str = "codex";
    const DESCRIPTION: &'static str =
        "Delegate a coding task to OpenAI Codex CLI. Use when you need Codex's code generation capabilities with direct file system access.";

    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("Codex: {}", truncate(&args.prompt, 80));
        notify_tool_start(Self::NAME, &args_summary);

        let cwd = resolve_cwd(args.cwd.as_deref());
        let result = self.manager.prompt("codex", &args.prompt, &cwd).await;

        match result {
            Ok(text) => {
                notify_tool_result(Self::NAME, "completed", true);
                Ok(AcpDelegateOutput {
                    harness: "codex".to_string(),
                    result: text,
                })
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
                Err(e)
            }
        }
    }
}

// =============================================================================
// GeminiCliTool
// =============================================================================

/// Delegate a task to Google Gemini CLI.
#[derive(Clone)]
pub struct GeminiCliTool {
    manager: Arc<AcpHarnessManager>,
}

impl GeminiCliTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for GeminiCliTool {
    const NAME: &'static str = "gemini_cli";
    const DESCRIPTION: &'static str =
        "Delegate a task to Google Gemini CLI. Use when you need Gemini's capabilities with direct file system access.";

    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("Gemini: {}", truncate(&args.prompt, 80));
        notify_tool_start(Self::NAME, &args_summary);

        let cwd = resolve_cwd(args.cwd.as_deref());
        let result = self.manager.prompt("gemini", &args.prompt, &cwd).await;

        match result {
            Ok(text) => {
                notify_tool_result(Self::NAME, "completed", true);
                Ok(AcpDelegateOutput {
                    harness: "gemini".to_string(),
                    result: text,
                })
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
                Err(e)
            }
        }
    }
}

// =============================================================================
// AcpSwitchTool
// =============================================================================

/// Arguments for switching the active CLI agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpSwitchArgs {
    /// Target agent to switch to: "claude-code", "codex", "gemini", or "aleph".
    pub target: String,
}

/// Output from the ACP switch tool.
#[derive(Debug, Clone, Serialize)]
pub struct AcpSwitchOutput {
    /// The target that was switched to.
    pub target: String,
    /// Human-readable status message.
    pub message: String,
}

/// Switch to direct conversation with an external CLI agent, or switch back to Aleph.
#[derive(Clone)]
pub struct AcpSwitchTool {
    manager: Arc<AcpHarnessManager>,
}

impl AcpSwitchTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for AcpSwitchTool {
    const NAME: &'static str = "acp_switch";
    const DESCRIPTION: &'static str =
        "Switch to direct conversation with an external CLI agent (Claude Code, Codex, or Gemini), or switch back to Aleph.";

    type Args = AcpSwitchArgs;
    type Output = AcpSwitchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("Switch to: {}", &args.target);
        notify_tool_start(Self::NAME, &args_summary);

        // Switching back to Aleph is always valid
        if args.target == "aleph" {
            let msg = "Switched back to Aleph.".to_string();
            notify_tool_result(Self::NAME, &msg, true);
            return Ok(AcpSwitchOutput {
                target: "aleph".to_string(),
                message: msg,
            });
        }

        // Validate harness exists
        if !self.manager.has_harness(&args.target) {
            let err_msg = format!("Unknown agent: '{}'. Valid targets: claude-code, codex, gemini, aleph", &args.target);
            notify_tool_result(Self::NAME, &err_msg, false);
            return Err(AlephError::tool(err_msg));
        }

        // Pre-spawn session for NativeAcp harnesses so the switch is immediate
        if self.manager.harness_mode(&args.target) == Some(crate::acp::harness::HarnessMode::NativeAcp) {
            let cwd = resolve_cwd(None);
            self.manager.ensure_session(&args.target, &cwd).await?;
        }

        let display_name = self
            .manager
            .display_name(&args.target)
            .unwrap_or(&args.target);
        let msg = format!("Switched to {}. Messages will be forwarded to this agent.", display_name);

        info!(target = %args.target, "ACP agent switch");
        notify_tool_result(Self::NAME, &msg, true);

        Ok(AcpSwitchOutput {
            target: args.target,
            message: msg,
        })
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // Use char_indices for UTF-8 safety
    match s.char_indices().nth(max_len) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let result = truncate("hello world this is a long string", 11);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 14); // 11 chars + "..."
    }

    #[test]
    fn test_truncate_utf8() {
        // Ensure no panic on multi-byte chars
        let result = truncate("你好世界这是一段中文", 4);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_resolve_cwd_some() {
        assert_eq!(resolve_cwd(Some("/tmp")), "/tmp");
    }

    #[test]
    fn test_resolve_cwd_none() {
        let cwd = resolve_cwd(None);
        assert!(!cwd.is_empty());
    }

    #[test]
    fn test_delegate_args_deserialize() {
        let json = r#"{"prompt": "Fix the bug", "cwd": "/home/user/project"}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.prompt, "Fix the bug");
        assert_eq!(args.cwd, Some("/home/user/project".to_string()));
    }

    #[test]
    fn test_delegate_args_no_cwd() {
        let json = r#"{"prompt": "Fix the bug"}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.prompt, "Fix the bug");
        assert_eq!(args.cwd, None);
    }

    #[test]
    fn test_switch_args_deserialize() {
        let json = r#"{"target": "claude-code"}"#;
        let args: AcpSwitchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.target, "claude-code");
    }
}
