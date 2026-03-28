//! ACP delegate and switch tools
//!
//! Provides builtin tools that delegate tasks to external CLI agents
//! (Claude Code, Codex, Gemini) via the ACP harness system.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{notify_tool_result, notify_tool_start};
use crate::acp::harness::HarnessMode;
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
    /// Execution mode override: "oneshot" or "native_acp". If not specified, uses the harness default.
    pub mode: Option<String>,
    /// Whether to reuse an existing session for multi-step continuity (native_acp mode only). Defaults to true.
    pub reuse_session: Option<bool>,
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

fn parse_mode(s: &str) -> Result<HarnessMode> {
    match s {
        "oneshot" => Ok(HarnessMode::Oneshot),
        "native_acp" => Ok(HarnessMode::NativeAcp),
        _ => Err(AlephError::tool(format!(
            "Invalid mode '{}'. Use 'oneshot' or 'native_acp'.",
            s
        ))),
    }
}

fn resolve_cwd(cwd: Option<&str>) -> String {
    cwd.map(|s| s.to_string()).unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string())
    })
}

// =============================================================================
// ACP delegate tool macro — eliminates duplication across harnesses
// =============================================================================

/// Generate an ACP delegate tool struct that forwards prompts to a specific harness.
///
/// Each invocation creates a `$struct_name` with `new()`, `Clone`, and `AlephTool` impl.
macro_rules! acp_delegate_tool {
    (
        struct $struct_name:ident;
        tool_name = $tool_name:literal;
        harness_id = $harness_id:literal;
        display_label = $display_label:literal;
        description = $description:literal;
    ) => {
        #[derive(Clone)]
        pub struct $struct_name {
            manager: Arc<AcpHarnessManager>,
        }

        impl $struct_name {
            pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
                Self { manager }
            }
        }

        #[async_trait]
        impl AlephTool for $struct_name {
            const NAME: &'static str = $tool_name;
            const DESCRIPTION: &'static str = $description;

            type Args = AcpDelegateArgs;
            type Output = AcpDelegateOutput;

            async fn call(&self, args: Self::Args) -> Result<Self::Output> {
                let args_summary = format!("{}: {}", $display_label, truncate(&args.prompt, 80));
                notify_tool_start(Self::NAME, &args_summary);

                let cwd = resolve_cwd(args.cwd.as_deref());
                let mode = args.mode.as_deref().map(parse_mode).transpose()?;
                let reuse = args.reuse_session.unwrap_or(true);
                let result = self.manager.prompt($harness_id, &args.prompt, &cwd, mode, reuse, None).await;

                match result {
                    Ok(text) => {
                        notify_tool_result(Self::NAME, "completed", true);
                        Ok(AcpDelegateOutput {
                            harness: $harness_id.to_string(),
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
    };
}

acp_delegate_tool! {
    struct ClaudeCodeTool;
    tool_name = "claude_code";
    harness_id = "claude-code";
    display_label = "Claude Code";
    description = "Delegate a coding task to Claude Code CLI. Supports two modes: \
        'oneshot' (fresh process per prompt, default) and 'native_acp' \
        (persistent session with context continuity). Set reuse_session \
        to maintain context across multi-step workflows.";
}

acp_delegate_tool! {
    struct CodexTool;
    tool_name = "codex";
    harness_id = "codex";
    display_label = "Codex";
    description = "Delegate a coding task to OpenAI Codex CLI. Supports 'oneshot' (default) \
        and 'native_acp' modes. Set reuse_session for multi-step continuity.";
}

acp_delegate_tool! {
    struct GeminiCliTool;
    tool_name = "gemini_cli";
    harness_id = "gemini";
    display_label = "Gemini";
    description = "Delegate a task to Google Gemini CLI. Supports 'native_acp' (default, persistent session) \
        and 'oneshot' modes. Set reuse_session for multi-step continuity.";
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
///
/// TODO: This tool currently validates the target and pre-spawns NativeAcp sessions,
/// but does NOT actually change any active-agent state in the execution engine.
/// The returned `target` must be consumed by the execution engine to route
/// subsequent messages to the selected harness. Until that integration is done,
/// this tool is effectively a no-op beyond session warm-up.
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
        if !self.manager.has_harness(&args.target).await {
            let err_msg = format!("Unknown agent: '{}'. Valid targets: claude-code, codex, gemini, aleph", &args.target);
            notify_tool_result(Self::NAME, &err_msg, false);
            return Err(AlephError::tool(err_msg));
        }

        // Pre-spawn session for NativeAcp harnesses so the switch is immediate
        if self.manager.harness_mode(&args.target).await == Some(crate::acp::harness::HarnessMode::NativeAcp) {
            let cwd = resolve_cwd(None);
            self.manager.ensure_session(&args.target, &cwd).await?;
        }

        let display_name = self
            .manager
            .display_name(&args.target)
            .await
            .unwrap_or_else(|| args.target.clone());
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
    // Use char_indices for UTF-8 safety (count chars, not bytes)
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
        assert_eq!(args.mode, None);
        assert_eq!(args.reuse_session, None);
    }

    #[test]
    fn test_delegate_args_no_cwd() {
        let json = r#"{"prompt": "Fix the bug"}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.prompt, "Fix the bug");
        assert_eq!(args.cwd, None);
        assert_eq!(args.mode, None);
        assert_eq!(args.reuse_session, None);
    }

    #[test]
    fn test_delegate_args_with_mode_and_reuse() {
        let json = r#"{"prompt": "task", "mode": "native_acp", "reuse_session": false}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.mode, Some("native_acp".to_string()));
        assert_eq!(args.reuse_session, Some(false));
    }

    #[test]
    fn test_parse_mode_valid() {
        assert!(matches!(parse_mode("oneshot"), Ok(HarnessMode::Oneshot)));
        assert!(matches!(parse_mode("native_acp"), Ok(HarnessMode::NativeAcp)));
    }

    #[test]
    fn test_parse_mode_invalid() {
        assert!(parse_mode("unknown").is_err());
        assert!(parse_mode("").is_err());
    }

    #[test]
    fn test_switch_args_deserialize() {
        let json = r#"{"target": "claude-code"}"#;
        let args: AcpSwitchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.target, "claude-code");
    }
}
