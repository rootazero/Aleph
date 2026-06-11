// Browser snapshot tool — captures an accessibility tree snapshot of the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_snapshot` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSnapshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Maximum output characters (default: 30000). Set higher for complex pages.
    pub max_chars: Option<usize>,
}

/// Output from the `browser_snapshot` tool.
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
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
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
        // Text-first: backend.snapshot() returns raw YAML/indented-tree text already.
        let max_chars = args.max_chars.unwrap_or(super::DEFAULT_CONTENT_MAX_CHARS);

        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.snapshot(&tab_id).await {
                Ok(snap) => {
                    // Bound first (line-boundary, never splitting a `[ref=]` token),
                    // then count refs on the EMITTED text so the reported count
                    // matches exactly what the model can see and act on.
                    let (text, truncated) = super::bound_content(&snap.snapshot_text, max_chars);
                    let ref_count = text.matches("[ref=").count();
                    // Page-derived DOM text is untrusted external content: scrub
                    // embedded credentials, then wrap with the injection boundary
                    // so chat-template markers injected by a hostile page cannot
                    // escape (see `redact_wrap`).
                    let wrapped = super::redact_wrap(&self.manager, &text);
                    Ok(BrowserSnapshotOutput {
                        success: true,
                        snapshot: Some(wrapped),
                        truncated,
                        ref_count,
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
                max_chars: None,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}
