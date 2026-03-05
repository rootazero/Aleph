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

// ── Browser engine unit tests ───────────────────────────────────────────

#[test]
fn test_css_contains_essential_rules() {
    let css = super::styles::github_markdown_css();
    assert!(css.contains("font-family"));
    assert!(css.contains("code"));
    assert!(css.contains("table"));
    assert!(css.contains("blockquote"));
}

#[test]
fn test_markdown_to_html_conversion() {
    let md = "# Hello\n\nThis is **bold** and *italic*.\n\n- item 1\n- item 2\n";
    let html = super::browser_engine::markdown_to_html(md);
    assert!(html.contains("<h1>"));
    assert!(html.contains("<strong>"));
    assert!(html.contains("<em>"));
    assert!(html.contains("<li>"));
}

#[test]
fn test_markdown_to_html_with_table() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
    let html = super::browser_engine::markdown_to_html(md);
    assert!(html.contains("<table>"));
    assert!(html.contains("<th>"));
}

#[test]
fn test_markdown_to_html_with_code_block() {
    let md = "```rust\nfn main() {}\n```\n";
    let html = super::browser_engine::markdown_to_html(md);
    assert!(html.contains("<pre>"));
    // pulldown-cmark adds class="language-rust" so we match the tag prefix
    assert!(html.contains("<code"));
}

#[test]
fn test_build_full_html_document() {
    let md = "# Test\n\nContent here.\n";
    let doc = super::browser_engine::build_html_document(md, Some("My Title"));
    assert!(doc.contains("<!DOCTYPE html>"));
    assert!(doc.contains("My Title"));
    assert!(doc.contains("<h1>"));
    assert!(doc.contains("font-family"));
}

// ── Browser engine integration tests ────────────────────────────────────

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
| Headings | Done |
| Lists | Done |
| Tables | Done |
| Code blocks | Done |

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

// ── Content format auto-detection ────────────────────────────────────────

#[test]
fn test_detect_markdown_headings() {
    let content = "# Title\n\nSome text\n\n## Section\n\nMore text";
    assert!(matches!(ContentFormat::detect(content), ContentFormat::Markdown));
}

#[test]
fn test_detect_markdown_mixed() {
    let content = "# Report\n\n**Bold text** and *italic*.\n\n- item 1\n- item 2\n";
    assert!(matches!(ContentFormat::detect(content), ContentFormat::Markdown));
}

#[test]
fn test_detect_plain_text() {
    let content = "Hello, this is just plain text.\nNothing special here.";
    assert!(matches!(ContentFormat::detect(content), ContentFormat::Text));
}

#[test]
fn test_detect_markdown_code_blocks() {
    let content = "Here is some code:\n\n```rust\nfn main() {}\n```\n\nEnd.";
    assert!(matches!(ContentFormat::detect(content), ContentFormat::Markdown));
}

#[test]
fn test_detect_markdown_links() {
    let content = "Check out [this link](https://example.com) for details.\n\nAlso **bold**.";
    assert!(matches!(ContentFormat::detect(content), ContentFormat::Markdown));
}

// ── Schema verification ─────────────────────────────────────────────────

#[test]
fn test_args_schema_includes_render_engine() {
    let schema = schemars::schema_for!(PdfGenerateArgs);
    let json = serde_json::to_string_pretty(&schema).unwrap();
    assert!(json.contains("render_engine"), "Schema should include render_engine field");
    assert!(json.contains("auto"), "Schema should include auto option");
    assert!(json.contains("browser"), "Schema should include browser option");
    assert!(json.contains("native"), "Schema should include native option");
}
