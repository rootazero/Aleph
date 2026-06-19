//! Permission tool — TCC permission detection and request.
//!
//! Delegates to `DesktopPlatform::permission()` (`PermissionCapability`).
//! When the capability is absent, all operations return a friendly message.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::tools::AlephTool;

/// Permission tool — gives the AI agent access to macOS TCC permission management.
#[derive(Clone)]
pub struct PermissionTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
}

impl PermissionTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self { platform }
    }
}

/// Arguments for the permission tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionArgs {
    /// Action to perform: "check", "`check_all`", "request", "guide", "`open_settings`"
    pub action: String,
    /// Permission to check, request, guide, or open settings for.
    ///
    /// TCC-managed (support check / request / guide / `open_settings`):
    /// "`screen_recording`", "camera", "microphone", "`speech_recognition`",
    /// "accessibility", "notifications".
    ///
    /// Manual-grant kinds (support guide / `open_settings`; check reports
    /// `Unknown`): "`input_monitoring`", "`full_disk`", "automation", "contacts",
    /// "calendars", "reminders", "photos", "location".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
}

/// Output from the permission tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Parse a permission string into a `PermissionKind` enum value.
fn parse_permission(s: &str) -> Option<aleph_desktop::permission_types::PermissionKind> {
    // serde rename_all = "snake_case", so we can deserialize from the snake_case string
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

#[async_trait]
impl AlephTool for PermissionTool {
    const NAME: &'static str = "permission";
    const DESCRIPTION: &'static str = r#"Check or request macOS system permissions (TCC).

Actions:
- check: Check status of a specific permission. Required: permission
- check_all: Check status of all managed permissions
- request: Request a permission (may show system dialog). Required: permission
- guide: Get a step-by-step guide (deep link + steps + rationale) for granting a permission the user must enable manually. Required: permission
- open_settings: Open System Settings to the pane for a permission. Required: permission

Permission values:
- check / request capable (TCC-managed): screen_recording, camera, microphone, speech_recognition, accessibility, notifications
- guide / open_settings only (manually granted in System Settings): input_monitoring, full_disk, automation, contacts, calendars, reminders, photos, location

Examples:
{"action":"check","permission":"screen_recording"}
{"action":"check_all"}
{"action":"request","permission":"microphone"}
{"action":"guide","permission":"accessibility"}
{"action":"open_settings","permission":"screen_recording"}"#;

    type Args = PermissionArgs;
    type Output = PermissionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let perm_cap = match self.platform.permission() {
            Some(p) => p,
            None => {
                return Ok(PermissionOutput {
                    success: false,
                    data: None,
                    message: Some(format!(
                        "Permission capability is not available on {}. \
                         This platform does not support TCC permission management.",
                        self.platform.platform_name()
                    )),
                });
            }
        };

        match args.action.as_str() {
            "check" => {
                let perm = match args.permission.as_deref().and_then(parse_permission) {
                    Some(p) => p,
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "check requires a valid 'permission' parameter. \
                                 Values: screen_recording, camera, microphone, \
                                 speech_recognition, accessibility, notifications"
                                    .to_string(),
                            ),
                        });
                    }
                };
                match perm_cap.check(perm).await {
                    Ok(info) => Ok(PermissionOutput {
                        success: true,
                        data: Some(serde_json::to_value(info).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(PermissionOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "check_all" => match perm_cap.check_all().await {
                Ok(infos) => Ok(PermissionOutput {
                    success: true,
                    data: Some(serde_json::to_value(infos).unwrap_or_default()),
                    message: None,
                }),
                Err(e) => Ok(PermissionOutput {
                    success: false,
                    data: None,
                    message: Some(e.to_string()),
                }),
            },

            "request" => {
                let perm = match args.permission.as_deref().and_then(parse_permission) {
                    Some(p) => p,
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "request requires a valid 'permission' parameter. \
                                 Values: screen_recording, camera, microphone, \
                                 speech_recognition, accessibility, notifications"
                                    .to_string(),
                            ),
                        });
                    }
                };
                match perm_cap.request(perm).await {
                    Ok(info) => Ok(PermissionOutput {
                        success: true,
                        data: Some(serde_json::to_value(info).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(PermissionOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "guide" => {
                let perm = match args.permission.as_deref().and_then(parse_permission) {
                    Some(p) => p,
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "guide requires a valid 'permission' parameter. \
                                 Values: screen_recording, camera, microphone, \
                                 speech_recognition, accessibility, notifications, \
                                 input_monitoring, full_disk, automation, contacts, \
                                 calendars, reminders, photos, location"
                                    .to_string(),
                            ),
                        });
                    }
                };
                match perm_cap.guide_permission(perm).await {
                    Ok(guide) => Ok(PermissionOutput {
                        success: true,
                        data: Some(serde_json::to_value(guide).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(PermissionOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "open_settings" => {
                let perm = match args.permission.as_deref().and_then(parse_permission) {
                    Some(p) => p,
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "open_settings requires a valid 'permission' parameter. \
                                 Values: screen_recording, camera, microphone, \
                                 speech_recognition, accessibility, notifications, \
                                 input_monitoring, full_disk, automation, contacts, \
                                 calendars, reminders, photos, location"
                                    .to_string(),
                            ),
                        });
                    }
                };
                match perm_cap.open_settings(perm).await {
                    Ok(opened) => Ok(PermissionOutput {
                        success: opened,
                        data: Some(serde_json::json!({ "opened": opened })),
                        message: if opened {
                            None
                        } else {
                            Some("System Settings could not be opened for this permission.".into())
                        },
                    }),
                    Err(e) => Ok(PermissionOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            unknown => Ok(PermissionOutput {
                success: false,
                data: None,
                message: Some(format!(
                    "Unknown action: '{unknown}'. Valid actions: check, check_all, \
                     request, guide, open_settings"
                )),
            }),
        }
    }
}
