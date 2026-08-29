//! Native PDF rendering engine using printpdf
//!
//! Provides fast, dependency-free PDF generation for plain text and Markdown.
//! Supports CJK text rendering with system font discovery.

use std::path::{Path, PathBuf};

use printpdf::{
    BuiltinFont, Mm, Op, ParsedFont, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, Point,
    Pt, TextItem,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use tracing::{debug, info, warn};

use super::args::{ContentFormat, PdfGenerateArgs, PdfGenerateOutput};
use crate::builtin_tools::error::ToolError;

/// Best-effort fsync of a file. Returns the error if the file is not open
/// or the platform lacks `sync_all`, but a failure here does NOT undo the
/// write — the temp → rename that follows is the atomicity boundary; the
/// fsync just makes the durability window shorter.
fn sync_all_best_effort(path: &Path) -> std::io::Result<()> {
    let f = std::fs::File::open(path)?;
    f.sync_all()
}

/// Sync analog of `crate::utils::atomic_write::atomic_write_bytes`. Used by
/// the PDF native engine because `generate` is itself sync (it runs under
/// `spawn_blocking` upstream in mod.rs), and the await cannot land here.
fn atomic_write_bytes_sync(path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    let tmp_path = path.with_extension("pdf.tmp");
    if let Err(e) = std::fs::write(&tmp_path, bytes) {
        return Err(ToolError::Execution(format!(
            "Failed to write PDF temp file {}: {e}",
            tmp_path.display()
        )));
    }
    if let Err(e) = sync_all_best_effort(&tmp_path) {
        tracing::warn!(error = %e, path = %tmp_path.display(), "PDF: fsync failed (continuing)");
    }
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        // Best-effort cleanup of the orphan temp file before propagating.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(ToolError::Execution(format!(
            "Failed to write PDF file {}: {e}",
            path.display()
        )));
    }
    Ok(())
}

