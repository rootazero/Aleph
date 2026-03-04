//! Tests for the PDF generation tool

use super::*;
use super::args::*;
use std::fs;

#[test]
fn test_wrap_text() {
    let text = "This is a long line of text that should be wrapped";
    let wrapped = native_engine::wrap_text(text, 50.0, 12.0);
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

#[test]
fn test_page_size_dimensions_inches() {
    let a4 = PageSize::A4;
    let (w, h) = a4.dimensions_inches();
    assert!((w - 8.2677).abs() < 0.001);
    assert!((h - 11.6929).abs() < 0.001);

    let letter = PageSize::Letter;
    let (w, h) = letter.dimensions_inches();
    assert!((w - 8.5).abs() < 0.01);
    assert!((h - 11.0).abs() < 0.01);
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
        render_engine: RenderEngine::Native,
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
        render_engine: RenderEngine::Auto,
    };

    let result = tool.call(args).await;
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.success);

    // Cleanup
    let _ = fs::remove_file(&output_path);
}

#[tokio::test]
async fn test_chinese_pdf_generation() {
    let tool = PdfGenerateTool::new();
    let temp_dir = std::env::temp_dir();
    let output_path = temp_dir.join("test_chinese.pdf");

    let content = "# 比特币价格报告\n\n\
        ## 市场概览\n\n\
        当前比特币价格约为 67,284 美元，折合人民币约 464,274 元。\n\n\
        ## 趋势分析\n\n\
        - 24小时涨幅：+2.3%\n\
        - 7天涨幅：+5.1%\n\
        - 30天涨幅：+12.8%\n\n\
        市场整体呈现上涨趋势，投资者情绪积极。";

    let args = PdfGenerateArgs {
        content: content.to_string(),
        output_path: output_path.to_string_lossy().to_string(),
        format: ContentFormat::Markdown,
        page_size: PageSize::A4,
        title: Some("比特币交易报告".to_string()),
        font_size: 12.0,
        line_spacing: 1.5,
        margin_mm: 20.0,
        render_engine: RenderEngine::Native,
    };

    let result = tool.call(args).await;
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.success);
    assert!(output.pages >= 1);

    // Verify the file is non-trivial (CJK content should produce reasonable size)
    let metadata = fs::metadata(&output_path).unwrap();
    assert!(metadata.len() > 1000, "PDF should have substantial content");

    // Cleanup
    let _ = fs::remove_file(&output_path);
}

#[test]
fn test_wrap_text_cjk() {
    // CJK characters should be counted as 2 units wide
    let chinese = "这是一段测试中文文本，用于验证换行功能";
    let wrapped = native_engine::wrap_text(chinese, 30.0, 12.0);
    // With ~30mm at 12pt, about 10 units, CJK chars are 2 units each -> ~5 chars/line
    assert!(wrapped.len() > 1, "Chinese text should wrap to multiple lines");
}

#[test]
fn test_is_cjk() {
    assert!(native_engine::is_cjk('中'));
    assert!(native_engine::is_cjk('の'));
    assert!(native_engine::is_cjk('한'));
    assert!(!native_engine::is_cjk('A'));
    assert!(!native_engine::is_cjk('1'));
}
