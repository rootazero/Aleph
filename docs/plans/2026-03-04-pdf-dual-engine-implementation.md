# PDF Dual-Engine Rendering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance `PdfGenerateTool` with a browser-based Markdown→HTML+CSS→Chrome→PDF rendering path, keeping the existing printpdf path as fallback.

**Architecture:** Split the monolithic `pdf_generate.rs` (849 lines) into a module directory. Add a browser engine that converts Markdown to HTML, wraps it with GitHub-flavored CSS, then uses `chromiumoxide` (already a project dependency) to render via headless Chrome's `Page.printToPDF`. Engine selection is automatic (browser preferred, native fallback) with an optional explicit `render_engine` field.

**Tech Stack:** Rust, `pulldown-cmark` 0.12 (Markdown→HTML via `push_html`), `chromiumoxide` 0.7 (`Page::pdf(PrintToPdfParams)`), `printpdf` 0.7 (native fallback)

---

### Task 1: Split `pdf_generate.rs` into module directory

**Files:**
- Delete: `core/src/builtin_tools/pdf_generate.rs`
- Create: `core/src/builtin_tools/pdf_generate/mod.rs`
- Create: `core/src/builtin_tools/pdf_generate/args.rs`
- Create: `core/src/builtin_tools/pdf_generate/native_engine.rs`
- Create: `core/src/builtin_tools/pdf_generate/tests.rs`
- Verify: `core/src/builtin_tools/mod.rs` (no change needed — `pub mod pdf_generate` works for both file and directory modules)

**Step 1: Create `args.rs` with all types**

Extract types from `pdf_generate.rs` into `core/src/builtin_tools/pdf_generate/args.rs`:

```rust
//! PDF generation arguments and types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Page size options
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum PageSize {
    #[default]
    A4,
    Letter,
    A3,
    Custom { width_mm: f32, height_mm: f32 },
}

impl PageSize {
    pub fn dimensions_mm(&self) -> (f32, f32) {
        match self {
            PageSize::A4 => (210.0, 297.0),
            PageSize::Letter => (215.9, 279.4),
            PageSize::A3 => (297.0, 420.0),
            PageSize::Custom { width_mm, height_mm } => (*width_mm, *height_mm),
        }
    }

    /// Dimensions in inches (for Chrome PrintToPDF)
    pub fn dimensions_inches(&self) -> (f64, f64) {
        let (w, h) = self.dimensions_mm();
        (w as f64 / 25.4, h as f64 / 25.4)
    }
}

/// Content format
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContentFormat {
    #[default]
    Text,
    Markdown,
}

/// Rendering engine preference
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderEngine {
    /// Auto-detect best available engine (browser preferred, native fallback)
    #[default]
    Auto,
    /// Force headless browser rendering (requires Chrome/Chromium)
    Browser,
    /// Force native printpdf rendering
    Native,
}

fn default_font_size() -> f32 { 12.0 }
fn default_line_spacing() -> f32 { 1.5 }
fn default_margin() -> f32 { 20.0 }

/// Arguments for PDF generation tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PdfGenerateArgs {
    /// Content to convert to PDF
    pub content: String,
    /// Output file path
    pub output_path: String,
    /// Content format (text or markdown)
    #[serde(default)]
    pub format: ContentFormat,
    /// Page size
    #[serde(default)]
    pub page_size: PageSize,
    /// Title (optional, shown at top of first page)
    #[serde(default)]
    pub title: Option<String>,
    /// Font size in points (default: 12)
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Line spacing multiplier (default: 1.5)
    #[serde(default = "default_line_spacing")]
    pub line_spacing: f32,
    /// Page margins in mm (default: 20)
    #[serde(default = "default_margin")]
    pub margin_mm: f32,
    /// Rendering engine (default: auto-detect)
    #[serde(default)]
    pub render_engine: RenderEngine,
}

/// Output from PDF generation tool
#[derive(Debug, Clone, Serialize)]
pub struct PdfGenerateOutput {
    pub success: bool,
    pub output_path: String,
    pub pages: usize,
    pub message: String,
}
```

**Step 2: Create `native_engine.rs` with existing printpdf code**

