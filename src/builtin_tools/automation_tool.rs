//! Automation tool — scripting (`AppleScript`, JXA, Shell) and Shortcuts.
//!
//! Delegates to `DesktopPlatform::automation()` (`AutomationCapability`).
//! When the capability is absent, all operations return a friendly message.

use crate::sync_primitives::Arc;

use aleph_desktop::automation_types::ScriptLanguage;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::error::Result;
use crate::tools::AlephTool;

/// Automation tool — gives the AI agent access to scripting and Shortcuts.
#[derive(Clone)]
pub struct AutomationTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl AutomationTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self {
            platform,
            approval_policy: None,
        }
    }

    /// Attach an approval policy to gate code-executing actions.
    ///
    /// `run_script` (AppleScript/JXA/shell/PowerShell) and `run_shortcut` are
    /// arbitrary code execution on the host and are checked against the
    /// `DesktopAutomation` action type before execution. Read-only
    /// `list_shortcuts` is always allowed.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Check the approval policy for a code-executing action.
    ///
    /// Returns `None` if allowed (or no policy configured), or
    /// `Some(AutomationOutput)` describing the denial / confirmation prompt.
    async fn check_approval(&self, action: &str, target: String) -> Option<AutomationOutput> {
        let policy = self.approval_policy.as_ref()?;
        let (agent_id, context) = crate::approval::audit_identity("automation", action, &target);
        let request = ActionRequest {
            action_type: ActionType::DesktopAutomation,
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
                Some(AutomationOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Action denied by approval policy: {reason}")),
                })
            }
            ApprovalDecision::Ask { ref prompt } => Some(AutomationOutput {
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

/// Arguments for the automation tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AutomationArgs {
    /// Action to perform: "`run_script`", "`list_shortcuts`", "`run_shortcut`"
    pub action: String,
    /// Script source code (required for `run_script`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Script language: "applescript", "javascript"/"jxa", "shell"/"bash" (for `run_script`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Shortcut name (required for `run_shortcut`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional input text for `run_shortcut`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Log file path for `run_background` (captures stdout+stderr). Optional —
    /// defaults to a temp file whose path is returned in the result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

/// Default log path for a `run_background` process when the caller does not
/// supply one. Uses a per-process counter so concurrent servers don't clobber
/// each other's logs within a single daemon lifetime.
fn default_background_log_path() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join(format!("aleph-background-{n}.log"))
        .to_string_lossy()
        .into_owned()
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

/// Parse a language string into `ScriptLanguage`.
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
- run_script: Execute a script that TERMINATES and return its output. Required: script, language. Languages: "applescript", "javascript"/"jxa", "shell"/"bash", "powershell". Bounded by a 120s timeout — do NOT use it for servers or watchers (they never exit and will be killed); use run_background for those.
- run_background: Launch a LONG-RUNNING process (dev server, file watcher, daemon) detached and return immediately with its PID. Required: script, language. Optional: log (path to capture stdout+stderr; defaults to a temp file, returned in the result). Use this to start a local server, then open the URL.
- list_shortcuts: List available Shortcuts / automation workflows
- run_shortcut: Run a Shortcut by name. Required: name. Optional: input

Examples:
{"action":"run_script","language":"applescript","script":"tell application \"Finder\" to get name of every window"}
{"action":"run_script","language":"shell","script":"ls -la ~/Desktop"}
{"action":"run_background","language":"shell","script":"cd /path/to/app && npm run dev","log":"/tmp/devserver.log"}
{"action":"run_script","language":"shell","script":"open http://localhost:5173"}
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
                            message: Some("run_script requires 'script' parameter.".to_string()),
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
                if let Some(out) = self
                    .check_approval("run_script", format!("{lang_str} script"))
                    .await
                {
                    return Ok(out);
                }
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
                            message: Some("run_shortcut requires 'name' parameter.".to_string()),
                        });
                    }
                };
                let input = args.input.as_deref();
                if let Some(out) = self
                    .check_approval("run_shortcut", format!("shortcut: {name}"))
                    .await
                {
                    return Ok(out);
                }
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

            "run_background" => {
                let script = match args.script {
                    Some(ref s) => s.as_str(),
                    None => {
                        return Ok(AutomationOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "run_background requires 'script' parameter.".to_string(),
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
                                "run_background requires 'language' parameter. \
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
                if let Some(out) = self
                    .check_approval("run_background", format!("{lang_str} background process"))
                    .await
                {
                    return Ok(out);
                }
                let log_path = args
                    .log
                    .clone()
                    .unwrap_or_else(default_background_log_path);
                match auto.run_background(language, script, &log_path).await {
                    Ok(pid) => Ok(AutomationOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "pid": pid,
                            "log_path": log_path,
                            "note": "Process started in the background. Read the log_path \
                                     to check its output/readiness; stop it with \
                                     run_script `kill <pid>`.",
                        })),
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
                     run_background, list_shortcuts, run_shortcut"
                )),
            }),
        }
    }
}
