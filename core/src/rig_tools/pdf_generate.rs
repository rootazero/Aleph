//! PDF generation tool for AI agent integration
//!
//! Implements AetherTool trait to provide PDF generation capabilities.
//! Supports plain text and Markdown to PDF conversion.
//!
//! # Features
//!
//! - Plain text to PDF
//! - Markdown to PDF (headings, paragraphs, lists, code blocks)
//! - Chinese text support (requires system font)
//! - Configurable page size, margins, and fonts

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use async_trait::async_trait;
use printpdf::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::tools::AetherTool;
use super::error::ToolError;

/// Page size options
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum PageSize {
    /// A4 (210mm x 297mm) - default
    #[default]
    A4,
    /// Letter (8.5in x 11in)
    Letter,
    /// A3 (297mm x 420mm)
    A3,
    /// Custom size in mm
    Custom { width_mm: f32, height_mm: f32 },
}

impl PageSize {
    fn dimensions_mm(&self) -> (f32, f32) {
        match self {
            PageSize::A4 => (210.0, 297.0),
            PageSize::Letter => (215.9, 279.4),
            PageSize::A3 => (297.0, 420.0),
            PageSize::Custom { width_mm, height_mm } => (*width_mm, *height_mm),
        }
    }
}

/// Content format
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContentFormat {
    /// Plain text
    #[default]
    Text,
    /// Markdown
    Markdown,
}

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
}

fn default_font_size() -> f32 {
    12.0
}

fn default_line_spacing() -> f32 {
    1.5
}

fn default_margin() -> f32 {
    20.0
}