Move the entire `generate()` method, `wrap_text()`, `render_text()`, `is_cjk()`, and `find_system_font()` from the old `pdf_generate.rs` into `core/src/builtin_tools/pdf_generate/native_engine.rs`:

```rust
//! Native PDF engine using printpdf.
//!
//! Low-level PDF generation without external dependencies.
//! Used as fallback when headless Chrome is unavailable.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use printpdf::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use tracing::{debug, warn};

use super::args::{ContentFormat, PdfGenerateArgs, PdfGenerateOutput};
use crate::builtin_tools::error::ToolError;

/// Generate PDF using native printpdf engine.
///
/// This is the fallback engine that works without external dependencies.
pub fn generate(args: &PdfGenerateArgs, output_path: &std::path::Path) -> Result<PdfGenerateOutput, ToolError> {
    // ... (entire existing generate() method body, adapted to take output_path as parameter)
    // Copy lines 209-568 from the original pdf_generate.rs
    // Change: use args by reference, accept resolved output_path
}

// ... (copy all helper functions: is_cjk, find_system_font, wrap_text, render_text)
// These are identical to the original code.
```

Key change: `generate()` takes `&PdfGenerateArgs` + resolved `&Path` instead of owning args and resolving path internally.

**Step 3: Create `mod.rs` with tool struct and dispatch**

Create `core/src/builtin_tools/pdf_generate/mod.rs`:

```rust
//! PDF generation tool for AI agent integration.
//!
//! Supports dual rendering engines:
//! - **Browser engine**: Markdown → HTML+CSS → headless Chrome → PDF (high quality)
//! - **Native engine**: Markdown → printpdf (fallback, no external deps)

mod args;
mod native_engine;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use async_trait::async_trait;
use tracing::{info, warn};

pub use args::{
    ContentFormat, PageSize, PdfGenerateArgs, PdfGenerateOutput, RenderEngine,
};
use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

/// PDF generation tool
#[derive(Clone)]
pub struct PdfGenerateTool {
    default_output_dir: Option<PathBuf>,
}

impl PdfGenerateTool {
    pub const NAME: &'static str = "pdf_generate";

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
DEFAULT OUTPUT: Use relative paths like \"article.pdf\" or \"translated.pdf\" for generated PDFs.";

    pub fn new() -> Self {
        Self { default_output_dir: None }
    }

    pub fn with_output_dir(output_dir: PathBuf) -> Self {
        Self { default_output_dir: Some(output_dir) }
    }

    /// Resolve output path from user-provided string
    fn resolve_output_path(&self, output_path: &str) -> std::result::Result<PathBuf, ToolError> {
        // ... (extract path resolution logic from original lines 519-541)
    }

    /// Generate PDF, dispatching to the appropriate engine
    async fn generate(&self, args: PdfGenerateArgs) -> std::result::Result<PdfGenerateOutput, ToolError> {
        let output_path = self.resolve_output_path(&args.output_path)?;

        // Create parent directories
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::Execution(format!("Failed to create output directory: {}", e))
            })?;
        }

        // For now, delegate to native engine (browser engine added in Task 3)
        native_engine::generate(&args, &output_path)
    }
}

impl Default for PdfGenerateTool {
    fn default() -> Self { Self::new() }
}

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
        self.generate(args).await.map_err(Into::into)
    }
}
```

**Step 4: Create `tests.rs` with existing tests**

Move all tests from original file to `core/src/builtin_tools/pdf_generate/tests.rs`:

```rust
use super::*;
use super::args::*;

// ... (copy all existing tests unchanged)
```

**Step 5: Delete old file and verify compilation**

```bash
rm core/src/builtin_tools/pdf_generate.rs
cargo check -p alephcore
```

Expected: Compiles successfully. The `pub mod pdf_generate` in `builtin_tools/mod.rs` now resolves to the directory module.

**Step 6: Run tests to verify no regressions**

```bash
cargo test -p alephcore --lib pdf_generate
```

Expected: All 6 existing tests pass (test_wrap_text, test_page_size_dimensions, test_simple_pdf_generation, test_markdown_pdf_generation, test_chinese_pdf_generation, test_wrap_text_cjk, test_is_cjk).

