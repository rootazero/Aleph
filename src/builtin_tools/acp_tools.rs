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
    /// Execution mode override: "oneshot" or "`native_acp`". If not specified, uses the harness default.
    pub mode: Option<String>,
    /// Whether to reuse an existing session for multi-step continuity (`native_acp` mode only). Defaults to true.
    pub reuse_session: Option<bool>,
    /// Optional session name. Lets you keep multiple parallel sessions in the
    /// same cwd (mirrors acpx's `-s backend` / `-s frontend`). Omit for the
    /// default unnamed session.
    pub session_name: Option<String>,
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
    pub const fn new(manager: Arc<AcpAdapterManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for AcpDelegateTool {
    const NAME: &'static str = "acp_delegate";
    const DESCRIPTION: &'static str = "Delegate a task to an external CLI agent via ACP. \
        Use 'claude-code', 'codex', or 'gemini' as the harness parameter, \
        or any custom harness the operator has registered.";

    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!(
            "{}: {}",
            &args.harness,
            crate::utils::text_format::truncate_text(&args.prompt, 80)
        );
        notify_tool_start(Self::NAME, &args_summary);

        // Trust level check
        let config = self
            .manager
            .get_config(&args.harness)
            .await
            .ok_or_else(|| {
                crate::acp::protocol::AcpOperationError::new(
                    crate::acp::protocol::AcpErrorCode::HarnessNotFound,
                    format!(
                        "Unknown ACP harness: '{}'. Check available harnesses via acp.list.",
                        args.harness
                    ),
                )
            })?;

        match config.trust_level {
            TrustLevel::Disabled => {
                let msg = format!(
                    "ACP harness '{}' is disabled (trust_level=disabled)",
                    args.harness
                );
                notify_tool_result(Self::NAME, &msg, false);
                return Err(crate::acp::protocol::AcpOperationError::new(
                    crate::acp::protocol::AcpErrorCode::HarnessDenied,
                    msg,
                )
                .into());
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
            .prompt_named(
                &args.harness,
                &args.prompt,
                &cwd,
                args.session_name.as_deref(),
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
    pub const fn new(manager: Arc<AcpAdapterManager>) -> Self {
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
        let msg = format!("Switched to {display_name}. Messages will be forwarded to this agent.");

        info!(target = %args.target, "ACP agent switch");
        notify_tool_result(Self::NAME, &msg, true);

        Ok(AcpSwitchOutput {
            target: args.target,
            message: msg,
        })
    }
}

// =============================================================================
// AcpSessionControlTool — set_mode / set_model / set_config_option /
// authenticate / cancel against an existing ACP session.
//
// R8 (Everything is a Tool): exposes every ACP runtime knob as a single tool
// the LLM can call. Mirrors acpx's session-control surface.
// =============================================================================

/// Discriminator for `acp_session_control`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpSessionAction {
    /// Switch the agent's interaction mode (adapter-specific IDs).
    SetMode,
    /// Switch the underlying model (adapter-specific IDs).
    SetModel,
    /// Set an adapter-specific runtime config option (e.g. `temperature`).
    SetConfigOption,
    /// Run the ACP `authenticate` handshake. Reads credential from env
    /// `ACP_AUTH_<METHOD_ID>` (uppercased, non-alphanumeric → `_`) when
    /// `credential` is omitted, mirroring acpx's auth-env policy.
    Authenticate,
    /// Send a cooperative `session/cancel` notification (does not block on
    /// an in-flight prompt; uses the manager's `CancelHandle`).
    Cancel,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpSessionControlArgs {
    /// Which harness to control. Must already have an active session.
    pub harness: String,
    /// Working directory of the session. Defaults to home if omitted.
    pub cwd: Option<String>,
    /// Optional session name (parallel-sessions-in-same-cwd, mirrors
    /// acpx `-s <name>`). Omit for the default unnamed session.
    pub session_name: Option<String>,
    /// What to do.
    pub action: AcpSessionAction,
    /// For `set_mode` / `set_model`: the target id. Ignored otherwise.
    pub id: Option<String>,
    /// For `set_config_option`: the option key (e.g. `temperature`).
    pub key: Option<String>,
    /// For `set_config_option`: the option value (any JSON).
    pub value: Option<serde_json::Value>,
    /// For `authenticate`: the auth `methodId` advertised by the adapter.
    pub method_id: Option<String>,
    /// For `authenticate`: opaque credential. If omitted, reads
    /// `ACP_AUTH_<methodId>` from process env.
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AcpSessionControlOutput {
    pub harness: String,
    pub action: AcpSessionAction,
    pub message: String,
}

#[derive(Clone)]
pub struct AcpSessionControlTool {
    manager: Arc<AcpAdapterManager>,
}

impl AcpSessionControlTool {
    pub const fn new(manager: Arc<AcpAdapterManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for AcpSessionControlTool {
    const NAME: &'static str = "acp_session_control";
    const DESCRIPTION: &'static str =
        "Control an existing ACP session: set_mode, set_model, set_config_option, \
         authenticate, or cancel. Requires that the session has already been \
         created (use acp_delegate first to create one).";

    type Args = AcpSessionControlArgs;
    type Output = AcpSessionControlOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &format!("{}: {:?}", args.harness, args.action));
        let cwd = resolve_cwd(args.cwd.as_deref());
        let session_name = args.session_name.as_deref();
        let message = match args.action {
            AcpSessionAction::SetMode => {
                let id = require_field(&args.id, "id", "set_mode")?;
                self.manager
                    .set_mode(&args.harness, &cwd, session_name, id)
                    .await?;
                format!("mode set to {id}")
            }
            AcpSessionAction::SetModel => {
                let id = require_field(&args.id, "id", "set_model")?;
                self.manager
                    .set_model(&args.harness, &cwd, session_name, id)
                    .await?;
                format!("model set to {id}")
            }
            AcpSessionAction::SetConfigOption => {
                let key = require_field(&args.key, "key", "set_config_option")?;
                let value = args.value.clone().unwrap_or(serde_json::Value::Null);
                self.manager
                    .set_config_option(&args.harness, &cwd, session_name, key, value)
                    .await?;
                format!("config option {key} updated")
            }
            AcpSessionAction::Authenticate => {
                let method_id = require_field(&args.method_id, "method_id", "authenticate")?;
                let credential = match args.credential.as_deref() {
                    Some(c) => c.to_string(),
                    None => read_auth_env(method_id).ok_or_else(|| {
                        crate::acp::protocol::AcpOperationError::new(
                            crate::acp::protocol::AcpErrorCode::AuthRequired,
                            format!(
                                "no credential provided and env ACP_AUTH_{} unset",
                                env_token(method_id)
                            ),
                        )
                    })?,
                };
                self.manager
                    .authenticate(&args.harness, &cwd, session_name, method_id, &credential)
                    .await?;
                format!("authenticated via method {method_id}")
            }
            AcpSessionAction::Cancel => {
                self.manager
                    .cancel_named(&args.harness, &cwd, session_name)
                    .await?;
                "cancel notification sent".to_string()
            }
        };
        notify_tool_result(Self::NAME, &message, true);
        Ok(AcpSessionControlOutput {
            harness: args.harness,
            action: args.action,
            message,
        })
    }
}

fn require_field<'a>(field: &'a Option<String>, name: &str, op: &str) -> Result<&'a str> {
    field.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| {
        crate::acp::protocol::AcpOperationError::new(
            crate::acp::protocol::AcpErrorCode::ProtocolError { code: -32602 },
            format!("acp_session_control: '{name}' required for action '{op}'"),
        )
        .into()
    })
}

/// Translate a `method_id` into the `ACP_AUTH`_* env variable name per
/// acpx's `auth-env.ts` policy: trim, non-alphanumeric → `_`, uppercase.
fn env_token(method_id: &str) -> String {
    method_id
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn read_auth_env(method_id: &str) -> Option<String> {
    let token = env_token(method_id);
    if token.is_empty() {
        return None;
    }
    let key = format!("ACP_AUTH_{token}");
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

// =============================================================================
// Helpers
// =============================================================================

fn parse_mode(s: &str) -> Result<AdapterMode> {
    match s {
        "oneshot" => Ok(AdapterMode::Oneshot),
        "native_acp" => Ok(AdapterMode::NativeAcp),
        _ => Err(crate::acp::protocol::AcpOperationError::new(
            crate::acp::protocol::AcpErrorCode::ModeUnsupported,
            format!("Invalid mode '{s}'. Use 'oneshot' or 'native_acp'."),
        )
        .into()),
    }
}

fn resolve_cwd(cwd: Option<&str>) -> String {
    cwd.map_or_else(
        || dirs::home_dir().map_or_else(|| ".".to_string(), |p| p.to_string_lossy().into_owned()),
        |s| s.to_string(),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
