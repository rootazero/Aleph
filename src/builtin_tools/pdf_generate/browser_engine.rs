//! Browser-based PDF rendering via playwright-cli.
//!
//! Converts Markdown to HTML, writes to a temp file, then uses
//! `playwright-cli -s=aleph-pdf-gen goto file://<tmp>` followed by
//! `playwright-cli -s=aleph-pdf-gen pdf --filename=<output>`.

use std::path::Path;
use std::time::Duration;

use pulldown_cmark::{html, Options, Parser};
use tempfile::NamedTempFile;
use tracing::{debug, info};

use super::args::{ContentFormat, PdfGenerateArgs, PdfGenerateOutput};
use super::styles;
use crate::browser::playwright_cli::PlaywrightCliDriver;
use crate::browser::profile::PlaywrightCliConfig;
use crate::builtin_tools::error::ToolError;

fn file_url_from_path(path: &Path) -> String {
    // RFC 8089: file URLs use forward slashes and require `file:///` on
    // Windows where paths start with a drive letter (`C:\…` → `file:///C:/…`).
    // `Path::display` on Windows preserves backslashes, which leaves the URL
    // unparseable for playwright-cli.
    let mut s = path.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    format!("file://{s}")
}
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

/// Cheap synchronous check: is the playwright-cli toolchain likely available
/// on this system? Callers use this to short-circuit before invoking the
/// async `generate()` path.
pub fn is_browser_engine_available() -> bool {
    which::which("playwright-cli").is_ok()
}

/// Generate a PDF using playwright-cli.
///
/// Flow:
/// 1. Build HTML document (Markdown or plain text wrapped in `<pre>`)
/// 2. Write HTML to a temp file
/// 3. `playwright-cli -s=aleph-pdf-gen goto file://<tmp>`
/// 4. `playwright-cli -s=aleph-pdf-gen pdf --filename=<output>`
/// 5. Temp file auto-cleaned when `NamedTempFile` drops
pub async fn generate(
    args: &PdfGenerateArgs,
    output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    let html_doc = match args.format {
        ContentFormat::Markdown => build_html_document(&args.content, args.title.as_deref()),
        ContentFormat::Text => {
            let escaped = styles::html_escape(&args.content);
            styles::wrap_html_with_styles(&format!("<pre>{escaped}</pre>"), args.title.as_deref())
        }
    };

    // Ensure the output directory exists before handing the path to
    // playwright-cli, which (unlike the native engine) does not create it and
    // would otherwise fail to write the PDF.
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ToolError::Execution(format!("create output dir: {e}")))?;
    }

    let tmp = NamedTempFile::with_suffix(".html")
        .map_err(|e| ToolError::Execution(format!("tempfile: {e}")))?;
    tokio::fs::write(tmp.path(), html_doc.as_bytes())
        .await
        .map_err(|e| ToolError::Execution(format!("write html: {e}")))?;
    let file_url = file_url_from_path(tmp.path());
    debug!("pdf_generate wrote HTML to {}", tmp.path().display());

    let driver = PlaywrightCliDriver::new(PlaywrightCliConfig::default());
    let session = "aleph-pdf-gen";
    driver
        .run(session, &["goto", &file_url], Duration::from_secs(30))
        .await
        .map_err(|e| ToolError::Execution(format!("goto: {e}")))?;

    let out_str = output_path.to_string_lossy().to_string();
    driver
        .run(
            session,
            &["pdf", "--filename", &out_str],
            Duration::from_secs(60),
        )
        .await
        .map_err(|e| ToolError::Execution(format!("pdf: {e}")))?;

    info!(path = %output_path.display(), "PDF generated via playwright-cli");

    Ok(PdfGenerateOutput {
        success: true,
        output_path: output_path.to_string_lossy().to_string(),
        pages: 0, // playwright-cli does not report page count
        message: format!(
            "PDF generated via playwright-cli: {}",
            output_path.display()
        ),
    })
}