/// Generate a PDF using the native printpdf engine
///
/// Takes a reference to args and a pre-resolved output path.
/// Path resolution is handled by the caller (mod.rs).
pub fn generate(
    args: &PdfGenerateArgs,
    output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    let (page_width_mm, page_height_mm) = args.page_size.dimensions_mm();

    // Create document (printpdf 0.9 builds pages as `Vec<Op>` and serializes at save).
    let mut doc = PdfDocument::new(args.title.as_deref().unwrap_or("Document"));

    // Resolve the active font handle: a parsed system font when available,
    // otherwise the built-in Helvetica.
    let active_font = load_font(&mut doc);

    // Calculate text area
    let text_width = page_width_mm - (args.margin_mm * 2.0);
    let line_height = args.font_size * args.line_spacing;
    let mut y_position = page_height_mm - args.margin_mm - args.font_size;

    let mut builder = DocBuilder::new(Mm(page_width_mm), Mm(page_height_mm));

    // Render title if provided
    if let Some(ref title) = args.title {
        let title_size = args.font_size * 1.5;
        builder.draw_line(title, title_size, args.margin_mm, y_position, &active_font);
        y_position -= line_height * 2.0;
    }

    // Parse and render content based on format
    match args.format {
        ContentFormat::Text => {
            // Simple text rendering
            for line in args.content.lines() {
                check_page_break(
                    &mut y_position,
                    &mut builder,
                    line_height,
                    args.margin_mm,
                    args.font_size,
                    page_height_mm,
                );

                // Word wrap
                let wrapped_lines = wrap_text(line, text_width, args.font_size);
                for wrapped_line in wrapped_lines {
                    check_page_break(
                        &mut y_position,
                        &mut builder,
                        line_height,
                        args.margin_mm,
                        args.font_size,
                        page_height_mm,
                    );

                    builder.draw_line(
                        &wrapped_line,
                        args.font_size,
                        args.margin_mm,
                        y_position,
                        &active_font,
                    );
                    y_position -= line_height;
                }
            }
        }
        ContentFormat::Markdown => {
            // Markdown rendering
            let options = Options::all();
            let parser = Parser::new_ext(&args.content, options);

            let mut current_text = String::new();
            let mut in_code_block = false;
            let mut list_depth = 0;
            let mut current_font_size = args.font_size;

            for event in parser {
                match event {
                    Event::Start(tag) => match tag {
                        Tag::Heading { level, .. } => {
                            // Flush current text
                            if !current_text.is_empty() {
                                render_text(
                                    &current_text,
                                    current_font_size,
                                    args.margin_mm,
                                    text_width,
                                    line_height,
                                    &mut y_position,
                                    &mut builder,
                                    page_height_mm,
                                    &active_font,
                                );
                                current_text.clear();
                            }

                            current_font_size = match level {
                                HeadingLevel::H1 => args.font_size * 2.0,
                                HeadingLevel::H2 => args.font_size * 1.7,
                                HeadingLevel::H3 => args.font_size * 1.4,
                                HeadingLevel::H4 => args.font_size * 1.2,
                                _ => args.font_size * 1.1,
                            };
                            y_position -= line_height * 0.5; // Extra space before heading
                        }
                        Tag::Paragraph => {}
                        Tag::CodeBlock(_) => {
                            in_code_block = true;
                            current_font_size = args.font_size * 0.9;
                        }
                        Tag::List(_) => {
                            list_depth += 1;
                        }
                        Tag::Item => {
                            let indent = "  ".repeat(list_depth);
                            current_text.push_str(&indent);
                            current_text.push_str("• ");
                        }
                        Tag::Emphasis | Tag::Strong => {}
                        _ => {}
                    },
                    Event::End(tag_end) => match tag_end {
                        TagEnd::Heading(_) => {
                            render_text(
                                &current_text,
                                current_font_size,
                                args.margin_mm,
                                text_width,
                                line_height,
                                &mut y_position,
                                &mut builder,
                                page_height_mm,
                                &active_font,
                            );
                            current_text.clear();
                            current_font_size = args.font_size;
                            y_position -= line_height * 0.5; // Extra space after heading
                        }
                        TagEnd::Paragraph => {
                            render_text(
                                &current_text,
                                current_font_size,
                                args.margin_mm,
                                text_width,
                                line_height,
                                &mut y_position,
                                &mut builder,
                                page_height_mm,
                                &active_font,
                            );
                            current_text.clear();
                            y_position -= line_height * 0.5; // Paragraph spacing
                        }
                        TagEnd::CodeBlock => {
                            render_text(
                                &current_text,
                                current_font_size,
                                args.margin_mm + 10.0, // Indent code
                                text_width - 10.0,
                                line_height,
                                &mut y_position,
                                &mut builder,
                                page_height_mm,
                                &active_font,
                            );
                            current_text.clear();
                            in_code_block = false;
                            current_font_size = args.font_size;
                            y_position -= line_height * 0.5;
                        }
                        TagEnd::List(_) => {
                            list_depth = list_depth.saturating_sub(1);
                            if list_depth == 0 {
                                y_position -= line_height * 0.5;
                            }
                        }
                        TagEnd::Item => {
                            render_text(
                                &current_text,
                                current_font_size,
                                args.margin_mm,
                                text_width,
                                line_height,
                                &mut y_position,
                                &mut builder,
                                page_height_mm,
                                &active_font,
                            );
                            current_text.clear();
                        }
                        _ => {}
                    },
                    Event::Text(text) => {
                        current_text.push_str(&text);
                    }
                    Event::Code(code) => {
                        current_text.push('`');
                        current_text.push_str(&code);
                        current_text.push('`');
                    }
                    Event::SoftBreak => {
                        if in_code_block {
                            current_text.push('\n');
                        } else {
                            current_text.push(' ');
                        }
                    }
                    Event::HardBreak => {
                        current_text.push('\n');
                    }
                    _ => {}
                }
            }

            // Render any remaining text
            if !current_text.is_empty() {
                render_text(
                    &current_text,
                    current_font_size,
                    args.margin_mm,
                    text_width,
                    line_height,
                    &mut y_position,
                    &mut builder,
                    page_height_mm,
                    &active_font,
                );
            }
        }
    }

    let pages = builder.finish();
    let page_count = pages.len();

    // Serialize the document to PDF bytes.
    let pdf_bytes = doc
        .with_pages(pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new());

    // Create parent directories if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ToolError::Execution(format!("Failed to create output directory: {e}")))?;
    }

    // Write the PDF bytes atomically: `std::fs::write` is non-atomic — a crash
    // mid-write leaves a truncated / zero-byte file at output_path and the
    // caller cannot tell 'partial write' from 'no data'. The temp + rename +
    // fsync path below mirrors the canvas / bundled paths' atomicity
    // invariant. `generate` runs on the blocking pool (see mod.rs
    // `spawn_blocking`), so a sync write is the right shape here.
    atomic_write_bytes_sync(output_path, &pdf_bytes)?;

    info!(
        output = %output_path.display(),
        pages = page_count,
        "PDF generated successfully"
    );

    Ok(PdfGenerateOutput {
        success: true,
        output_path: output_path.to_string_lossy().to_string(),
        pages: page_count,
        message: format!(
            "Successfully generated {} page PDF: {}",
            page_count,
            output_path.display()
        ),
    })
}

