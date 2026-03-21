//! Automation tool — scripting (AppleScript, JXA, Shell) and Shortcuts.
//!
//! Delegates to `DesktopPlatform::automation()` (AutomationCapability).
//! When the capability is absent, all operations return a friendly message.

use crate::sync_primitives::Arc;

use aleph_desktop::automation_types::ScriptLanguage;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::tools::AlephTool;

/// Automation tool — gives the AI agent access to scripting and Shortcuts.
#[derive(Clone)]
pub struct AutomationTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
}

impl AutomationTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self { platform }
    }
}

/// Arguments for the automation tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AutomationArgs {
    /// Action to perform: "run_script", "list_shortcuts", "run_shortcut"
    pub action: String,
    /// Script source code (required for run_script).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Script language: "applescript", "javascript"/"jxa", "shell"/"bash" (for run_script).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Shortcut name (required for run_shortcut).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional input text for run_shortcut.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
}

/// Output from the automation tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Parse a language string into ScriptLanguage.
fn parse_script_language(lang: &str) -> Option<ScriptLanguage> {
    match lang.to_lowercase().as_str() {
        "applescript" => Some(ScriptLanguage::AppleScript),
        "javascript" | "jxa" => Some(ScriptLanguage::Jxa),
        "shell" | "bash" => Some(ScriptLanguage::Shell),
        "powershell" => Some(ScriptLanguage::PowerShell),
        _ => None,
    }
}

#[async_trait]
impl AlephTool for AutomationTool {
    const NAME: &'static str = "automation";
    const DESCRIPTION: &'static str = r#"Run automation scripts and Shortcuts on the desktop.

Actions:
- run_script: Execute a script. Required: script, language. Languages: "applescript", "javascript"/"jxa", "shell"/"bash", "powershell"
- list_shortcuts: List available Shortcuts / automation workflows
- run_shortcut: Run a Shortcut by name. Required: name. Optional: input

Examples:
{"action":"run_script","language":"applescript","script":"tell application \"Finder\" to get name of every window"}
{"action":"run_script","language":"shell","script":"ls -la ~/Desktop"}
{"action":"list_shortcuts"}
{"action":"run_shortcut","name":"My Workflow","input":"hello"}"#;

    type Args = AutomationArgs;
    type Output = AutomationOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let auto = match self.platform.automation() {
            Some(a) => a,
            None => {
                return Ok(AutomationOutput {
                    success: false,
                    data: None,
                    message: Some(format!(
                        "Automation capability is not available on {}. \
                         This platform does not support automation scripting.",
                        self.platform.platform_name()
                    )),
                });
            }
        };

        match args.action.as_str() {
            "run_script" => {
                let script = match args.script {
                    Some(ref s) => s.as_str(),
                    None => {
                        return Ok(AutomationOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "run_script requires 'script' parameter.".to_string(),
                            ),
                        });
                    }
                };
                let lang_str = match args.language {
                    Some(ref l) => l.as_str(),
                    None => {
                        return Ok(AutomationOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "run_script requires 'language' parameter. \
                                 Valid languages: applescript, javascript/jxa, shell/bash, powershell"
                                    .to_string(),
                            ),
                        });
                    }
                };
                let language = match parse_script_language(lang_str) {
                    Some(l) => l,
                    None => {
                        return Ok(AutomationOutput {
                            success: false,
                            data: None,
                            message: Some(format!(
                                "Unknown language: '{lang_str}'. \
                                 Valid languages: applescript, javascript/jxa, shell/bash, powershell"
                            )),
                        });
                    }
                };
                match auto.run_script(language, script).await {
                    Ok(output) => Ok(AutomationOutput {
                        success: true,
                        data: Some(serde_json::json!({ "output": output })),
                        message: None,
                    }),
                    Err(e) => Ok(AutomationOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "list_shortcuts" => match auto.list_shortcuts().await {
                Ok(shortcuts) => Ok(AutomationOutput {
                    success: true,
                    data: Some(serde_json::to_value(shortcuts).unwrap_or_default()),
                    message: None,
                }),
                Err(e) => Ok(AutomationOutput {
                    success: false,
                    data: None,
                    message: Some(e.to_string()),
                }),
            },

            "run_shortcut" => {
                let name = match args.name {
                    Some(ref n) => n.as_str(),
                    None => {
                        return Ok(AutomationOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "run_shortcut requires 'name' parameter.".to_string(),
                            ),
                        });
                    }
                };
                let input = args.input.as_deref();
                match auto.run_shortcut(name, input).await {
                    Ok(output) => Ok(AutomationOutput {
                        success: true,
                        data: Some(serde_json::json!({ "output": output })),
                        message: None,
                    }),
                    Err(e) => Ok(AutomationOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            unknown => Ok(AutomationOutput {
                success: false,
                data: None,
                message: Some(format!(
                    "Unknown action: '{unknown}'. Valid actions: run_script, \
                     list_shortcuts, run_shortcut"
                )),
            }),
        }
    }
}
