//! Native PDF rendering engine using printpdf
//!
//! Provides fast, dependency-free PDF generation for plain text and Markdown.
//! Supports CJK text rendering with system font discovery.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use printpdf::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use tracing::{debug, info, warn};

use super::args::{ContentFormat, PdfGenerateArgs, PdfGenerateOutput};
use crate::builtin_tools::error::ToolError;

/// Generate a PDF using the native printpdf engine
///
/// Takes a reference to args and a pre-resolved output path.
/// Path resolution is handled by the caller (mod.rs).
pub fn generate(args: &PdfGenerateArgs, output_path: &Path) -> Result<PdfGenerateOutput, ToolError> {
    let (page_width_mm, page_height_mm) = args.page_size.dimensions_mm();
    let _margin = Mm(args.margin_mm);

    // Create document
    let (doc, page1, layer1) = PdfDocument::new(
        args.title.as_deref().unwrap_or("Document"),
        Mm(page_width_mm),
        Mm(page_height_mm),
        "Layer 1",
    );

    let mut current_layer = doc.get_page(page1).get_layer(layer1);
    let mut current_page = page1;
    let mut page_count = 1;

    // Try to load a font
    let font = if let Some(font_path) = find_system_font() {
        debug!("Using system font: {:?}", font_path);
        match doc.add_external_font(File::open(&font_path).map_err(|e| {
            ToolError::Execution(format!("Failed to open font file: {}", e))
        })?) {
            Ok(f) => Some(f),
            Err(e) => {
                warn!("Failed to load font: {}, using built-in font", e);
                None
            }
        }
    } else {
        warn!("No system font found, using built-in font");
        None
    };

    // Use built-in font if external font failed
    let builtin_font = doc.add_builtin_font(BuiltinFont::Helvetica).map_err(|e| {
        ToolError::Execution(format!("Failed to add built-in font: {}", e))
    })?;

    let active_font = font.as_ref().unwrap_or(&builtin_font);

    // Calculate text area
    let text_width = page_width_mm - (args.margin_mm * 2.0);
    let line_height = args.font_size * args.line_spacing;
    let mut y_position = page_height_mm - args.margin_mm - args.font_size;

    // Helper to add new page
    let add_new_page = |doc: &PdfDocumentReference| -> (PdfPageIndex, PdfLayerReference) {
        let (new_page, new_layer) =
            doc.add_page(Mm(page_width_mm), Mm(page_height_mm), "Layer 1");
        (new_page, doc.get_page(new_page).get_layer(new_layer))
    };

    // Helper to check and handle page break
    let check_page_break =
        |y: &mut f32,
         doc: &PdfDocumentReference,
         layer: &mut PdfLayerReference,
         page: &mut PdfPageIndex,
         count: &mut usize| {
            if *y < args.margin_mm + line_height {
                let (new_page, new_layer) = add_new_page(doc);
                *page = new_page;
                *layer = new_layer;
                *count += 1;
                *y = page_height_mm - args.margin_mm - args.font_size;
            }
        };

    // Render title if provided
    if let Some(ref title) = args.title {
        let title_size = args.font_size * 1.5;
        current_layer.use_text(
            title,
            title_size,
            Mm(args.margin_mm),
            Mm(y_position),
            active_font,
        );
        y_position -= line_height * 2.0;
    }

    // Parse and render content based on format
    match args.format {
        ContentFormat::Text => {
            // Simple text rendering
            for line in args.content.lines() {
                check_page_break(
                    &mut y_position,
                    &doc,
                    &mut current_layer,
                    &mut current_page,
                    &mut page_count,
                );

                // Word wrap
                let wrapped_lines = wrap_text(line, text_width, args.font_size);
                for wrapped_line in wrapped_lines {
                    check_page_break(
                        &mut y_position,
                        &doc,
                        &mut current_layer,
                        &mut current_page,
                        &mut page_count,
                    );

                    current_layer.use_text(
                        &wrapped_line,
                        args.font_size,
                        Mm(args.margin_mm),
                        Mm(y_position),
                        active_font,
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
                                    &doc,
                                    &mut current_layer,
                                    &mut current_page,
                                    &mut page_count,
                                    page_width_mm,
                                    page_height_mm,
                                    active_font,
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
                                &doc,
                                &mut current_layer,
                                &mut current_page,
                                &mut page_count,
                                page_width_mm,
                                page_height_mm,
                                active_font,
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
                                &doc,
                                &mut current_layer,
                                &mut current_page,
                                &mut page_count,
                                page_width_mm,
                                page_height_mm,
                                active_font,
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
                                &doc,
                                &mut current_layer,
                                &mut current_page,
                                &mut page_count,
                                page_width_mm,
                                page_height_mm,
                                active_font,
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
                                &doc,
                                &mut current_layer,
                                &mut current_page,
                                &mut page_count,
                                page_width_mm,
                                page_height_mm,
                                active_font,
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
                    &doc,
                    &mut current_layer,
                    &mut current_page,
                    &mut page_count,
                    page_width_mm,
                    page_height_mm,
                    active_font,
                );
            }
        }
    }

    // Create parent directories if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ToolError::Execution(format!("Failed to create output directory: {}", e))
        })?;
    }

    // Save PDF
    let file = File::create(output_path).map_err(|e| {
        ToolError::Execution(format!("Failed to create PDF file: {}", e))
    })?;

    doc.save(&mut BufWriter::new(file)).map_err(|e| {
        ToolError::Execution(format!("Failed to save PDF: {}", e))
    })?;

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