**Step 7: Commit**

```bash
git add core/src/builtin_tools/pdf_generate/
git add -u core/src/builtin_tools/pdf_generate.rs
git commit -m "refactor(pdf): split pdf_generate.rs into module directory"
```

---

### Task 2: Add CSS stylesheet for browser engine

**Files:**
- Create: `core/src/builtin_tools/pdf_generate/styles.rs`

**Step 1: Write the test**

Add to `tests.rs`:

```rust
#[test]
fn test_css_contains_essential_rules() {
    let css = super::styles::github_markdown_css();
    assert!(css.contains("font-family"), "CSS must set font-family");
    assert!(css.contains("code"), "CSS must style code blocks");
    assert!(css.contains("table"), "CSS must style tables");
    assert!(css.contains("blockquote"), "CSS must style blockquotes");
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib pdf_generate::tests::test_css_contains_essential_rules
```

Expected: FAIL — module `styles` not found.

**Step 3: Implement `styles.rs`**

Create `core/src/builtin_tools/pdf_generate/styles.rs`:

```rust
//! Embedded CSS stylesheets for browser-based PDF rendering.

/// GitHub-flavored Markdown CSS for PDF output.
///
/// Optimized for print media with:
/// - CJK font fallback chain
/// - Code block syntax highlighting base
/// - Table styling with alternating rows
/// - Print-friendly page break hints
pub fn github_markdown_css() -> &'static str {
    r#"
    * {
        margin: 0;
        padding: 0;
        box-sizing: border-box;
    }

    body {
        font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial,
                     "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC",
                     sans-serif;
        font-size: 14px;
        line-height: 1.6;
        color: #24292e;
        padding: 2em;
        max-width: 100%;
    }

    /* Headings */
    h1, h2, h3, h4, h5, h6 {
        margin-top: 1.5em;
        margin-bottom: 0.5em;
        font-weight: 600;
        line-height: 1.25;
    }
    h1 { font-size: 2em; border-bottom: 1px solid #eaecef; padding-bottom: 0.3em; }
    h2 { font-size: 1.5em; border-bottom: 1px solid #eaecef; padding-bottom: 0.3em; }
    h3 { font-size: 1.25em; }
    h4 { font-size: 1em; }
    h5 { font-size: 0.875em; }
    h6 { font-size: 0.85em; color: #6a737d; }

    /* Paragraphs */
    p {
        margin-bottom: 1em;
    }

    /* Links */
    a {
        color: #0366d6;
        text-decoration: none;
    }

    /* Lists */
    ul, ol {
        padding-left: 2em;
        margin-bottom: 1em;
    }
    li {
        margin-bottom: 0.25em;
    }
    li > ul, li > ol {
        margin-bottom: 0;
    }

    /* Code */
    code {
        font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo,
                     "Courier New", monospace;
        font-size: 0.85em;
        background-color: rgba(27, 31, 35, 0.05);
        border-radius: 3px;
        padding: 0.2em 0.4em;
    }
    pre {
        background-color: #f6f8fa;
        border-radius: 6px;
        padding: 16px;
        overflow-x: auto;
        margin-bottom: 1em;
        line-height: 1.45;
    }
    pre code {
        background-color: transparent;
        padding: 0;
        font-size: 0.85em;
    }

    /* Tables */
    table {
        border-collapse: collapse;
        width: 100%;
        margin-bottom: 1em;
    }
    th, td {
        border: 1px solid #dfe2e5;
        padding: 6px 13px;
        text-align: left;
    }
    th {
        font-weight: 600;
        background-color: #f6f8fa;
    }
    tr:nth-child(even) {
        background-color: #f6f8fa;
    }

    /* Blockquotes */
    blockquote {
        border-left: 4px solid #dfe2e5;
        color: #6a737d;
        padding: 0 1em;
        margin-bottom: 1em;
    }

    /* Horizontal rules */
    hr {
        border: none;
        border-top: 1px solid #eaecef;
        margin: 1.5em 0;
    }

    /* Images */
    img {
        max-width: 100%;
        height: auto;
    }

    /* Print-specific */
    @media print {
        body { padding: 0; }
        h1, h2, h3 { break-after: avoid; }
        pre, blockquote, table { break-inside: avoid; }
        p { orphans: 3; widows: 3; }
    }
    "#
}

/// Wrap HTML content with full document structure and CSS.
pub fn wrap_html_with_styles(html_body: &str, title: Option<&str>) -> String {
    let css = github_markdown_css();
    let title_str = title.unwrap_or("Document");
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title_str}</title>
    <style>{css}</style>
</head>
<body>
{html_body}
</body>
</html>"#
    )
}
```

