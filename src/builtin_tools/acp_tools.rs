//! ACP delegate and switch tools
//!
//! Provides a unified tool that delegates tasks to external CLI agents
//! (Claude Code, Codex, Gemini, or custom) via the ACP harness system.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{notify_tool_result, notify_tool_start, notify_tool_streaming_chunk};
use crate::acp::adapter::AdapterMode;
use crate::acp::manager::AcpAdapterManager;
use crate::acp::AcpChunkCallback;
use crate::config::types::acp::TrustLevel;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// AcpDelegateTool — unified delegation to any ACP harness
// =============================================================================

/// Arguments for the unified ACP delegate tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpDelegateArgs {
    /// Which harness to delegate to (e.g. "claude-code", "gemini", "codex", or any custom).
    pub harness: String,
    /// The prompt / task description to send to the external CLI agent.
    pub prompt: String,
    /// Working directory for the agent session. Defaults to home directory if not specified.
    pub cwd: Option<String>,
    /// Execution mode override: "oneshot" or "native_acp". If not specified, uses the harness default.
    pub mode: Option<String>,
    /// Whether to reuse an existing session for multi-step continuity (native_acp mode only). Defaults to true.
    pub reuse_session: Option<bool>,
}

/// Output from the unified ACP delegate tool.
#[derive(Debug, Clone, Serialize)]
pub struct AcpDelegateOutput {
    /// Which harness produced the result.
    pub harness: String,
    /// The text response from the external agent.
    pub result: String,
}

/// Unified ACP delegate tool — delegates tasks to any registered ACP harness.
#[derive(Clone)]
pub struct AcpDelegateTool {
    manager: Arc<AcpAdapterManager>,
}

impl AcpDelegateTool {
    pub fn new(manager: Arc<AcpAdapterManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for AcpDelegateTool {
    const NAME: &'static str = "acp_delegate";
    const DESCRIPTION: &'static str = "Delegate a task to an external CLI agent via ACP. \
        Use 'claude-code', 'codex', or 'gemini' as the harness parameter, \
        or any custom harness registered via acp.create.";

    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("{}: {}", &args.harness, truncate(&args.prompt, 80));
        notify_tool_start(Self::NAME, &args_summary);

        // Trust level check
        let config = self
            .manager
            .get_config(&args.harness)
            .await
            .ok_or_else(|| {
                AlephError::tool(format!(
                    "Unknown ACP harness: '{}'. Check available harnesses via acp.list.",
                    args.harness
                ))
            })?;

        match config.trust_level {
            TrustLevel::Disabled => {
                let msg = format!(
                    "ACP harness '{}' is disabled (trust_level=disabled)",
                    args.harness
                );
                notify_tool_result(Self::NAME, &msg, false);
                return Err(AlephError::tool(msg));
            }
            TrustLevel::Confirm => {
                // User confirmation not yet integrated with gateway approval mechanism.
                // Block rather than silently proceeding — set trust_level=full to allow.
                let msg = format!(
                    "ACP harness '{}' requires user confirmation (trust_level=confirm), \
                     but approval flow is not yet implemented. Set trust_level to 'full' \
                     via acp.update to allow delegation.",
                    args.harness
                );
                notify_tool_result(Self::NAME, &msg, false);
                return Err(AlephError::tool(msg));
            }
            TrustLevel::Full => {}
        }

        let cwd = resolve_cwd(args.cwd.as_deref());
        let mode = args.mode.as_deref().map(parse_mode).transpose()?;
        let reuse = args.reuse_session.unwrap_or(true);

        // Build streaming callback
        let on_chunk: AcpChunkCallback = Arc::new(move |chunk: &str| {
            notify_tool_streaming_chunk("acp_delegate", chunk);
        });

        let result = self
            .manager
            .prompt(
                &args.harness,
                &args.prompt,
                &cwd,
                mode,
                reuse,
                Some(on_chunk),
            )
            .await;

        match result {
            Ok(text) => {
                notify_tool_result(Self::NAME, "completed", true);
                Ok(AcpDelegateOutput {
                    harness: args.harness,
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
// AcpSwitchTool (preserved unchanged)
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
    manager: Arc<AcpAdapterManager>,
}

impl AcpSwitchTool {
    pub fn new(manager: Arc<AcpAdapterManager>) -> Self {
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

        if args.target == "aleph" {
            let msg = "Switched back to Aleph.".to_string();
            notify_tool_result(Self::NAME, &msg, true);
            return Ok(AcpSwitchOutput {
                target: "aleph".to_string(),
                message: msg,
            });
        }

        if !self.manager.has_harness(&args.target).await {
            let err_msg = format!(
                "Unknown agent: '{}'. Valid targets: claude-code, codex, gemini, aleph",
                &args.target
            );
            notify_tool_result(Self::NAME, &err_msg, false);
            return Err(AlephError::tool(err_msg));
        }

        if self.manager.harness_mode(&args.target).await
            == Some(crate::acp::adapter::AdapterMode::NativeAcp)
        {
            let cwd = resolve_cwd(None);
            self.manager.ensure_session(&args.target, &cwd).await?;
        }

        let display_name = self
            .manager
            .display_name(&args.target)
            .await
            .unwrap_or_else(|| args.target.clone());
        let msg = format!(
            "Switched to {}. Messages will be forwarded to this agent.",
            display_name
        );

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

fn parse_mode(s: &str) -> Result<AdapterMode> {
    match s {
        "oneshot" => Ok(AdapterMode::Oneshot),
        "native_acp" => Ok(AdapterMode::NativeAcp),
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

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
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
    }

    #[test]
    fn test_truncate_utf8() {
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
        let json =
            r#"{"harness": "claude-code", "prompt": "Fix the bug", "cwd": "/home/user/project"}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.harness, "claude-code");
        assert_eq!(args.prompt, "Fix the bug");
        assert_eq!(args.cwd, Some("/home/user/project".to_string()));
    }

    #[test]
    fn test_delegate_args_minimal() {
        let json = r#"{"harness": "gemini", "prompt": "test"}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.harness, "gemini");
        assert_eq!(args.mode, None);
        assert_eq!(args.reuse_session, None);
    }

    #[test]
    fn test_parse_mode_valid() {
        assert!(matches!(parse_mode("oneshot"), Ok(AdapterMode::Oneshot)));
        assert!(matches!(
            parse_mode("native_acp"),
            Ok(AdapterMode::NativeAcp)
        ));
    }

    #[test]
    fn test_parse_mode_invalid() {
        assert!(parse_mode("unknown").is_err());
    }

    #[test]
    fn test_switch_args_deserialize() {
        let json = r#"{"target": "claude-code"}"#;
        let args: AcpSwitchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.target, "claude-code");
    }
}
