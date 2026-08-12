// Browser console tool — reads browser console messages.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserConsoleArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
}

#[derive(Debug, Serialize)]
pub struct BrowserConsoleOutput {
    pub success: bool,
    pub messages: String,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct BrowserConsoleTool {
    manager: Arc<ProfileManager>,
}

impl BrowserConsoleTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserConsoleTool {
    const NAME: &'static str = "browser_console";
    const DESCRIPTION: &'static str =
        "Read browser console messages (log, warn, error) for debugging";
    type Args = BrowserConsoleArgs;
    type Output = BrowserConsoleOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.console_messages(&tab_id).await {
                Ok(messages) => {
                    let line_count = messages.lines().count();
                    // Console output is page-script-controlled; treat as untrusted
                    // (redact credentials, then wrap). Append-ordered log:
                    // head+tail truncation keeps the newest messages.
                    let wrapped = super::redact_and_wrap_log(&self.manager, &messages);
                    Ok(BrowserConsoleOutput {
                        success: true,
                        messages: wrapped,
                        message: Some(format!("{line_count} console message(s)")),
                    })
                }
                Err(e) => Ok(BrowserConsoleOutput {
                    success: false,
                    messages: String::new(),
                    message: Some(format!(
                        "Console read failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserConsoleOutput {
                success: false,
                messages: String::new(),
                message: Some(super::backend_error_text(&self.manager, &e)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_console_read() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserConsoleTool::new(manager);
        let result = tool
            .call(BrowserConsoleArgs {
                profile: "default".into(),
            })
            .await
            .unwrap();
        assert!(!result.success); // No browser running
    }
}
