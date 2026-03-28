//! System tool — app management, clipboard, notifications, system info.
//!
//! Delegates to `DesktopPlatform::system()` (SystemCapability).
//! When the capability is absent, all operations return a friendly message.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::tools::AlephTool;

/// System tool — gives the AI agent access to system-level operations.
#[derive(Clone)]
pub struct SystemTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
}

impl SystemTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self { platform }
    }
}

/// Arguments for the system tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SystemArgs {
    /// Action to perform: "launch_app", "quit_app", "list_running_apps",
    /// "send_notification", "clipboard_read", "clipboard_write", "system_info"
    pub action: String,
    /// Application name or bundle ID (required for launch_app, quit_app).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// Notification title (required for send_notification).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Notification body (required for send_notification) or text for clipboard_write.
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
- list_running_apps: List currently running applications
- send_notification: Send a system notification. Required: title, body
- clipboard_read: Read current clipboard content
- clipboard_write: Write text to clipboard. Required: body
- system_info: Get system information (OS, hostname, architecture, username)
- user_idle_time: Seconds since last keyboard/mouse input

Examples:
{"action":"launch_app","app_name":"Safari"}
{"action":"quit_app","app_name":"Safari"}
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

        match args.action.as_str() {
            "launch_app" => {
                let app_name = match args.app_name {
                    Some(ref name) => name.as_str(),
                    None => {
                        return Ok(SystemOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "launch_app requires 'app_name' parameter.".to_string(),
                            ),
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
                            message: Some(
                                "quit_app requires 'app_name' parameter.".to_string(),
                            ),
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
                            message: Some(
                                "clipboard_write requires 'body' parameter.".to_string(),
                            ),
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
                     list_running_apps, send_notification, clipboard_read, clipboard_write, \
                     system_info, user_idle_time"
                )),
            }),
        }
    }
}
