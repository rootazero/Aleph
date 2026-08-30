//! PDF generation tool for AI agent integration
//!
//! Implements `AlephTool` trait to provide PDF generation capabilities.
//! Supports plain text and Markdown to PDF conversion with multiple rendering engines.
//!
//! # Engines
//!
//! - **Native** (printpdf): Fast, no external dependencies. Good for text and simple Markdown.
//! - **Browser** (headless Chrome): High-fidelity HTML/CSS rendering. Added in a later task.
//!
//! # Features
//!
//! - Plain text to PDF
//! - Markdown to PDF (headings, paragraphs, lists, code blocks)
//! - Chinese text support (requires system font)
//! - Configurable page size, margins, and fonts

pub mod args;
mod browser_engine;
pub mod native_engine;
mod styles;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tracing::{info, warn};

use super::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

pub use args::{ContentFormat, PageSize, PdfGenerateArgs, PdfGenerateOutput, RenderEngine};

/// PDF generation tool
#[derive(Clone)]
pub struct PdfGenerateTool {
    /// Handle to workspace-scoped tool context (provides `output_dir`, etc.)
    pub tool_context_handle: Option<crate::tools::ToolContextHandle>,
    /// The managed browser driver's configuration, so the browser engine
    /// resolves the same `playwright-cli` the browser tools do.
    ///
    /// `None` only where no browser subsystem exists (tests, ad-hoc
    /// construction); the registry always supplies it.
    pub playwright_config: Option<crate::browser::profile::PlaywrightCliConfig>,
}

impl PdfGenerateTool {
    /// Tool identifier
    pub const NAME: &'static str = "pdf_generate";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str =
        "Generate PDF documents from text or Markdown content.\n\n\
Features:\n\
- Plain text to PDF conversion\n\
- Markdown support (headings, paragraphs, lists, code blocks, bold, italic)\n\
- Configurable page size (A4, Letter, A3, or custom)\n\
- Adjustable font size, line spacing, and margins\n\n\
PATH RESOLUTION:\n\
- Relative paths (e.g., \"article.pdf\") → the run's output dir\n\
- Home paths (e.g., \"~/Desktop/doc.pdf\") → expanded to user's home directory\n\
- Absolute paths (e.g., \"/Users/name/doc.pdf\") → used as-is\n\n\
DEFAULT OUTPUT: Use relative paths like \"article.pdf\" or \"translated.pdf\" for generated PDFs.";

    /// Create a new PDF generation tool
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tool_context_handle: None,
            playwright_config: None,
        }
    }

    /// Attach a `ToolContext` handle for workspace-scoped output path resolution
    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    /// Adopt the managed browser driver's `playwright-cli` settings.
    ///
    /// The registry must call this: without it the engine builds a driver from
    /// `PlaywrightCliConfig::default()` and resolves its binary as though no
    /// `binary_path` had ever been configured — a second construction site
    /// inheriting none of the first's settings.
    #[must_use]
    pub fn with_playwright_config(
        mut self,
        config: crate::browser::profile::PlaywrightCliConfig,
    ) -> Self {
        self.playwright_config = Some(config);
        self
    }

    /// Resolve the output path from user-provided string
    ///
    /// Path resolution rules:
    /// 1. Absolute paths are coerced to relative — `resolve_output_path`
    ///    joins them onto the per-run `FsScope` / workspace output dir
    ///    rather than honouring the absolute prefix. An LLM-supplied
    ///    absolute path bypassed the FsScope sandbox entirely (BT-C-R4-01).
    ///    The base name is preserved; only the parent is rewritten.
    /// 2. Home paths (starting with `~`) - expanded and joined onto the
    ///    workspace output dir, same reasoning.
    /// 3. Relative paths - anchored at the per-run `FsScope` base, falling back
    ///    to the shared `ToolContext` `output_dir`, then a global default
    async fn resolve_output_path(
        &self,
        output_path: &str,
    ) -> std::result::Result<PathBuf, ToolError> {
        let output_path = Path::new(output_path);

        // BT-C-R4-01: previously absolute paths were returned as-is,
        // bypassing the FsScope sandbox and `create_dir_all`-ing
        // arbitrary parent directories (an LLM-supplied
        // `/tmp/alice/.ssh/authorized_keys` would have created `.ssh/`
        // and let the agent write a public key there). Now we always
        // anchor under the workspace output dir; only the file name is
        // taken from the model. This brings pdf_generate in line with
        // what file_ops::check_and_resolve_path does for read/write.
        if output_path.is_absolute() {
            let filename = output_path.file_name().ok_or_else(|| {
                ToolError::InvalidArgs(
                    "absolute output path has no file name component".to_string(),
                )
            })?;
            // Inline the base-dir resolution (the existing relative-path
            // branch below) so the absolute and `~` cases anchor under
            // the same FsScope / workspace as everything else. BT-C-R4-01
            // closes the prior bypass where absolute paths wrote
            // anywhere on the host.
            let output_dir = self.choose_output_dir().await?;
            return Ok(output_dir.join(filename));
        }

        if let Some(s) = output_path.to_str() {
            if s.starts_with('~') {
                // BT-C-R4-01: same — take only the file name component and
                // anchor under the workspace, so `~/foo.pdf` resolves to
                // `<workspace>/foo.pdf` not `~/foo.pdf`.
                let filename = output_path.file_name().ok_or_else(|| {
                    ToolError::InvalidArgs("~-prefixed path has no file name component".to_string())
                })?;
                let output_dir = self.choose_output_dir().await?;
                return Ok(output_dir.join(filename));
            }
        }

        // For relative paths, anchor at the per-run `FsScope` task-local first
        // (per-run truth, immune to a concurrent run rewriting the shared
        // `ToolContextHandle` mid-run — the same precedence file tools use in
        // `file_ops::check_and_resolve_path`). A normal run's scope base is
        // already `<workspace>/output/documents`, so this is identical to the
        // shared-handle branch below for non-isolated runs; a worktree-isolated
        // agent anchors at its checkout root, matching where its file writes go.
        // Falls back to the shared handle, then a global default, outside any scope.
        let output_dir = if let Some(scope) = crate::tools::fs_scope::current() {
            scope.base
        } else if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            ctx.output_dir.join("documents")
        } else {
            crate::utils::paths::get_workspaces_dir()
                .map_err(|_| {
                    ToolError::Execution(
                        "Cannot determine home directory for output path".to_string(),
                    )
                })?
                .join("main")
                .join("output")
                .join("documents")
        };

        Ok(output_dir.join(output_path))
    }

    /// BT-C-R4-01: helper that returns the workspace base directory used
    /// for relative-path anchoring. Extracted so the absolute / `~` cases
    /// can reuse the exact same FsScope / ToolContext precedence rules.
    async fn choose_output_dir(&self) -> std::result::Result<PathBuf, ToolError> {
        if let Some(scope) = crate::tools::fs_scope::current() {
            Ok(scope.base)
        } else if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Ok(ctx.output_dir.join("documents"))
        } else {
            crate::utils::paths::get_workspaces_dir()
                .map_err(|_| {
                    ToolError::Execution(
                        "Cannot determine home directory for output path".to_string(),
                    )
                })
                .map(|p| p.join("main").join("output").join("documents"))
        }
    }
}