/// Accumulates page content as `Vec<Op>` for printpdf 0.9's imperative model.
///
/// Each drawn line is emitted as a self-contained text object
/// (`StartTextSection` … `EndTextSection`) so that `SetTextCursor` (which maps
/// to the relative PDF `Td` operator) resolves against a freshly reset text
/// matrix — giving absolute positioning per line, matching the old
/// `layer.use_text` behaviour.
struct DocBuilder {
    /// Completed pages.
    pages: Vec<PdfPage>,
    /// Operations accumulated for the page currently being built.
    ops: Vec<Op>,
    page_width: Mm,
    page_height: Mm,
}

impl DocBuilder {
    const fn new(page_width: Mm, page_height: Mm) -> Self {
        Self {
            pages: Vec::new(),
            ops: Vec::new(),
            page_width,
            page_height,
        }
    }

    /// Draw a single line of text with its baseline at `(x_mm, y_mm)` from the
    /// bottom-left corner of the page.
    fn draw_line(
        &mut self,
        text: &str,
        font_size: f32,
        x_mm: f32,
        y_mm: f32,
        font: &PdfFontHandle,
    ) {
        self.ops.push(Op::StartTextSection);
        self.ops.push(Op::SetFont {
            font: font.clone(),
            size: Pt(font_size),
        });
        self.ops.push(Op::SetTextCursor {
            pos: Point::new(Mm(x_mm), Mm(y_mm)),
        });
        self.ops.push(Op::ShowText {
            items: vec![TextItem::Text(text.to_string())],
        });
        self.ops.push(Op::EndTextSection);
    }

    /// Finalize the current page and start a fresh one.
    fn new_page(&mut self) {
        let ops = std::mem::take(&mut self.ops);
        self.pages
            .push(PdfPage::new(self.page_width, self.page_height, ops));
    }

    /// Finalize the document, flushing the in-progress page, and return all pages.
    fn finish(mut self) -> Vec<PdfPage> {
        let ops = std::mem::take(&mut self.ops);
        self.pages
            .push(PdfPage::new(self.page_width, self.page_height, ops));
        self.pages
    }
}

/// Resolve the active font: a parsed system font registered with the document,
/// falling back to the built-in Helvetica when none is available or parseable.
fn load_font(doc: &mut PdfDocument) -> PdfFontHandle {
    if let Some(font_path) = find_system_font() {
        match std::fs::read(&font_path) {
            Ok(bytes) => {
                let mut warnings = Vec::new();
                if let Some(parsed) = ParsedFont::from_bytes(&bytes, 0, &mut warnings) {
                    debug!("Using system font: {:?}", font_path);
                    return PdfFontHandle::External(doc.add_font(&parsed));
                }
                warn!("Failed to parse font {:?}, using built-in font", font_path);
            }
            Err(e) => warn!(
                "Failed to read font {:?}: {}, using built-in font",
                font_path, e
            ),
        }
    } else {
        warn!("No system font found, using built-in font");
    }
    PdfFontHandle::Builtin(BuiltinFont::Helvetica)
}

