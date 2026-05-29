// Browser PDF tool — prints the current page to a PDF file.

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_pdf tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserPdfArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Absolute path where the PDF file should be written.
    pub output_path: String,
}

/// Output from the browser_pdf tool.
#[derive(Debug, Serialize)]
pub struct BrowserPdfOutput {
    pub success: bool,
    pub output_path: Option<String>,
    pub message: Option<String>,
}

/// Prints the current browser page to a PDF file.
#[derive(Clone)]
pub struct BrowserPdfTool {
    manager: Arc<ProfileManager>,
}

impl BrowserPdfTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserPdfTool {
    const NAME: &'static str = "browser_pdf";
    const DESCRIPTION: &'static str =
        "Print the current browser page to a PDF file at the given output path";
    type Args = BrowserPdfArgs;
    type Output = BrowserPdfOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let path = PathBuf::from(&args.output_path);
        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.pdf(&tab_id, &path).await {
                Ok(()) => Ok(BrowserPdfOutput {
                    success: true,
                    output_path: Some(args.output_path.clone()),
                    message: Some(format!("Saved PDF to {}", args.output_path)),
                }),
                Err(e) => Ok(BrowserPdfOutput {
                    success: false,
                    output_path: None,
                    message: Some(format!("PDF export failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserPdfOutput {
                success: false,
                output_path: None,
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
    async fn test_pdf_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserPdfTool::new(manager);
        let result = tool
            .call(BrowserPdfArgs {
                profile: "default".into(),
                output_path: "/tmp/aleph-test-page.pdf".into(),
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}