impl Default for PdfGenerateTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of `AlephTool` trait for `PdfGenerateTool`
#[async_trait]
impl AlephTool for PdfGenerateTool {
    const NAME: &'static str = "pdf_generate";
    const DESCRIPTION: &'static str = Self::DESCRIPTION;

    type Args = PdfGenerateArgs;
    type Output = PdfGenerateOutput;

    async fn call(&self, mut args: Self::Args) -> Result<Self::Output> {
        // Auto-detect Markdown when format is Text (the default)
        if matches!(args.format, ContentFormat::Text) {
            let detected = ContentFormat::detect(&args.content);
            if matches!(detected, ContentFormat::Markdown) {
                info!("Auto-detected Markdown content, switching format to Markdown");
                args.format = ContentFormat::Markdown;
            }
        }

        let output_path = self.resolve_output_path(&args.output_path).await?;

        let result = match args.render_engine {
            RenderEngine::Browser => {
                browser_engine::generate(&args, &output_path, self.playwright_config.as_ref()).await
            }
            RenderEngine::Native => {
                // `native_engine::generate` is sync CPU+IO (markdown parse, font
                // file read, `printpdf` build, `std::fs::write`) — running it
                // inline stalls the tokio worker for the full document. Move
                // it to the blocking pool.
                let output_path = output_path.clone();
                tokio::task::spawn_blocking(move || native_engine::generate(&args, &output_path))
                    .await
                    .map_err(|e| crate::builtin_tools::error::ToolError::Execution(format!(
                        "pdf_generate join failed: {e}"
                    )))?
            }
            RenderEngine::Auto => {
                if browser_engine::is_browser_engine_available(self.playwright_config.as_ref()) {
                    match browser_engine::generate(
                        &args,
                        &output_path,
                        self.playwright_config.as_ref(),
                    )
                    .await
                    {
                        Ok(output) => Ok(output),
                        Err(e) => {
                            warn!(error = %e, "Browser engine failed, falling back to native");
                            let output_path = output_path.clone();
                            tokio::task::spawn_blocking(move || {
                                native_engine::generate(&args, &output_path)
                            })
                            .await
                            .map_err(|e| crate::builtin_tools::error::ToolError::Execution(format!(
                                "pdf_generate join failed: {e}"
                            )))?
                        }
                    }
                } else {
                    info!("Chrome not available, using native PDF engine");
                    let output_path = output_path.clone();
                    tokio::task::spawn_blocking(move || native_engine::generate(&args, &output_path))
                        .await
                        .map_err(|e| crate::builtin_tools::error::ToolError::Execution(format!(
                            "pdf_generate join failed: {e}"
                        )))?
                }
            }
        };

        result.map_err(Into::into)
    }
}
