// Browser network tool — reads the network request log of the current page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_network` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserNetworkArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
}

/// Output from the `browser_network` tool.
#[derive(Debug, Serialize)]
pub struct BrowserNetworkOutput {
    pub success: bool,
    pub requests: String,
    pub message: Option<String>,
}

/// Reads the network request log of the current browser page for debugging.
#[derive(Clone)]
pub struct BrowserNetworkTool {
    manager: Arc<ProfileManager>,
}

impl BrowserNetworkTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserNetworkTool {
    const NAME: &'static str = "browser_network";
    const DESCRIPTION: &'static str =
        "Read the network request log of the current browser page for debugging";
    type Args = BrowserNetworkArgs;
    type Output = BrowserNetworkOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.network_log(&tab_id).await {
                Ok(requests) => {
                    let line_count = requests.lines().count();
                    // Network log entries include URLs/headers from the page;
                    // untrusted (redact credentials, then wrap). Append-ordered
                    // log: head+tail truncation keeps the newest requests.
                    let wrapped = super::redact_and_wrap_log(&self.manager, &requests);
                    Ok(BrowserNetworkOutput {
                        success: true,
                        requests: wrapped,
                        message: Some(format!("{line_count} network log line(s)")),
                    })
                }
                Err(e) => Ok(BrowserNetworkOutput {
                    success: false,
                    requests: String::new(),
                    message: Some(format!(
                        "Network log read failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserNetworkOutput {
                success: false,
                requests: String::new(),
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
    async fn test_network_read_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserNetworkTool::new(manager);
        let result = tool
            .call(BrowserNetworkArgs {
                profile: "default".into(),
            })
            .await
            .unwrap();
        assert!(!result.success); // No browser running
    }
}
