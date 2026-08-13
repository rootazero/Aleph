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
    pub const DESCRIPTION: &'static str = "Generate PDF documents from text or Markdown content.\n\n\
Features:\n\
- Plain text to PDF conversion\n\
- Markdown support (headings, paragraphs, lists, code blocks, bold, italic)\n\
- Configurable page size (A4, Letter, A3, or custom)\n\
- Adjustable font size, line spacing, and margins\n\n\
PATH RESOLUTION:\n\
- Relative paths (e.g., \"article.pdf\") → saved to ~/.aleph/output/\n\
- Home paths (e.g., \"~/Desktop/doc.pdf\") → expanded to user's home directory\n\
- Absolute paths (e.g., \"/Users/name/doc.pdf\") → used as-is\n\n\
DEFAULT OUTPUT: Use relative paths like \"article.pdf\" or \"translated.pdf\" for generated PDFs. They will be saved to the default output directory (~/.aleph/output/), which is always writable.\n\n\
Examples:\n\
- Simple: {\"content\": \"Hello World\", \"output_path\": \"hello.pdf\"}\n\
- With title: {\"content\": \"Document content\", \"output_path\": \"doc.pdf\", \"title\": \"My Document\"}\n\
- Markdown: {\"content\": \"# Heading\", \"output_path\": \"doc.pdf\", \"format\": \"markdown\"}";

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
    /// 1. Absolute paths (starting with `/`) - used as-is
    /// 2. Home paths (starting with `~`) - expanded to home directory
    /// 3. Relative paths - anchored at the per-run `FsScope` base, falling back
    ///    to the shared `ToolContext` `output_dir`, then a global default
    async fn resolve_output_path(
        &self,
        output_path: &str,
    ) -> std::result::Result<PathBuf, ToolError> {
        let output_path = Path::new(output_path);

        if output_path.is_absolute() {
            return Ok(output_path.to_path_buf());
        }

        if let Some(s) = output_path.to_str() {
            if s.starts_with('~') {
                let home = dirs::home_dir().ok_or_else(|| {
                    ToolError::Execution("Cannot resolve '~': home directory not found".to_string())
                })?;
                return Ok(PathBuf::from(s.replacen('~', &home.to_string_lossy(), 1)));
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
            dirs::home_dir()
                .ok_or_else(|| {
                    ToolError::Execution(
                        "Cannot determine home directory for output path".to_string(),
                    )
                })?
                .join(".aleph")
                .join("workspaces")
                .join("main")
                .join("output")
                .join("documents")
        };

        Ok(output_dir.join(output_path))
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
    const DESCRIPTION: &'static str = "Generate PDF documents from text or Markdown content.\n\n\
Features:\n\
- Plain text to PDF conversion\n\
- Markdown support (headings, paragraphs, lists, code blocks, bold, italic)\n\
- Configurable page size (A4, Letter, A3, or custom)\n\
- Adjustable font size, line spacing, and margins\n\n\
PATH RESOLUTION:\n\
- Relative paths (e.g., \"article.pdf\") → saved to ~/.aleph/output/\n\
- Home paths (e.g., \"~/Desktop/doc.pdf\") → expanded to user's home directory\n\
- Absolute paths (e.g., \"/Users/name/doc.pdf\") → used as-is\n\n\
DEFAULT OUTPUT: Use relative paths like \"article.pdf\" or \"translated.pdf\" for generated PDFs.";

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
            RenderEngine::Native => native_engine::generate(&args, &output_path),
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
                            native_engine::generate(&args, &output_path)
                        }
                    }
                } else {
                    info!("Chrome not available, using native PDF engine");
                    native_engine::generate(&args, &output_path)
                }
            }
        };

        result.map_err(Into::into)
    }
}