/// Output from PDF generation tool
#[derive(Debug, Clone, Serialize)]
pub struct PdfGenerateOutput {
    pub success: bool,
    pub output_path: String,
    pub pages: usize,
    pub message: String,
}

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
- Relative paths (e.g., \"article.pdf\") → saved to ~/.config/aether/output/\n\
- Home paths (e.g., \"~/Desktop/doc.pdf\") → expanded to user's home directory\n\
- Absolute paths (e.g., \"/Users/name/doc.pdf\") → used as-is\n\n\
DEFAULT OUTPUT: Use relative paths like \"article.pdf\" or \"translated.pdf\" for generated PDFs. They will be saved to the default output directory (~/.config/aether/output/), which is always writable.\n\n\
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

    /// Find a suitable font for text rendering
    fn find_system_font() -> Option<PathBuf> {
        // Try common font locations
        let font_paths = if cfg!(target_os = "macos") {
            vec![
                // macOS system fonts - prefer fonts with good Unicode coverage
                "/System/Library/Fonts/Helvetica.ttc",
                "/System/Library/Fonts/Times.ttc",
                "/System/Library/Fonts/PingFang.ttc", // Chinese support
                "/System/Library/Fonts/Supplemental/Arial.ttf",
                "/Library/Fonts/Arial.ttf",
            ]
        } else if cfg!(target_os = "windows") {
            vec![
                "C:\\Windows\\Fonts\\arial.ttf",
                "C:\\Windows\\Fonts\\times.ttf",
                "C:\\Windows\\Fonts\\msyh.ttc", // Chinese support
            ]
        } else {
            // Linux
            vec![
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", // Chinese
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

    /// Generate PDF from content (internal implementation)
    fn generate(&self, args: PdfGenerateArgs) -> std::result::Result<PdfGenerateOutput, ToolError> {
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
        let font = if let Some(font_path) = Self::find_system_font() {
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
                    let wrapped_lines = Self::wrap_text(line, text_width, args.font_size);
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
                                    Self::render_text(
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
                                Self::render_text(
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
                                Self::render_text(
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
                                Self::render_text(
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
                                Self::render_text(
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
                    Self::render_text(
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

        // Determine output path
        // Path resolution rules:
        // 1. Absolute paths (starting with `/`) - used as-is
        // 2. Home paths (starting with `~`) - expanded to home directory
        // 3. Relative paths - resolved to output directory (~/.config/aether/output/)
        let output_path = if args.output_path.starts_with('/') {
            PathBuf::from(&args.output_path)
        } else if args.output_path.starts_with('~') {
            PathBuf::from(
                args.output_path
                    .replace('~', dirs::home_dir().unwrap_or_default().to_str().unwrap_or("")),
            )
        } else if let Some(ref dir) = self.default_output_dir {
            dir.join(&args.output_path)
        } else {
            // Use the default output directory for relative paths
            let output_dir = crate::utils::paths::get_output_dir().map_err(|e| {
                ToolError::Execution(format!("Failed to get output directory: {}", e))
            })?;
            output_dir.join(&args.output_path)
        };

        // Create parent directories if needed
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::Execution(format!("Failed to create output directory: {}", e))
            })?;
        }

        // Save PDF
        let file = File::create(&output_path).map_err(|e| {
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

    /// Simple word wrapping (approximate)
    fn wrap_text(text: &str, max_width_mm: f32, font_size: f32) -> Vec<String> {
        // Approximate characters per line based on font size and width
        // This is a rough estimate - printpdf doesn't provide text metrics easily
        let chars_per_mm = 0.4 / (font_size / 12.0); // Rough approximation
        let max_chars = (max_width_mm * chars_per_mm) as usize;

        if max_chars == 0 || text.len() <= max_chars {
            return vec![text.to_string()];
        }

        let mut lines = Vec::new();
        let mut current_line = String::new();

        for word in text.split_whitespace() {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.len() + 1 + word.len() <= max_chars {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
            }
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
    fn render_text(
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
            let wrapped_lines = Self::wrap_text(line, text_width, font_size);
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
}

impl Default for PdfGenerateTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of AetherTool trait for PdfGenerateTool
#[async_trait]
impl AetherTool for PdfGenerateTool {
    const NAME: &'static str = "pdf_generate";
    const DESCRIPTION: &'static str = "Generate PDF documents from text or Markdown content.\n\n\
Features:\n\
- Plain text to PDF conversion\n\
- Markdown support (headings, paragraphs, lists, code blocks, bold, italic)\n\
- Configurable page size (A4, Letter, A3, or custom)\n\
- Adjustable font size, line spacing, and margins\n\n\
PATH RESOLUTION:\n\
- Relative paths (e.g., \"article.pdf\") → saved to ~/.config/aether/output/\n\
- Home paths (e.g., \"~/Desktop/doc.pdf\") → expanded to user's home directory\n\
- Absolute paths (e.g., \"/Users/name/doc.pdf\") → used as-is\n\n\
DEFAULT OUTPUT: Use relative paths like \"article.pdf\" or \"translated.pdf\" for generated PDFs.";

    type Args = PdfGenerateArgs;
    type Output = PdfGenerateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.generate(args).map_err(Into::into)
    }
}

// =============================================================================
// Transitional rig::tool::Tool implementation (to be removed in Phase 4)
// =============================================================================

impl rig::tool::Tool for PdfGenerateTool {
    const NAME: &'static str = "pdf_generate";

    type Args = PdfGenerateArgs;
    type Output = PdfGenerateOutput;
    type Error = ToolError;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Content to convert to PDF"
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Output file path for the PDF"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "markdown"],
                        "description": "Content format (default: text)"
                    },
                    "page_size": {
                        "type": "string",
                        "enum": ["a4", "letter", "a3"],
                        "description": "Page size (default: a4)"
                    },
                    "title": {
                        "type": "string",
                        "description": "Document title (optional)"
                    },
                    "font_size": {
                        "type": "number",
                        "description": "Font size in points (default: 12)"
                    },
                    "line_spacing": {
                        "type": "number",
                        "description": "Line spacing multiplier (default: 1.5)"
                    },
                    "margin_mm": {
                        "type": "number",
                        "description": "Page margins in mm (default: 20)"
                    }
                },
                "required": ["content", "output_path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        self.generate(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_wrap_text() {
        let text = "This is a long line of text that should be wrapped";
        let wrapped = PdfGenerateTool::wrap_text(text, 50.0, 12.0);
        assert!(!wrapped.is_empty());
    }

    #[test]
    fn test_page_size_dimensions() {
        let a4 = PageSize::A4;
        let (w, h) = a4.dimensions_mm();
        assert_eq!(w, 210.0);
        assert_eq!(h, 297.0);

        let letter = PageSize::Letter;
        let (w, h) = letter.dimensions_mm();
        assert!((w - 215.9).abs() < 0.1);
        assert!((h - 279.4).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_simple_pdf_generation() {
        let tool = PdfGenerateTool::new();
        let temp_dir = std::env::temp_dir();
        let output_path = temp_dir.join("test_simple.pdf");

        let args = PdfGenerateArgs {
            content: "Hello, World!\n\nThis is a test PDF.".to_string(),
            output_path: output_path.to_string_lossy().to_string(),
            format: ContentFormat::Text,
            page_size: PageSize::A4,
            title: Some("Test Document".to_string()),
            font_size: 12.0,
            line_spacing: 1.5,
            margin_mm: 20.0,
        };

        let result = tool.call(args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(output.success);
        assert_eq!(output.pages, 1);

        // Cleanup
        let _ = fs::remove_file(&output_path);
    }

    #[tokio::test]
    async fn test_markdown_pdf_generation() {
        let tool = PdfGenerateTool::new();
        let temp_dir = std::env::temp_dir();
        let output_path = temp_dir.join("test_markdown.pdf");

        let markdown_content = r#"# Main Title

This is a paragraph with some **bold** and *italic* text.

## Section 1

- Item 1
- Item 2
- Item 3

## Section 2

Some more text here.

```
code block
```
"#;

        let args = PdfGenerateArgs {
            content: markdown_content.to_string(),
            output_path: output_path.to_string_lossy().to_string(),
            format: ContentFormat::Markdown,
            page_size: PageSize::A4,
            title: None,
            font_size: 12.0,
            line_spacing: 1.5,
            margin_mm: 20.0,
        };

        let result = tool.call(args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(output.success);

        // Cleanup
        let _ = fs::remove_file(&output_path);
    }
}
