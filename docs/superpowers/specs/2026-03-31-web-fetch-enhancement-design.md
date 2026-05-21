# Enhanced web_fetch: Readability + Markdown Output

**Date**: 2026-03-31
**Status**: Approved
**Scope**: `src/builtin_tools/web_fetch.rs`

## Problem

Current `web_fetch` has two critical deficiencies for LLM consumption:

1. **Loses all structure** — `el.text().collect::<String>()` flattens HTML to plain text. Headings, lists, code blocks, links all become undifferentiated whitespace-separated words.
2. **Weak noise removal** — 6 hardcoded CSS selectors (`article`, `main`, `.content`, `.post-content`, `#content`, `body`) cannot distinguish content from navigation, sidebars, ads, or footers.

**Impact**: When Aleph searches the web to answer user questions, the extracted content is noisy and unstructured, degrading LLM reasoning quality.

## Solution

Two-layer enhancement inside `web_fetch`, no external containers or services:

1. **Readability extraction** (noise removal) — Mozilla Readability algorithm replaces selector-based extraction
2. **HTML→Markdown conversion** (structure preservation) — Converts clean HTML to Markdown for LLM consumption

## Data Flow

```
HTML input
  │
  ▼
[Pre-clean] Remove <script>, <style>, hidden elements, zero-width chars
  │
  ▼
[Safety gate] HTML size ≤ 1MB, nesting depth ≤ 3000
  │
  ▼
[Readability] ──success──▶ Clean HTML fragment
  │ failure                     │
  ▼                             ▼
[Fallback: existing selectors] [htmd: HTML→Markdown]
  │                             │
  ▼                             ▼
[clean_text: plain text]    Structured Markdown
  │                             │
  └──────────┬──────────────────┘
             ▼
   [Truncate + content sanitization + output]
```

## Interface Changes

### WebFetchArgs (input)

```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
    /// Content extraction mode (default: markdown)
    #[serde(default)]
    pub extract_mode: ExtractMode,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExtractMode {
    #[default]
    Markdown,
    Text,
}
```

- `extract_mode` defaults to `Markdown` — backward compatible, existing callers get better output without changes
- `Text` mode preserves current behavior for any code that depends on plain text output

### WebFetchResult (output)

```rust
#[derive(Debug, Clone, Serialize)]
pub struct WebFetchResult {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
    /// Which extraction method was used
    pub extractor: Extractor,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Extractor {
    Readability,
    Selector,
}
```

- `extractor` field added for observability/debugging — indicates which path produced the content
- `content` field changes from plain text to Markdown (when `extract_mode = Markdown`)

## Implementation Details

### 1. Pre-cleaning (new function: `pre_clean_html`)

Before Readability processing, remove elements that pollute content extraction:

- `<script>`, `<style>`, `<noscript>` elements — executable/styling noise
- Elements with `display: none`, `visibility: hidden`, `aria-hidden="true"`, `hidden` attribute
- Common hidden CSS classes: `sr-only`, `visually-hidden`, `d-none`, `screen-reader-only`
- Zero-width Unicode characters: U+200B–U+200F, U+202A–U+202E, U+2060–U+2064, U+FEFF
- HTML comments

**Reference**: OpenClaw's `web-fetch-visibility.ts` pattern, adapted to Rust with `scraper` crate's DOM manipulation.

### 2. Safety Gates (new function: `validate_html_safety`)

Protect against malicious HTML:

- **Size limit**: Reject HTML > 1,000,000 characters (before DOM parsing)
- **Nesting depth**: Quick heuristic scan for deeply nested tag bombs (> 3000 levels). Reject without full parse.

These gates run before Readability to prevent memory/stack exhaustion.

### 3. Readability Extraction

Use `readability` crate (Rust port of Mozilla Readability):

```rust
fn extract_with_readability(&self, html: &str, url: &str) -> Option<String> {
    let mut doc = readability::extractor::extract(&html, url).ok()?;
    // Returns extracted HTML content (not plain text)
    let content = doc.content;
    if content.trim().is_empty() {
        return None;  // Trigger fallback
    }
    Some(content)
}
```

Returns `None` on failure → triggers fallback to existing selector logic.

### 4. HTML→Markdown Conversion

Use `htmd` crate for clean conversion:

```rust
fn html_to_markdown(&self, html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| {
        // If conversion fails, fall back to text extraction
        self.strip_tags(html)
    })
}
```

Preserves: headings (h1-h6), lists (ul/ol/li), code blocks, links, bold/italic, tables, blockquotes.

### 5. Fallback Chain

```rust
fn extract_content(&self, document: &Html, url: &str, mode: &ExtractMode) -> (String, Extractor) {
    // Try Readability first
    if let Some(readable_html) = self.extract_with_readability(&document_html, url) {
        let content = match mode {
            ExtractMode::Markdown => self.html_to_markdown(&readable_html),
            ExtractMode::Text => self.clean_text(&self.strip_tags(&readable_html)),
        };
        if content.len() > self.min_content_length {
            return (self.truncate_content(content), Extractor::Readability);
        }
    }

    // Fallback: existing selector-based extraction
    let content = self.extract_with_selectors(document, mode);
    (self.truncate_content(content), Extractor::Selector)
}
```

### 6. Unchanged Components

- **SSRF protection** (`safe_fetch`, `SsrfPolicy`) — untouched
- **Content sanitization** (`wrap_external_content`) — untouched, runs after extraction
- **WebFetchPolicy** — existing fields preserved, no breaking changes
- **Tool name** — remains `web_fetch`
- **Tool registration** — no changes to `BuiltinToolRegistry`

## New Dependencies

```toml
# Cargo.toml
readability = "0.3"    # Mozilla Readability algorithm (Rust port)
htmd = "0.1"           # HTML → Markdown conversion
```

Both are pure Rust crates with minimal transitive dependencies. No C/FFI bindings, no runtime services.

## Testing Strategy

### Unit Tests

1. **Readability extraction** — HTML with nav/sidebar/footer → extracts only article content
2. **Markdown output quality** — HTML with h2/ul/code/a → correct Markdown structure
3. **Fallback behavior** — Minimal HTML that Readability can't handle → falls back to selectors
4. **ExtractMode::Text** — Same extraction but plain text output (backward compat)
5. **Pre-cleaning** — Hidden elements removed, zero-width chars stripped
6. **Safety gates** — Oversized HTML rejected, deeply nested HTML rejected
7. **Extractor metadata** — Result correctly reports which extractor was used

### Integration Tests

1. **Real URL fetch** (`#[ignore]`) — Fetch example.com, verify Markdown output structure
2. **SSRF still works** — Localhost/private IPs still blocked after refactor

## Output Example

### Before (current)

```
Features Fast Safe Reliable Getting Started Install the package... Navigation Home About Contact Footer Copyright 2024
```

### After (enhanced)

```markdown
## Features

- Fast
- Safe
- Reliable

## Getting Started

Install the package...
```

## Scope Boundaries

**In scope:**
- Readability + Markdown extraction pipeline
- Pre-cleaning and safety gates
- ExtractMode parameter
- Fallback chain
- Unit tests

**Out of scope:**
- SearXNG search backend integration (separate spec)
- URL caching (not needed for current use patterns)
- Firecrawl or any external service integration
- Changes to browser_tools
