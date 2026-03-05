//! Browser-based PDF rendering engine using headless Chrome/Chromium.
//!
//! Converts Markdown to HTML, applies GitHub-flavored CSS styling, and uses
//! Chrome's built-in print-to-PDF via the Chrome DevTools Protocol (CDP).
//! Produces high-fidelity PDF output with full CSS support.

use std::path::Path;

use chromiumoxide::browser::{Browser, BrowserConfig as CdpBrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use futures::StreamExt;
use pulldown_cmark::{html, Options, Parser};
use tracing::{debug, info, warn};

use super::args::{ContentFormat, PdfGenerateArgs, PdfGenerateOutput};
use super::styles;
use crate::browser::find_chromium;
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
    find_chromium().is_ok()
}

/// Generate a PDF using headless Chrome via CDP.
///
/// Flow:
/// 1. Build HTML document (Markdown or plain text wrapped in `<pre>`)
/// 2. Write HTML to a temporary file
/// 3. Launch headless Chrome and render the page to PDF
/// 4. Write PDF bytes to the output path
/// 5. Clean up the temporary file
pub async fn generate(
    args: &PdfGenerateArgs,
    output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    // Step 1: Build HTML document
    let html_doc = match args.format {
        ContentFormat::Markdown => {
            build_html_document(&args.content, args.title.as_deref())
        }
        ContentFormat::Text => {
            let escaped = html_escape(&args.content);
            // Use <div> with whitespace preservation instead of <pre>
            // to avoid the code-block gray background on plain text
            let html_body = format!(
                "<div style=\"white-space: pre-wrap; word-wrap: break-word;\">{escaped}</div>"
            );
            styles::wrap_html_with_styles(&html_body, args.title.as_deref())
        }
    };

    // Step 2: Write HTML to temp file
    let temp_path = std::env::temp_dir().join(format!("aleph_pdf_{}.html", std::process::id()));
    std::fs::write(&temp_path, &html_doc).map_err(|e| {
        ToolError::Execution(format!("Failed to write temp HTML file: {}", e))
    })?;

    let file_url = format!("file://{}", temp_path.display());
    debug!(url = %file_url, "Rendering PDF from temp HTML");

    // Step 3: Render PDF with Chrome
    let result = render_pdf_with_chrome(&file_url, args, output_path).await;

    // Step 4: Cleanup temp file (best-effort)
    if let Err(e) = std::fs::remove_file(&temp_path) {
        warn!(error = %e, path = %temp_path.display(), "Failed to clean up temp HTML file");
    }

    result
}

/// Launch headless Chrome, navigate to the HTML file, and print to PDF.
async fn render_pdf_with_chrome(
    url: &str,
    args: &PdfGenerateArgs,
    output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    // Find Chrome binary
    let chrome_path = find_chromium().map_err(|e| {
        ToolError::Execution(format!("Chrome not found: {}", e))
    })?;
    debug!(chrome = %chrome_path.display(), "Using Chrome binary");

    // Build CDP browser config
    let config = CdpBrowserConfig::builder()
        .chrome_executable(chrome_path)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .build()
        .map_err(|e| ToolError::Execution(format!("Failed to build browser config: {}", e)))?;

    // Launch browser
    let (browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to launch Chrome: {}", e)))?;

    // Spawn CDP event handler
    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if let Err(e) = event {
                tracing::trace!("CDP handler event: {e}");
            }
        }
    });

    // Navigate to the HTML file
    let page = browser.new_page(url).await.map_err(|e| {
        ToolError::Execution(format!("Failed to open page: {}", e))
    })?;

    // Wait for page to load
    page.wait_for_navigation().await.map_err(|e| {
        ToolError::Execution(format!("Failed waiting for page navigation: {}", e))
    })?;

    // Build PDF print parameters
    let (paper_w, paper_h) = args.page_size.dimensions_inches();
    let margin_inches = f64::from(args.margin_mm) / 25.4;

    let print_params = PrintToPdfParams::builder()
        .paper_width(paper_w)
        .paper_height(paper_h)
        .margin_top(margin_inches)
        .margin_bottom(margin_inches)
        .margin_left(margin_inches)
        .margin_right(margin_inches)
        .print_background(true)
        .build();

    // Generate PDF
    let pdf_bytes = page.pdf(print_params).await.map_err(|e| {
        ToolError::Execution(format!("Failed to generate PDF: {}", e))
    })?;

    // Create parent directories if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ToolError::Execution(format!("Failed to create output directory: {}", e))
        })?;
    }

    // Write PDF to output path
    std::fs::write(output_path, &pdf_bytes).map_err(|e| {
        ToolError::Execution(format!("Failed to write PDF file: {}", e))
    })?;

    // Estimate page count from PDF bytes
    let page_count = estimate_page_count(&pdf_bytes);

    info!(
        output = %output_path.display(),
        pages = page_count,
        bytes = pdf_bytes.len(),
        "PDF generated via browser engine"
    );

    // Clean up: drop browser, abort handler
    drop(browser);
    handler_task.abort();

    Ok(PdfGenerateOutput {
        success: true,
        output_path: output_path.to_string_lossy().to_string(),
        pages: page_count,
        message: format!(
            "Successfully generated {} page PDF via browser engine: {}",
            page_count,
            output_path.display()
        ),
    })
}

/// Escape HTML special characters in plain text content.
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Estimate the number of pages in a PDF by searching for `/Count N` patterns.
///
/// This is a heuristic that looks for the PDF page tree `/Count` entry.
/// Falls back to 1 if no pattern is found.
fn estimate_page_count(pdf_bytes: &[u8]) -> usize {
    // Look for /Count <number> pattern in the PDF cross-reference / page tree
    let haystack = String::from_utf8_lossy(pdf_bytes);
    let mut max_count = 0usize;

    for cap_start in haystack.match_indices("/Count ") {
        let after = &haystack[cap_start.0 + 7..];
        let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num_str.parse::<usize>() {
            if n > max_count {
                max_count = n;
            }
        }
    }

    if max_count > 0 {
        max_count
    } else {
        1
    }
}
