// Browser PDF tool — prints the current page to a PDF file.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::builtin_tools::file_ops::{check_and_resolve_path, get_denied_paths};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_pdf` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserPdfArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Absolute path where the PDF file should be written. Subject to the same
    /// protected-location denylist as the file tools.
    pub output_path: String,
}

/// Output from the `browser_pdf` tool.
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
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserPdfTool {
    const NAME: &'static str = "browser_pdf";
    // The capability is one-sided: `BrowserBackend::pdf`'s default is served
    // by the existing-session backend and returns an error naming it, so a
    // description that promised PDF unconditionally cost the model a turn to
    // discover its profile could not print. `browser_emulate` states its own
    // split the same way.
    const DESCRIPTION: &'static str =
        "Print the current browser page to a PDF file at the given output path \
         — managed profiles only (e.g. profile='default')";
    type Args = BrowserPdfArgs;
    type Output = BrowserPdfOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // The write target goes through the FILE layer's sole path resolver —
        // the same one `file_ops` uses — before anything is printed. It applies
        // the credential denylist, the operator's `[sandbox] deny_read_globs`
        // floor and the worktree `FsScope` rebase. Without it `browser_pdf` was
        // a second face on the filesystem that the deny policy did not bind:
        // `file_write` refused to clobber `~/.ssh/config` while a print-to-PDF
        // at the same path went straight through, and an isolated run wrote
        // outside its worktree. `None` for the output-dir override means a
        // relative path is refused rather than silently landing somewhere —
        // this tool's contract is an absolute path.
        let path =
            match check_and_resolve_path(Path::new(&args.output_path), &get_denied_paths(), None) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(BrowserPdfOutput {
                        success: false,
                        output_path: None,
                        message: Some(e.to_string()),
                    });
                }
            };
        let resolved = path.display().to_string();
        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.pdf(&tab_id, &path).await {
                // Report the RESOLVED path: it is where the file actually
                // landed, which differs from what the caller asked for whenever
                // an `FsScope` rebase or a `~` expansion applied.
                Ok(()) => Ok(BrowserPdfOutput {
                    success: true,
                    output_path: Some(resolved.clone()),
                    message: Some(format!("Saved PDF to {resolved}")),
                }),
                Err(e) => Ok(BrowserPdfOutput {
                    success: false,
                    output_path: None,
                    message: Some(format!(
                        "PDF export failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserPdfOutput {
                success: false,
                output_path: None,
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
    async fn test_pdf_refuses_a_protected_output_path() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserPdfTool::new(manager);
        // `~/.ssh` is in the file layer's fixed credential denylist; printing a
        // page over a key file must be refused, and refused before the browser
        // is touched (the message is the deny verdict, not "no tabs open").
        let result = tool
            .call(BrowserPdfArgs {
                profile: "default".into(),
                output_path: "~/.ssh/id_rsa".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output_path.is_none());
        let message = result.message.unwrap();
        assert!(
            message.contains("protected location"),
            "expected a deny verdict, got: {message}"
        );
    }

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
