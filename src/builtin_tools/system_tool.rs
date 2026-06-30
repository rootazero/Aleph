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
    /// Action to perform: "`launch_app`", "`quit_app`", "`open_path`",
    /// "`list_running_apps`", "`send_notification`", "`clipboard_read`",
    /// "`clipboard_write`", "`system_info`"
    pub action: String,
    /// Application name or bundle ID (required for `launch_app`, `quit_app`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// File path or URL to open with the default app (required for `open_path`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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
- open_path: Open a file or URL with the OS default app (like double-clicking it). Use this to show the user a file you created — e.g. an .html report opens in their browser. Required: path (absolute file path or URL)
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
{"action":"open_path","path":"/Users/me/output/report.html"}
{"action":"open_path","path":"https://example.com"}
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
            // Opening a file/URL launches its default app — gate it like launch_app
            // so it can't bypass the DesktopLaunchApp approval. "open" is accepted
            // as an alias because models reach for it by analogy to `open`/`xdg-open`.
            "open_path" | "open" => Some((
                ActionType::DesktopLaunchApp,
                args.path.clone().unwrap_or_default(),
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

            "open_path" | "open" => {
                let target = match args.path {
                    Some(ref p) if !p.trim().is_empty() => p.as_str(),
                    _ => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "open_path requires a non-empty 'path' parameter \
                                 (an absolute file path or a URL)."
                                    .to_string(),
                            ),
                        });
                    }
                };
                match sys.open_path(target).await {
                    Ok(()) => Ok(SystemOutput {
                        success: true,
                        data: None,
                        message: Some(format!(
                            "Opened {target} with the default application."
                        )),
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
                     restart_app, open_path, list_running_apps, send_notification, \
                     clipboard_read, clipboard_write, system_info, user_idle_time"
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
    use aleph_desktop::traits::SystemCapability;
    use aleph_desktop::{DesktopError, DesktopPlatform};
    use aleph_desktop::Result as DesktopResult;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Records `open_path` targets so we can assert the tool forwards them.
    struct RecordingSystem {
        opened: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl SystemCapability for RecordingSystem {
        async fn launch_app(&self, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn quit_app(&self, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn list_running_apps(&self) -> DesktopResult<Vec<AppInfo>> {
            unimplemented!()
        }
        async fn send_notification(&self, _: &str, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn clipboard_read(&self) -> DesktopResult<ClipboardContent> {
            unimplemented!()
        }
        async fn clipboard_write(&self, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn system_info(&self) -> DesktopResult<SystemInfo> {
            unimplemented!()
        }
        // Override the trait default so the test never spawns a real handler.
        async fn open_path(&self, target: &str) -> DesktopResult<()> {
            if target.trim().is_empty() {
                return Err(DesktopError::InputFailed("empty".into()));
            }
            self.opened.lock().unwrap().push(target.to_string());
            Ok(())
        }
    }

    struct RecordingPlatform {
        sys: RecordingSystem,
    }

    impl DesktopPlatform for RecordingPlatform {
        fn platform_name(&self) -> &str {
            "TestOS"
        }
        fn screen(&self) -> Option<&dyn aleph_desktop::traits::ScreenCapability> {
            None
        }
        fn pim(&self) -> Option<&dyn aleph_desktop::traits::PimCapability> {
            None
        }
        fn system(&self) -> Option<&dyn SystemCapability> {
            Some(&self.sys)
        }
        fn automation(&self) -> Option<&dyn aleph_desktop::traits::AutomationCapability> {
            None
        }
        fn permission(&self) -> Option<&dyn aleph_desktop::traits::PermissionCapability> {
            None
        }
        fn media(&self) -> Option<&dyn aleph_desktop::traits::MediaCapability> {
            None
        }
    }

    fn tool_with_recorder() -> (SystemTool, Arc<Mutex<Vec<String>>>) {
        let opened = Arc::new(Mutex::new(Vec::new()));
        let platform = Arc::new(RecordingPlatform {
            sys: RecordingSystem {
                opened: Arc::clone(&opened),
            },
        });
        (SystemTool::new(platform), opened)
    }

    #[tokio::test]
    async fn open_path_forwards_target_to_capability() {
        let (tool, opened) = tool_with_recorder();
        let out = tool
            .call(SystemArgs {
                action: "open_path".into(),
                app_name: None,
                path: Some("/Users/me/output/report.html".into()),
                title: None,
                body: None,
            })
            .await
            .unwrap();

        assert!(out.success, "expected success, got: {:?}", out.message);
        assert_eq!(
            opened.lock().unwrap().as_slice(),
            &["/Users/me/output/report.html".to_string()]
        );
    }

    #[tokio::test]
    async fn open_alias_also_routes_to_open_path() {
        let (tool, opened) = tool_with_recorder();
        let out = tool
            .call(SystemArgs {
                action: "open".into(),
                app_name: None,
                path: Some("https://example.com".into()),
                title: None,
                body: None,
            })
            .await
            .unwrap();

        assert!(out.success);
        assert_eq!(opened.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn open_path_without_path_returns_friendly_error() {
        let (tool, opened) = tool_with_recorder();
        let out = tool
            .call(SystemArgs {
                action: "open_path".into(),
                app_name: None,
                path: None,
                title: None,
                body: None,
            })
            .await
            .unwrap();

        assert!(!out.success);
        assert!(out.message.unwrap().contains("requires"));
        assert!(opened.lock().unwrap().is_empty());
    }
}