/// Check if a character is CJK (Chinese, Japanese, Korean) or full-width
pub fn is_cjk(c: char) -> bool {
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
pub fn find_system_font() -> Option<PathBuf> {
    // Try common font locations — CJK-capable fonts FIRST
    // (PingFang/Hiragino/STHeiti support both Latin AND CJK characters)
    let font_paths = if cfg!(target_os = "macos") {
        vec![
            "/System/Library/Fonts/PingFang.ttc",            // macOS 10.11+ (may be absent on some versions)
            "/System/Library/Fonts/Hiragino Sans GB.ttc",    // CJK sans-serif, widely available
            "/System/Library/Fonts/STHeiti Medium.ttc",      // STHeiti CJK
            "/System/Library/Fonts/Supplemental/Songti.ttc", // CJK serif fallback
            "/System/Library/Fonts/Helvetica.ttc",           // Latin-only last resort
        ]
    } else if cfg!(target_os = "windows") {
        vec![
            "C:\\Windows\\Fonts\\msyh.ttc", // Microsoft YaHei — CJK + Latin
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
pub fn wrap_text(text: &str, max_width_mm: f32, font_size: f32) -> Vec<String> {
    // Approximate column units per mm (CJK char = 2 units, Latin char = 1 unit)
    let units_per_mm = 0.4 / (font_size / 12.0);
    let max_units = (max_width_mm * units_per_mm) as usize;

    if max_units == 0 {
        return vec![text.to_string()];
    }

    // Quick check: calculate display width
    let display_width: usize = text
        .chars()
        .map(|c| if is_cjk(c) { 2 } else { 1 })
        .sum();
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

/// Render text with word wrapping and page breaks
#[allow(clippy::too_many_arguments)]
pub fn render_text(
    text: &str,
    font_size: f32,
    margin: f32,
    text_width: f32,
    line_height: f32,
    y_position: &mut f32,
    doc: &PdfDocumentReference,
    current_layer: &mut PdfLayerReference,
    current_page: &mut PdfPageIndex,
    page_count: &mut usize,
    page_width_mm: f32,
    page_height_mm: f32,
    font: &IndirectFontRef,
) {
    for line in text.lines() {
        let wrapped_lines = wrap_text(line, text_width, font_size);
        for wrapped_line in wrapped_lines {
            // Check for page break
            if *y_position < margin + line_height {
                let (new_page, new_layer) =
                    doc.add_page(Mm(page_width_mm), Mm(page_height_mm), "Layer 1");
                *current_page = new_page;
                *current_layer = doc.get_page(new_page).get_layer(new_layer);
                *page_count += 1;
                *y_position = page_height_mm - margin - font_size;
            }

            current_layer.use_text(
                &wrapped_line,
                font_size,
                Mm(margin),
                Mm(*y_position),
                font,
            );
            *y_position -= line_height;
        }
    }
}