**Step 4: Add `mod styles;` to `mod.rs`**

Add `mod styles;` (not `pub mod`) to `core/src/builtin_tools/pdf_generate/mod.rs` after `mod native_engine;`.

**Step 5: Run test to verify it passes**

```bash
cargo test -p alephcore --lib pdf_generate::tests::test_css_contains_essential_rules
```

Expected: PASS

**Step 6: Commit**

```bash
git add core/src/builtin_tools/pdf_generate/styles.rs
git commit -m "pdf: add GitHub-flavored CSS stylesheet for browser engine"
```

---

### Task 3: Implement browser engine

**Files:**
- Create: `core/src/builtin_tools/pdf_generate/browser_engine.rs`
- Modify: `core/src/builtin_tools/pdf_generate/mod.rs` — add engine dispatch

**Step 1: Write the tests**

Add to `tests.rs`:

```rust
#[test]
fn test_markdown_to_html_conversion() {
    let md = "# Hello\n\nThis is **bold** and *italic*.\n\n- item 1\n- item 2\n";
    let html = super::browser_engine::markdown_to_html(md);
    assert!(html.contains("<h1>"), "Should convert # to <h1>");
    assert!(html.contains("<strong>"), "Should convert ** to <strong>");
    assert!(html.contains("<em>"), "Should convert * to <em>");
    assert!(html.contains("<li>"), "Should convert - to <li>");
}

#[test]
fn test_markdown_to_html_with_table() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
    let html = super::browser_engine::markdown_to_html(md);
    assert!(html.contains("<table>"), "Should convert table syntax");
    assert!(html.contains("<th>"), "Should have table headers");
}

#[test]
fn test_markdown_to_html_with_code_block() {
    let md = "```rust\nfn main() {}\n```\n";
    let html = super::browser_engine::markdown_to_html(md);
    assert!(html.contains("<pre>"), "Should wrap code in <pre>");
    assert!(html.contains("<code>"), "Should wrap code in <code>");
}

#[test]
fn test_build_full_html_document() {
    let md = "# Test\n\nContent here.\n";
    let doc = super::browser_engine::build_html_document(md, Some("My Title"));
    assert!(doc.contains("<!DOCTYPE html>"), "Should be full HTML document");
    assert!(doc.contains("My Title"), "Should include title");
    assert!(doc.contains("<h1>"), "Should contain rendered markdown");
    assert!(doc.contains("font-family"), "Should contain CSS");
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib pdf_generate::tests::test_markdown_to_html
```

Expected: FAIL — module `browser_engine` not found.

**Step 3: Implement `browser_engine.rs`**

Create `core/src/builtin_tools/pdf_generate/browser_engine.rs`:

