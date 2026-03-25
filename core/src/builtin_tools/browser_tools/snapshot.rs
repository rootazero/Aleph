// Browser snapshot tool — captures an accessibility tree snapshot of the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::snapshot_format::{format_snapshot, SnapshotFormatOptions};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

fn default_true() -> bool { true }

/// Arguments for the browser_snapshot tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSnapshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Only include interactive elements (buttons, inputs, links).
    #[serde(default)]
    pub interactive_only: bool,
    /// Skip unnamed structural elements (default: true).
    #[serde(default = "default_true")]
    pub compact: bool,
    /// Maximum output characters (default: 30000). Set higher for complex pages.
    pub max_chars: Option<usize>,
}

/// Output from the browser_snapshot tool.
#[derive(Debug, Serialize)]
pub struct BrowserSnapshotOutput {
    pub success: bool,
    pub snapshot: Option<String>,
    pub truncated: bool,
    pub ref_count: usize,
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
        let opts = SnapshotFormatOptions {
            interactive_only: args.interactive_only,
            compact: args.compact,
            max_chars: args.max_chars.or(Some(30_000)),
            max_depth: None,
        };

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.snapshot(&tab_id).await {
                Ok(snap) => {
                    let formatted = format_snapshot(&snap, &opts);
                    Ok(BrowserSnapshotOutput {
                        success: true,
                        snapshot: Some(formatted.text),
                        truncated: formatted.truncated,
                        ref_count: formatted.ref_count,
                        message: Some(format!("Snapshot captured in profile '{}'", args.profile)),
                    })
                }
                Err(e) => Ok(BrowserSnapshotOutput {
                    success: false,
                    snapshot: None,
                    truncated: false,
                    ref_count: 0,
                    message: Some(format!("Snapshot failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserSnapshotOutput {
                success: false,
                snapshot: None,
                truncated: false,
                ref_count: 0,
                message: Some(format!("{e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_snapshot_returns_snapshot() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSnapshotTool::new(manager);

        let result = tool
            .call(BrowserSnapshotArgs {
                profile: "default".into(),
                interactive_only: false,
                compact: true,
                max_chars: None,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}