/// Start a new page and reset the vertical cursor when the next line would
/// overflow the bottom margin.
fn check_page_break(
    y: &mut f32,
    builder: &mut DocBuilder,
    line_height: f32,
    margin: f32,
    font_size: f32,
    page_height_mm: f32,
) {
    if *y < margin + line_height {
        builder.new_page();
        *y = page_height_mm - margin - font_size;
    }
}

/// Check if a character is CJK (Chinese, Japanese, Korean) or full-width
#[must_use]
pub const fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{2E80}'..='\u{2EFF}' // CJK Radicals Supplement
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Fullwidth Forms
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{1100}'..='\u{11FF}' // Hangul Jamo
    )
}

/// Find a suitable font for text rendering
#[must_use]
pub fn find_system_font() -> Option<PathBuf> {
    // Try common font locations — CJK-capable fonts FIRST
    // (PingFang/Hiragino/STHeiti support both Latin AND CJK characters)
    let font_paths = if cfg!(target_os = "macos") {
        vec![
            "/System/Library/Fonts/PingFang.ttc", // macOS 10.11+ (may be absent on some versions)
            "/System/Library/Fonts/Hiragino Sans GB.ttc", // CJK sans-serif, widely available
            "/System/Library/Fonts/STHeiti Medium.ttc", // STHeiti CJK
            "/System/Library/Fonts/Supplemental/Songti.ttc", // CJK serif fallback
            "/System/Library/Fonts/Helvetica.ttc", // Latin-only last resort
        ]
    } else if cfg!(target_os = "windows") {
        vec![
            "C:\\Windows\\Fonts\\msyh.ttc",   // Microsoft YaHei — CJK + Latin
            "C:\\Windows\\Fonts\\simsun.ttc", // SimSun — CJK fallback
            "C:\\Windows\\Fonts\\arial.ttf",
        ]
    } else {
        // Linux
        vec![
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", // CJK + Latin
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ]
    };

    for path in font_paths {
        let path_buf = PathBuf::from(path);
        if path_buf.exists() {
            return Some(path_buf);
        }
    }
    None
}

/// Text wrapping that handles both Latin (word-boundary) and CJK (character-boundary)
#[must_use]
pub fn wrap_text(text: &str, max_width_mm: f32, font_size: f32) -> Vec<String> {
    // Approximate column units per mm (CJK char = 2 units, Latin char = 1 unit)
    let units_per_mm = 0.4 / (font_size / 12.0);
    let max_units = (max_width_mm * units_per_mm) as usize;

    if max_units == 0 {
        return vec![text.to_string()];
    }

    // Quick check: calculate display width
    let display_width: usize = text.chars().map(|c| if is_cjk(c) { 2 } else { 1 }).sum();
    if display_width <= max_units {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width: usize = 0;

    for c in text.chars() {
        let char_width = if is_cjk(c) { 2 } else { 1 };

        // If adding this char would overflow, start a new line
        if current_width + char_width > max_units && !current_line.is_empty() {
            lines.push(current_line);
            current_line = String::new();
            current_width = 0;
        }

        current_line.push(c);
        current_width += char_width;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Render text with word wrapping and page breaks.
#[allow(clippy::too_many_arguments)]
fn render_text(
    text: &str,
    font_size: f32,
    margin: f32,
    text_width: f32,
    line_height: f32,
    y_position: &mut f32,
    builder: &mut DocBuilder,
    page_height_mm: f32,
    font: &PdfFontHandle,
) {
    for line in text.lines() {
        let wrapped_lines = wrap_text(line, text_width, font_size);
        for wrapped_line in wrapped_lines {
            // Check for page break
            if *y_position < margin + line_height {
                builder.new_page();
                *y_position = page_height_mm - margin - font_size;
            }

            builder.draw_line(&wrapped_line, font_size, margin, *y_position, font);
            *y_position -= line_height;
        }
    }
}