```rust
//! Browser-based PDF engine using headless Chrome.
//!
//! Renders Markdown → HTML+CSS → PDF via chromiumoxide.
//! Produces high-quality PDFs with proper typography, tables, and code blocks.

use std::path::Path;

use chromiumoxide::browser::{Browser, BrowserConfig as CdpBrowserConfig};
use chromiumoxide_cdp::cdp::browser_protocol::page::PrintToPdfParams;
use futures::StreamExt;
use pulldown_cmark::{Options, Parser, html::push_html};
use tracing::{debug, info, warn};

use super::args::{PdfGenerateArgs, PdfGenerateOutput};
use super::styles;
use crate::browser::find_chromium;
use crate::builtin_tools::error::ToolError;

/// Convert Markdown to HTML string using pulldown-cmark.
pub fn markdown_to_html(markdown: &str) -> String {
    let options = Options::all();
    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    push_html(&mut html_output, parser);
    html_output
}

/// Build a complete HTML document from Markdown with embedded CSS.
pub fn build_html_document(markdown: &str, title: Option<&str>) -> String {
    let html_body = markdown_to_html(markdown);
    styles::wrap_html_with_styles(&html_body, title)
}

/// Check if a headless Chrome browser is available.
pub fn is_chrome_available() -> bool {
    find_chromium().is_ok()
}

/// Generate PDF using headless Chrome.
///
/// Flow: Markdown → HTML+CSS → temp file → Chrome → PDF bytes → output file
pub async fn generate(
    args: &PdfGenerateArgs,
    output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    // Step 1: Build HTML document
    let html = match args.format {
        super::args::ContentFormat::Markdown => {
            build_html_document(&args.content, args.title.as_deref())
        }
        super::args::ContentFormat::Text => {
            // Wrap plain text in <pre> for browser rendering
            let escaped = html_escape(&args.content);
            let html_body = if let Some(ref title) = args.title {
                format!("<h1>{}</h1>\n<pre>{}</pre>", html_escape(title), escaped)
            } else {
                format!("<pre>{}</pre>", escaped)
            };
            styles::wrap_html_with_styles(&html_body, args.title.as_deref())
        }
    };

    // Step 2: Write HTML to temp file
    let temp_dir = std::env::temp_dir();
    let temp_html = temp_dir.join(format!("aleph_pdf_{}.html", std::process::id()));
    std::fs::write(&temp_html, &html).map_err(|e| {
        ToolError::Execution(format!("Failed to write temp HTML: {}", e))
    })?;

    let file_url = format!("file://{}", temp_html.display());
    debug!(url = %file_url, "Rendering PDF via browser engine");

    // Step 3: Launch headless Chrome and render
    let result = render_pdf_with_chrome(&file_url, args, output_path).await;

    // Step 4: Cleanup temp file (best-effort)
    let _ = std::fs::remove_file(&temp_html);

    result
}

/// Launch Chrome, navigate to URL, and print to PDF.
async fn render_pdf_with_chrome(
    url: &str,
    args: &PdfGenerateArgs,
    output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    let chrome_path = find_chromium().map_err(|_| {
        ToolError::Execution("Chrome/Chromium not found on this system".to_string())
    })?;

    let config = CdpBrowserConfig::builder()
        .chrome_executable(chrome_path)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .build()
        .map_err(|e| ToolError::Execution(format!("Failed to build browser config: {}", e)))?;

    let (browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to launch Chrome: {}", e)))?;

    // Spawn CDP event handler
    let handle = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if let Err(e) = event {
                tracing::trace!("CDP event error during PDF render: {e}");
            }
        }
    });

    // Navigate and render
    let page = browser
        .new_page(url)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to open page: {}", e)))?;

    // Wait for page load
    page.wait_for_navigation()
        .await
        .map_err(|e| ToolError::Execution(format!("Page navigation failed: {}", e)))?;

    // Configure print parameters
    let (paper_width, paper_height) = args.page_size.dimensions_inches();
    let margin_inches = args.margin_mm as f64 / 25.4;

    let print_params = PrintToPdfParams::builder()
        .landscape(false)
        .print_background(true)
        .paper_width(paper_width)
        .paper_height(paper_height)
        .margin_top(margin_inches)
        .margin_bottom(margin_inches)
        .margin_left(margin_inches)
        .margin_right(margin_inches)
        .build();

    // Generate PDF
    let pdf_bytes = page
        .pdf(print_params)
        .await
        .map_err(|e| ToolError::Execution(format!("PDF generation failed: {}", e)))?;

    // Write PDF to output
    std::fs::write(output_path, &pdf_bytes).map_err(|e| {
        ToolError::Execution(format!("Failed to write PDF file: {}", e))
    })?;

    // Estimate page count (rough: PDF header contains /Count)
    let page_count = estimate_page_count(&pdf_bytes);

    // Cleanup browser
    drop(page);
    drop(browser);
    handle.abort();

    info!(
        output = %output_path.display(),
        pages = page_count,
        engine = "browser",
        "PDF generated successfully via Chrome"
    );

    Ok(PdfGenerateOutput {
        success: true,
        output_path: output_path.to_string_lossy().to_string(),
        pages: page_count,
        message: format!(
            "Successfully generated {} page PDF (browser engine): {}",
            page_count,
            output_path.display()
        ),
    })
}

/// Rough page count estimation from PDF bytes.
fn estimate_page_count(pdf_bytes: &[u8]) -> usize {
    // Look for /Count N in the PDF trailer
    let content = String::from_utf8_lossy(pdf_bytes);
    if let Some(pos) = content.rfind("/Count ") {
        let after = &content[pos + 7..];
        if let Some(end) = after.find(|c: char| !c.is_ascii_digit()) {
            if let Ok(count) = after[..end].parse::<usize>() {
                return count;
            }
        }
    }
    1 // Default assumption
}

/// Basic HTML escaping for plain text content.
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
```

