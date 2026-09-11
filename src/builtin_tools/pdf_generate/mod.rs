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

use std::borrow::Cow;
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
    /// Where the browser engine's Chromium comes from — the same
    /// `[browser.runtime]` the browser tools resolve through.
    ///
    /// Separate from `playwright_config` because they answer different
    /// questions (which CLI vs which browser) and the registry supplies them
    /// from two different places on `BrowserSystemConfig`. `None` has the same
    /// meaning as above.
    pub browser_runtime: Option<crate::browser::profile::BrowserRuntimeConfig>,
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
            browser_runtime: None,
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

    /// Adopt the browser subsystem's `[browser.runtime]` settings.
    ///
    /// The registry must call this for the same reason it must call
    /// [`Self::with_playwright_config`], one layer down: this engine now
    /// launches a Chromium of its own, so without it an operator's pinned
    /// binary is honoured by the browser tools and silently ignored here —
    /// two answers to "which Chromium", neither of them stated at runtime.
    #[must_use]
    pub fn with_browser_runtime(
        mut self,
        runtime: crate::browser::profile::BrowserRuntimeConfig,
    ) -> Self {
        self.browser_runtime = Some(runtime);
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
        use crate::builtin_tools::file_ops::{check_and_resolve_path, get_denied_paths};

        let output_path = Path::new(output_path);

        // BT-C-R4-01 + denylist gate: previously absolute paths were
        // returned as-is, bypassing the FsScope sandbox and
        // `create_dir_all`-ing arbitrary parent directories (an
        // LLM-supplied `/tmp/alice/.ssh/authorized_keys` would have
        // created `.ssh/` and let the agent write a public key
        // there). The relative-path branch had a similar gap: it joined
        // the LLM-supplied path onto `output_dir` without canonicalizing
        // or checking that the result stayed inside `output_dir` (a
        // `../../tmp/evil.pdf` escapes the FsScope). Now both branches
        // route through the shared `check_and_resolve_path` so the
        // credential denylist (~/.ssh, ~/.aws, ~/.netrc, /etc/passwd,
        // /etc/shadow, /etc/sudoers, ~/.aleph/secrets.vault,
        // ~/.aleph/data, …), the operator's `[sandbox] deny_read_globs`,
        // and the /proc secret-leaves block all run.
        //
        // For absolute / `~` paths we first rewrite to the basename and
        // anchor under the per-run FsScope / workspace output dir, then
        // pass the rewritten path through `check_and_resolve_path` so the
        // denylist still applies (e.g. a basename that itself matches a
        // denied pattern).
        let output_dir = self.choose_output_dir().await?;
        let input = if output_path.is_absolute() {
            let filename = output_path.file_name().ok_or_else(|| {
                ToolError::InvalidArgs(
                    "absolute output path has no file name component".to_string(),
                )
            })?;
            Cow::Owned(output_dir.join(filename))
        } else if output_path
            .to_str()
            .map(|s| s.starts_with('~'))
            .unwrap_or(false)
        {
            let filename = output_path.file_name().ok_or_else(|| {
                ToolError::InvalidArgs("~-prefixed path has no file name component".to_string())
            })?;
            Cow::Owned(output_dir.join(filename))
        } else {
            Cow::Borrowed(output_path)
        };

        // `check_and_resolve_path` canonicalizes the input, applies the
        // denylist, and (when `output_dir_override` is Some) joins
        // relative paths onto the override — so the relative branch is
        // anchored at the same `output_dir` the absolute / `~` branches
        // already use.
        let denied = get_denied_paths();
        let resolved = check_and_resolve_path(&input, &denied, Some(&output_dir))?;

        // Containment: the resolved path MUST sit inside the per-run
        // output dir. `check_and_resolve_path` canonicalizes, so this is
        // a real check even for paths that exist on disk and even when
        // the input went through a `..` component.
        let canonical_output_dir = output_dir
            .canonicalize()
            .unwrap_or_else(|_| output_dir.clone());
        if !resolved.starts_with(&canonical_output_dir) {
            return Err(ToolError::InvalidArgs(format!(
                "output_path escapes the workspace output dir: {}",
                output_path.display()
            )));
        }

        Ok(resolved)
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
                browser_engine::generate(
                    &args,
                    &output_path,
                    self.playwright_config.as_ref(),
                    self.browser_runtime.as_ref(),
                )
                .await
            }
            RenderEngine::Native => {
                // `native_engine::generate` is sync CPU+IO (markdown parse, font
                // file read, `printpdf` build, `std::fs::write`) — running it
                // inline stalls the tokio worker for the full document. Move
                // it to the blocking pool.
                let output_path = output_path.clone();
                tokio::task::spawn_blocking(move || native_engine::generate(&args, &output_path))
                    .await
                    .map_err(|e| {
                        crate::builtin_tools::error::ToolError::Execution(format!(
                            "pdf_generate join failed: {e}"
                        ))
                    })?
            }
            RenderEngine::Auto => {
                if browser_engine::is_browser_engine_available(self.playwright_config.as_ref()) {
                    match browser_engine::generate(
                        &args,
                        &output_path,
                        self.playwright_config.as_ref(),
                        self.browser_runtime.as_ref(),
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
                            .map_err(|e| {
                                crate::builtin_tools::error::ToolError::Execution(format!(
                                    "pdf_generate join failed: {e}"
                                ))
                            })?
                        }
                    }
                } else {
                    info!("Chrome not available, using native PDF engine");
                    let output_path = output_path.clone();
                    tokio::task::spawn_blocking(move || {
                        native_engine::generate(&args, &output_path)
                    })
                    .await
                    .map_err(|e| {
                        crate::builtin_tools::error::ToolError::Execution(format!(
                            "pdf_generate join failed: {e}"
                        ))
                    })?
                }
            }
        };

        result.map_err(Into::into)
    }
}
