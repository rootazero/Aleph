//! System tool — app management, clipboard, notifications, system info.
//!
//! Delegates to `DesktopPlatform::system()` (`SystemCapability`).
//! When the capability is absent, all operations return a friendly message.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::error::Result;
use crate::tools::AlephTool;

/// System tool — gives the AI agent access to system-level operations.
#[derive(Clone)]
pub struct SystemTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl SystemTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self {
            platform,
            approval_policy: None,
        }
    }

    /// Attach an approval policy to gate state-changing actions.
    ///
    /// `launch_app` / `quit_app` / `restart_app` (`DesktopLaunchApp`) and
    /// `clipboard_write` (`DesktopType`) are checked before execution — the same
    /// OS operations `DesktopTool` already gates, so an agent cannot bypass the
    /// `desktop` tool's gate by routing through `system`. Read-only actions and
    /// notifications are always allowed.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Check the approval policy for a gated action.
    ///
    /// Returns `None` if allowed (or no policy configured), or
    /// `Some(SystemOutput)` describing the denial / confirmation prompt.
    /// Mirrors `DesktopTool::check_approval` (parameterized `ActionType`).
    async fn check_approval(
        &self,
        action_type: ActionType,
        action: &str,
        target: String,
    ) -> Option<SystemOutput> {
        let policy = self.approval_policy.as_ref()?;
        let (agent_id, context) = crate::approval::audit_identity("system", action, &target);
        let request = ActionRequest {
            action_type,
            target,
            agent_id,
            context,
            timestamp: chrono::Utc::now(),
        };
        let decision = policy.check(&request).await;
        match decision {
            ApprovalDecision::Allow => {
                policy.record(&request, &decision).await;
                None
            }
            ApprovalDecision::Deny { ref reason } => {
                policy.record(&request, &decision).await;
                Some(SystemOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Action denied by approval policy: {reason}")),
                })
            }
            ApprovalDecision::Ask { ref prompt } => Some(SystemOutput {
                success: false,
                data: Some(serde_json::json!({
                    "approval_required": true,
                    "prompt": prompt,
                })),
                message: Some(format!("Approval required: {prompt}")),
            }),
        }
    }
}

/// Arguments for the system tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SystemArgs {
    /// Action to perform: "`launch_app`", "`quit_app`", "`list_running_apps`",
    /// "`send_notification`", "`clipboard_read`", "`clipboard_write`", "`system_info`"
    pub action: String,
    /// Application name or bundle ID (required for `launch_app`, `quit_app`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// Notification title (required for `send_notification`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Notification body (required for `send_notification`) or text for `clipboard_write`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Output from the system tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[async_trait]
impl AlephTool for SystemTool {
    const NAME: &'static str = "system";
    const DESCRIPTION: &'static str = r#"System-level operations: app management, clipboard, notifications, system info.

Actions:
- launch_app: Launch an application. Required: app_name
- quit_app: Quit a running application. Required: app_name
- restart_app: Quit then relaunch an application. Required: app_name
- list_running_apps: List currently running applications
- send_notification: Send a system notification. Required: title, body
- clipboard_read: Read current clipboard content
- clipboard_write: Write text to clipboard. Required: body
- system_info: Get system information (OS, hostname, architecture, username)
- user_idle_time: Seconds since last keyboard/mouse input

Examples:
{"action":"launch_app","app_name":"Safari"}
{"action":"quit_app","app_name":"Safari"}
{"action":"restart_app","app_name":"Safari"}
{"action":"list_running_apps"}
{"action":"send_notification","title":"Reminder","body":"Meeting in 5 minutes"}
{"action":"clipboard_read"}
{"action":"clipboard_write","body":"Hello, world!"}
{"action":"system_info"}
{"action":"user_idle_time"}"#;

    type Args = SystemArgs;
    type Output = SystemOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let sys = match self.platform.system() {
            Some(s) => s,
            None => {
                return Ok(SystemOutput {
                    success: false,
                    data: None,
                    message: Some(format!(
                        "System capability is not available on {}. \
                         This platform does not support system operations.",
                        self.platform.platform_name()
                    )),
                });
            }
        };

