# PDF Dual-Engine Rendering Design

> **Date**: 2026-03-04
> **Status**: Draft
> **Scope**: Enhance `PdfGenerateTool` with browser-based Markdown rendering

## Problem

当 Aleph 生成报告时，LLM 输出 Markdown 格式内容，但当前 `PdfGenerateTool` 生成的 PDF 是未解析的 Markdown 原文（可见 `#`、`**`、`-` 等符号），而不是经过排版的格式化文档。

**根本原因**：现有实现使用 `printpdf` 低层 API 手工绘制文本，排版能力有限且可能存在格式传递的 bug。

## Solution

在现有 `PdfGenerateTool` 中增加 **Markdown → HTML+CSS → Headless Chrome → PDF** 的高质量渲染路径，保留现有 `printpdf` 路径作为 fallback。

### Architecture

```
LLM calls pdf_generate(content, format: "markdown")
        │
        ▼
  ┌─ Detect Chrome availability ─┐
  │                               │
  ▼                               ▼
[Browser Engine]            [Native Engine]
Markdown                    Markdown
  → pulldown-cmark → HTML    → pulldown-cmark → Events
  → Embedded CSS stylesheet    → printpdf manual drawing
  → chromiumoxide render       → Basic typography
  → PDF (GitHub-quality)       → PDF (current quality)
```

### Key Decisions

1. **Enhance existing tool, not new tool** — Avoids LLM confusion between competing tools (P2 High Cohesion)
2. **chromiumoxide already in Cargo.toml** — No new heavy dependency (R3 Core Minimalism)
3. **Auto-detect with fallback** — Defensive design (P7)
4. **Zero API change** — LLM calls unchanged, transparent upgrade

## Detailed Design

### 1. Interface Changes

Add optional `render_engine` field to `PdfGenerateArgs`:

```rust
/// Rendering engine preference
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderEngine {
    /// Auto-detect best available engine (default)
    #[default]
    Auto,
    /// Force headless browser rendering (requires Chrome/Chromium)
    Browser,
    /// Force native printpdf rendering
    Native,
}

pub struct PdfGenerateArgs {
    // ... existing fields unchanged ...

    /// Rendering engine (default: auto)
    #[serde(default)]
    pub render_engine: RenderEngine,
}
```

### 2. Browser Engine Flow

```
1. pulldown-cmark parses Markdown → HTML string
2. Wrap HTML with <style> block (GitHub-flavored CSS)
3. Write to temp file (data: URIs have length limits)
4. chromiumoxide launches headless Chrome
5. Navigate to file:///tmp/aleph_pdf_xxx.html
6. Page.printToPDF with configured page size/margins
7. Write PDF bytes to output_path
8. Cleanup temp file
```

#### CSS Stylesheet Features

- **Typography**: System font stack with CJK fallback chain
- **Headings**: H1-H6 with proportional sizing and spacing
- **Paragraphs**: Line-height 1.6, margin-bottom for spacing
- **Code blocks**: Monospace font, #f6f8fa background, rounded border, padding
- **Inline code**: Background highlight, slight border-radius
- **Tables**: Border-collapse, alternating row colors, header background
- **Lists**: Proper indentation, nested list support
- **Blockquotes**: Left border, italic, muted color
- **Links**: Colored, underlined
- **Page breaks**: Avoid orphans/widows via CSS `break-inside: avoid`

#### Chrome Detection Priority

```
1. CHROME_PATH environment variable
2. macOS: /Applications/Google Chrome.app/.../Google Chrome
3. macOS: /Applications/Chromium.app/.../Chromium
4. Linux: chromium-browser, google-chrome, chromium (via which)
5. Windows: Program Files paths
```

### 3. Native Engine (Existing)

Current `printpdf` + `pulldown-cmark` implementation moves to `native_engine.rs` unchanged. Acts as fallback when Chrome is unavailable.

### 4. File Organization

Current `pdf_generate.rs` (849 lines) → split into module directory:

```
core/src/builtin_tools/pdf_generate/
├── mod.rs              # PdfGenerateTool, AlephTool impl, engine dispatch
├── args.rs             # PdfGenerateArgs, PageSize, ContentFormat, RenderEngine
├── browser_engine.rs   # Markdown → HTML+CSS → Chrome → PDF
├── native_engine.rs    # Existing printpdf implementation
├── styles.rs           # Embedded CSS template (GitHub-flavored)
└── tests.rs            # Unit + integration tests
```

### 5. Error Handling & Degradation

| Scenario | Behavior |
|----------|----------|
| Chrome not installed | Auto → Native fallback, log `warn!` |
| Chrome launch timeout (10s) | Fallback to Native, log `warn!` |
| Chrome render failure | Fallback to Native, log `warn!` |
| `render_engine: "browser"` but no Chrome | Return error (explicit request = no silent fallback) |
| `render_engine: "native"` | Direct to Native, skip Chrome detection |

### 6. Performance Considerations

- Chrome launch is expensive (~1-3s) — consider keeping browser instance alive via `BrowserRuntime` (already in `core/src/browser/runtime.rs`)
- Reuse existing `BrowserRuntime` singleton if available
- Temp HTML file cleanup via `Drop` or explicit cleanup after PDF save

## Out of Scope

- Image embedding in Markdown (future enhancement)
- Custom CSS themes (future — could add `theme` field to args)
- Page headers/footers (Chrome `printToPDF` supports this natively, but defer)
- Table of Contents generation

## Testing Plan

1. **Unit**: CSS generation, HTML wrapping, Chrome detection
2. **Integration**: Full Markdown → PDF pipeline with Chrome (skip if no Chrome in CI)
3. **Fallback**: Verify graceful degradation when Chrome unavailable
4. **CJK**: Chinese/Japanese/Korean text rendering in both engines
5. **Visual**: Manual comparison of output quality

## Architecture Compliance

| Rule | Compliance |
|------|-----------|
| R1 (Brain-Limb Separation) | ✅ No platform APIs in Core |
| R3 (Core Minimalism) | ✅ Reuses existing chromiumoxide dep |
| P2 (High Cohesion) | ✅ Single tool, dual engines |
| P3 (Extensibility) | ✅ Engine enum allows future additions |
| P6 (Simplicity) | ✅ Auto-detect, zero config needed |
| P7 (Defensive Design) | ✅ Graceful degradation chain |
