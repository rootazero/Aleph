//! Browser-based PDF rendering engine — MIGRATION IN PROGRESS.
//!
//! chromiumoxide was removed in the Playwright CLI migration.
//! Task 12 will rewrite this module to use PlaywrightCliDriver.

use std::path::Path;

use pulldown_cmark::{html, Options, Parser};

use super::args::{PdfGenerateArgs, PdfGenerateOutput};
use super::styles;
use crate::builtin_tools::error::ToolError;

/// Convert Markdown source to HTML fragment using pulldown-cmark.
///
/// Enables all extensions: tables, footnotes, strikethrough, task lists, etc.
pub fn markdown_to_html(markdown: &str) -> String {
    let options = Options::all();
    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// Build a complete HTML document from Markdown source.
///
/// Converts Markdown to HTML, then wraps with DOCTYPE, head, CSS, and body.
pub fn build_html_document(markdown: &str, title: Option<&str>) -> String {
    let html_body = markdown_to_html(markdown);
    styles::wrap_html_with_styles(&html_body, title)
}

/// Check whether a Chromium-based browser is available on the system.
pub fn is_chrome_available() -> bool {
    // Temporarily always false until Task 12 migrates this to playwright-cli.
    false
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub async fn generate(
    _args: &PdfGenerateArgs,
    _output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    let _ = html_escape; // keep helper for Task 12
    Err(ToolError::Execution(
        "PDF generation temporarily unavailable during playwright-cli migration (Task 12 pending)"
            .into(),
    ))
}