        // Gate state-changing actions behind the approval policy (the same OS
        // ops DesktopTool gates), so an agent cannot bypass that gate via the
        // `system` tool. Permissive default = Allow, so this is byte-identical
        // until a policy file tightens it.
        let gated = match args.action.as_str() {
            "launch_app" | "quit_app" | "restart_app" => Some((
                ActionType::DesktopLaunchApp,
                args.app_name.clone().unwrap_or_default(),
            )),
            "clipboard_write" => Some((
                ActionType::DesktopType,
                args.body.clone().unwrap_or_default(),
            )),
            _ => None,
        };
        if let Some((action_type, target)) = gated {
            if let Some(out) = self
                .check_approval(action_type, args.action.as_str(), target)
                .await
            {
                return Ok(out);
            }
        }

        match args.action.as_str() {
            "launch_app" => {
                let app_name = match args.app_name {
                    Some(ref name) => name.as_str(),
                    None => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some("launch_app requires 'app_name' parameter.".to_string()),
                        });
                    }
                };
                match sys.launch_app(app_name).await {
                    Ok(()) => Ok(SystemOutput {
                        success: true,
                        data: None,
                        message: None,
                    }),
                    Err(e) => Ok(SystemOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "quit_app" => {
                let app_name = match args.app_name {
                    Some(ref name) => name.as_str(),
                    None => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some("quit_app requires 'app_name' parameter.".to_string()),
                        });
                    }
                };
                match sys.quit_app(app_name).await {
                    Ok(()) => Ok(SystemOutput {
                        success: true,
                        data: None,
                        message: None,
                    }),
                    Err(e) => Ok(SystemOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "restart_app" => {
                let app_name = match args.app_name {
                    Some(ref name) => name.as_str(),
                    None => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some("restart_app requires 'app_name' parameter.".to_string()),
                        });
                    }
                };
                match sys.restart_app(app_name).await {
                    Ok(()) => Ok(SystemOutput {
                        success: true,
                        data: None,
                        message: None,
                    }),
                    Err(e) => Ok(SystemOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "list_running_apps" => match sys.list_running_apps().await {
                Ok(apps) => Ok(SystemOutput {
                    success: true,
                    data: Some(serde_json::to_value(apps).unwrap_or_default()),
                    message: None,
                }),
                Err(e) => Ok(SystemOutput {
                    success: false,
                    data: None,
                    message: Some(e.to_string()),
                }),
            },

            "send_notification" => {
                let title = match args.title {
                    Some(ref t) => t.as_str(),
                    None => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "send_notification requires 'title' parameter.".to_string(),
                            ),
                        });
                    }
                };
                let body = match args.body {
                    Some(ref b) => b.as_str(),
                    None => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "send_notification requires 'body' parameter.".to_string(),
                            ),
                        });
                    }
                };
                match sys.send_notification(title, body).await {
                    Ok(()) => Ok(SystemOutput {
                        success: true,
                        data: None,
                        message: None,
                    }),
                    Err(e) => Ok(SystemOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "clipboard_read" => match sys.clipboard_read().await {
                Ok(content) => Ok(SystemOutput {
                    success: true,
                    data: Some(serde_json::to_value(content).unwrap_or_default()),
                    message: None,
                }),
                Err(e) => Ok(SystemOutput {
                    success: false,
                    data: None,
                    message: Some(e.to_string()),
                }),
            },

            "clipboard_write" => {
                let text = match args.body {
                    Some(ref b) => b.as_str(),
                    None => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some("clipboard_write requires 'body' parameter.".to_string()),
                        });
                    }
                };
                match sys.clipboard_write(text).await {
                    Ok(()) => Ok(SystemOutput {
                        success: true,
                        data: None,
                        message: None,
                    }),
                    Err(e) => Ok(SystemOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "system_info" => match sys.system_info().await {
                Ok(info) => Ok(SystemOutput {
                    success: true,
                    data: Some(serde_json::to_value(info).unwrap_or_default()),
                    message: None,
                }),
                Err(e) => Ok(SystemOutput {
                    success: false,
                    data: None,
                    message: Some(e.to_string()),
                }),
            },

            "user_idle_time" => match sys.user_idle_seconds().await {
                Ok(seconds) => Ok(SystemOutput {
                    success: true,
                    data: Some(serde_json::json!({ "idle_seconds": seconds })),
                    message: None,
                }),
                Err(e) => Ok(SystemOutput {
                    success: false,
                    data: None,
                    message: Some(e.to_string()),
                }),
            },

            unknown => Ok(SystemOutput {
                success: false,
                data: None,
                message: Some(format!(
                    "Unknown action: '{unknown}'. Valid actions: launch_app, quit_app, \
                     restart_app, list_running_apps, send_notification, clipboard_read, \
                     clipboard_write, system_info, user_idle_time"
                )),
            }),
        }
    }
}
