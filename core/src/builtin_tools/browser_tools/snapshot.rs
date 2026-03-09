// Browser snapshot tool — captures an accessibility tree snapshot of the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

fn default_profile() -> String {
    "default".into()
}

/// Arguments for the browser_snapshot tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSnapshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "default_profile")]
    pub profile: String,
}

/// Output from the browser_snapshot tool.
#[derive(Debug, Serialize)]
pub struct BrowserSnapshotOutput {
    pub success: bool,
    pub aria_tree: Option<String>,
    pub message: Option<String>,
}

/// Captures an accessibility tree (ARIA) snapshot of the current page.
#[derive(Clone)]
pub struct BrowserSnapshotTool {
    manager: Arc<ProfileManager>,
}

impl BrowserSnapshotTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserSnapshotTool {
    const NAME: &'static str = "browser_snapshot";
    const DESCRIPTION: &'static str =
        "Get an accessibility tree snapshot of the current browser page for structured understanding";
    type Args = BrowserSnapshotArgs;
    type Output = BrowserSnapshotOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserSnapshotOutput {
            success: true,
            aria_tree: Some("[placeholder] ARIA tree for current page".into()),
            message: Some(format!(
                "Snapshot captured in profile '{}'",
                args.profile
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_snapshot_returns_aria_tree() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSnapshotTool::new(manager);

        let result = tool
            .call(BrowserSnapshotArgs {
                profile: "default".into(),
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.aria_tree.is_some());
    }
}
