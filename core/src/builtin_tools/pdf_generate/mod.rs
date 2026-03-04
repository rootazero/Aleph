//! PDF generation tool for AI agent integration
//!
//! Implements AlephTool trait to provide PDF generation capabilities.
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
pub mod native_engine;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use async_trait::async_trait;

use super::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

pub use args::{ContentFormat, PageSize, PdfGenerateArgs, PdfGenerateOutput, RenderEngine};

/// PDF generation tool
#[derive(Clone)]
pub struct PdfGenerateTool {
    /// Default output directory
    default_output_dir: Option<PathBuf>,
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
    pub fn new() -> Self {
        Self {
            default_output_dir: None,
        }
    }

    /// Create with custom output directory
    pub fn with_output_dir(output_dir: PathBuf) -> Self {
        Self {
            default_output_dir: Some(output_dir),
        }
    }

    /// Resolve the output path from user-provided string
    ///
    /// Path resolution rules:
    /// 1. Absolute paths (starting with `/`) - used as-is
    /// 2. Home paths (starting with `~`) - expanded to home directory
    /// 3. Relative paths - resolved to output directory (~/.aleph/output/)
    fn resolve_output_path(&self, output_path: &str) -> std::result::Result<PathBuf, ToolError> {
        if output_path.starts_with('/') {
            Ok(PathBuf::from(output_path))
        } else if output_path.starts_with('~') {
            Ok(PathBuf::from(
                output_path
                    .replace('~', dirs::home_dir().unwrap_or_default().to_str().unwrap_or("")),
            ))
        } else if let Some(ref dir) = self.default_output_dir {
            Ok(dir.join(output_path))
        } else {
            // Use the default output directory for relative paths
            let output_dir = crate::utils::paths::get_output_dir().map_err(|e| {
                ToolError::Execution(format!("Failed to get output directory: {}", e))
            })?;
            Ok(output_dir.join(output_path))
        }
    }
}

impl Default for PdfGenerateTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of AlephTool trait for PdfGenerateTool
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

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let output_path = self.resolve_output_path(&args.output_path)?;
        // For now, always dispatch to native engine (browser engine added in Task 3)
        native_engine::generate(&args, &output_path).map_err(Into::into)
    }
}