**Step 4: Add engine dispatch to `mod.rs`**

Update the `generate()` method in `mod.rs`:

```rust
mod browser_engine;

// In PdfGenerateTool::generate():
async fn generate(&self, args: PdfGenerateArgs) -> std::result::Result<PdfGenerateOutput, ToolError> {
    let output_path = self.resolve_output_path(&args.output_path)?;

    // Create parent directories
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ToolError::Execution(format!("Failed to create output directory: {}", e))
        })?;
    }

    match args.render_engine {
        RenderEngine::Browser => {
            // Explicit browser request — fail if unavailable
            browser_engine::generate(&args, &output_path).await
        }
        RenderEngine::Native => {
            // Explicit native request
            native_engine::generate(&args, &output_path)
        }
        RenderEngine::Auto => {
            // Try browser first, fallback to native
            if browser_engine::is_chrome_available() {
                match browser_engine::generate(&args, &output_path).await {
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
    }
}
```

**Step 5: Run all tests**

```bash
cargo test -p alephcore --lib pdf_generate
```

Expected: All tests pass (unit tests for HTML conversion don't need Chrome; integration tests use native engine if Chrome unavailable).

**Step 6: Commit**

```bash
git add core/src/builtin_tools/pdf_generate/browser_engine.rs
git commit -m "pdf: add browser engine with Markdown→HTML+CSS→Chrome→PDF pipeline"
```

---

### Task 4: Add browser engine integration test

**Files:**
- Modify: `core/src/builtin_tools/pdf_generate/tests.rs`

**Step 1: Write the integration test**

Add to `tests.rs`:

```rust
/// Integration test: browser engine generates valid PDF from Markdown.
/// Skipped if Chrome is not available.
#[tokio::test]
async fn test_browser_engine_markdown_pdf() {
    if !super::browser_engine::is_chrome_available() {
        eprintln!("Skipping browser engine test — Chrome not available");
        return;
    }

    let tool = PdfGenerateTool::new();
    let temp_dir = std::env::temp_dir();
    let output_path = temp_dir.join("test_browser_engine.pdf");

    let markdown_content = r#"# Browser Engine Test

This PDF was rendered by the **browser engine**.

## Features

- Proper heading sizes
- **Bold** and *italic* text
- Lists with correct indentation

## Table

| Feature | Status |
|---------|--------|
| Headings | ✅ |
| Lists | ✅ |
| Tables | ✅ |
| Code blocks | ✅ |

## Code

```rust
fn main() {
    println!("Hello from browser engine!");
}
```

> This is a blockquote. It should have a left border and muted color.

中文内容测试：这是一段中文文本，用于验证 CJK 字体渲染。
"#;

    let args = PdfGenerateArgs {
        content: markdown_content.to_string(),
        output_path: output_path.to_string_lossy().to_string(),
        format: ContentFormat::Markdown,
        page_size: PageSize::A4,
        title: Some("Browser Engine Test".to_string()),
        font_size: 12.0,
        line_spacing: 1.5,
        margin_mm: 20.0,
        render_engine: RenderEngine::Browser,
    };

    let result = tool.call(args).await;
    assert!(result.is_ok(), "Browser engine should produce PDF: {:?}", result.err());

    let output = result.unwrap();
    assert!(output.success);
    assert!(output.pages >= 1);

    // Verify PDF file exists and has substantial content
    let metadata = std::fs::metadata(&output_path).unwrap();
    assert!(metadata.len() > 5000, "Browser-rendered PDF should be substantial");

    // Cleanup
    let _ = std::fs::remove_file(&output_path);
}

/// Test: Auto engine falls back to native when Chrome is unavailable.
#[tokio::test]
async fn test_auto_engine_fallback() {
    let tool = PdfGenerateTool::new();
    let temp_dir = std::env::temp_dir();
    let output_path = temp_dir.join("test_auto_fallback.pdf");

    let args = PdfGenerateArgs {
        content: "# Fallback Test\n\nThis should work regardless of Chrome.\n".to_string(),
        output_path: output_path.to_string_lossy().to_string(),
        format: ContentFormat::Markdown,
        page_size: PageSize::A4,
        title: None,
        font_size: 12.0,
        line_spacing: 1.5,
        margin_mm: 20.0,
        render_engine: RenderEngine::Auto,
    };

    let result = tool.call(args).await;
    assert!(result.is_ok(), "Auto engine should always succeed: {:?}", result.err());

    let output = result.unwrap();
    assert!(output.success);

    let _ = std::fs::remove_file(&output_path);
}
```

**Step 2: Run tests**

```bash
cargo test -p alephcore --lib pdf_generate::tests::test_browser_engine_markdown_pdf
cargo test -p alephcore --lib pdf_generate::tests::test_auto_engine_fallback
```

Expected: Both pass (browser test skips gracefully if no Chrome; auto test always succeeds via fallback).

**Step 3: Commit**

```bash
git add core/src/builtin_tools/pdf_generate/tests.rs
git commit -m "pdf: add browser engine integration tests with auto-fallback"
```

---

### Task 5: Update exports and verify end-to-end

**Files:**
- Verify: `core/src/builtin_tools/mod.rs` line 80 — `pub use pdf_generate::{PdfGenerateArgs, PdfGenerateTool};`
- Verify: `core/src/tools/builtin.rs` — `with_pdf_generate()` still works

**Step 1: Verify the public API is unchanged**

Check that existing code using `PdfGenerateArgs` and `PdfGenerateTool` still compiles:

```bash
cargo check -p alephcore
```

Expected: No errors. The re-export path `crate::builtin_tools::pdf_generate::{PdfGenerateArgs, PdfGenerateTool}` is stable because `mod.rs` re-exports from `args.rs`.

**Step 2: Verify `RenderEngine` is exposed in JSON Schema**

Add a test to `tests.rs`:

```rust
#[test]
fn test_args_schema_includes_render_engine() {
    let schema = schemars::schema_for!(PdfGenerateArgs);
    let json = serde_json::to_string_pretty(&schema).unwrap();
    assert!(json.contains("render_engine"), "Schema should include render_engine field");
    assert!(json.contains("auto"), "Schema should include auto option");
    assert!(json.contains("browser"), "Schema should include browser option");
    assert!(json.contains("native"), "Schema should include native option");
}
```

**Step 3: Run full test suite**

```bash
cargo test -p alephcore --lib pdf_generate
```

Expected: All tests pass.

**Step 4: Final commit**

```bash
git add -u
git commit -m "pdf: verify dual-engine integration and JSON Schema exposure"
```

---

### Summary of Files

| Action | File |
|--------|------|
| Delete | `core/src/builtin_tools/pdf_generate.rs` |
| Create | `core/src/builtin_tools/pdf_generate/mod.rs` |
| Create | `core/src/builtin_tools/pdf_generate/args.rs` |
| Create | `core/src/builtin_tools/pdf_generate/native_engine.rs` |
| Create | `core/src/builtin_tools/pdf_generate/browser_engine.rs` |
| Create | `core/src/builtin_tools/pdf_generate/styles.rs` |
| Create | `core/src/builtin_tools/pdf_generate/tests.rs` |
| Unchanged | `core/src/builtin_tools/mod.rs` (pub mod + pub use already correct) |
| Unchanged | `core/src/tools/builtin.rs` (with_pdf_generate() unchanged) |
| Unchanged | `core/Cargo.toml` (all deps already present) |
