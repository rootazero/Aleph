# Enhanced web_fetch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `web_fetch` to produce clean, structured Markdown output using Readability extraction + HTML→Markdown conversion, with graceful fallback to existing selector-based extraction.

**Architecture:** Two new crates (`readability`, `htmd`) handle content extraction and format conversion. A pre-cleaning pass removes noise elements before Readability runs. The existing selector-based extraction becomes the fallback path. No external services or containers.

**Tech Stack:** Rust, `readability` crate (Mozilla Readability port), `htmd` crate (HTML→Markdown), `scraper` (existing)

**Spec:** `docs/superpowers/specs/2026-03-31-web-fetch-enhancement-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `Cargo.toml` | Modify | Add `readability` and `htmd` dependencies |
| `src/builtin_tools/web_fetch.rs` | Modify | Main enhancement: pre-clean, readability, markdown conversion, fallback chain |
| `src/config/types/policies/web_fetch.rs` | Modify | Add `enable_readability` policy field |

---

### Task 1: Add Dependencies

**Files:**
- Modify: `Cargo.toml:113` (near existing `scraper` dependency)

- [ ] **Step 1: Add readability and htmd crates to Cargo.toml**

In `Cargo.toml`, after the `scraper = "0.22"` line (line 113), add:

```toml
# For Readability content extraction (Mozilla algorithm)
readability = "0.3"
# For HTML to Markdown conversion
htmd = "0.1"
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`

Expected: Successful compilation (may show warnings, no errors).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "deps: add readability and htmd crates for web_fetch enhancement"
```

---

### Task 2: Add ExtractMode and Extractor Types

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs:1-35`

- [ ] **Step 1: Write failing test for ExtractMode deserialization**

Add at the bottom of the existing `#[cfg(test)] mod tests` block in `web_fetch.rs`:

```rust
    #[test]
    fn test_extract_mode_defaults_to_markdown() {
        let args: WebFetchArgs = serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
        assert!(matches!(args.extract_mode, ExtractMode::Markdown));
    }

    #[test]
    fn test_extract_mode_text() {
        let args: WebFetchArgs = serde_json::from_str(
            r#"{"url": "https://example.com", "extract_mode": "text"}"#
        ).unwrap();
        assert!(matches!(args.extract_mode, ExtractMode::Text));
    }

    #[test]
    fn test_extractor_serialization() {
        let result = WebFetchResult {
            url: "https://example.com".to_string(),
            title: Some("Test".to_string()),
            content: "# Hello".to_string(),
            extractor: Extractor::Readability,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["extractor"], "readability");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib web_fetch::tests::test_extract_mode -- 2>&1 | tail -10`

Expected: FAIL — `ExtractMode` type does not exist yet.

- [ ] **Step 3: Add ExtractMode, Extractor enums and update WebFetchArgs/WebFetchResult**

Replace the existing `WebFetchArgs` and `WebFetchResult` structs (lines 17-34) with:

```rust
/// Content extraction mode
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExtractMode {
    /// Structured Markdown output (default)
    #[default]
    Markdown,
    /// Plain text output (legacy behavior)
    Text,
}

/// Which extraction method produced the content
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Extractor {
    /// Mozilla Readability algorithm
    Readability,
    /// CSS selector-based fallback
    Selector,
}

/// Arguments for web fetch tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
    /// Content extraction mode (default: markdown)
    #[serde(default)]
    pub extract_mode: ExtractMode,
}

/// Web fetch result containing extracted content
#[derive(Debug, Clone, Serialize)]
pub struct WebFetchResult {
    /// The fetched URL
    pub url: String,
    /// Page title extracted from <title> tag
    pub title: Option<String>,
    /// Main text content extracted from the page
    pub content: String,
    /// Which extraction method was used
    pub extractor: Extractor,
}
```

- [ ] **Step 4: Fix call_impl to populate the new `extractor` field**

In `call_impl`, update the `Ok(WebFetchResult { ... })` return (around line 169) to include `extractor`:

```rust
        Ok(WebFetchResult {
            url: args.url,
            title,
            content: wrapped_content,
            extractor: Extractor::Selector, // Temporary: will be updated in Task 4
        })
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib web_fetch::tests -- 2>&1 | tail -15`

Expected: All tests PASS, including the 3 new ones.

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "feat(web_fetch): add ExtractMode and Extractor types"
```

---

### Task 3: Implement Pre-cleaning and Safety Gates

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs`

- [ ] **Step 1: Write failing tests for pre-cleaning and safety gates**

Add to the `tests` module:

```rust
    #[test]
    fn test_pre_clean_removes_script_and_style() {
        let html = r#"<html><body>
            <script>alert('xss')</script>
            <style>.hide { display: none; }</style>
            <p>Visible content here</p>
        </body></html>"#;
        let cleaned = WebFetchTool::pre_clean_html(html);
        assert!(!cleaned.contains("alert"));
        assert!(!cleaned.contains("display: none"));
        assert!(cleaned.contains("Visible content here"));
    }

    #[test]
    fn test_pre_clean_removes_hidden_elements() {
        let html = r#"<html><body>
            <div style="display:none">Hidden</div>
            <div aria-hidden="true">Aria hidden</div>
            <div hidden>Attr hidden</div>
            <p>Visible</p>
        </body></html>"#;
        let cleaned = WebFetchTool::pre_clean_html(html);
        assert!(!cleaned.contains("Hidden"));
        assert!(!cleaned.contains("Aria hidden"));
        assert!(!cleaned.contains("Attr hidden"));
        assert!(cleaned.contains("Visible"));
    }

    #[test]
    fn test_pre_clean_strips_zero_width_chars() {
        let html = "<html><body><p>Hello\u{200B}World\u{FEFF}Test</p></body></html>";
        let cleaned = WebFetchTool::pre_clean_html(html);
        assert!(cleaned.contains("HelloWorldTest"));
    }

    #[test]
    fn test_safety_gate_rejects_oversized_html() {
        let huge = "a".repeat(1_100_000);
        assert!(WebFetchTool::validate_html_safety(&huge).is_err());
    }

    #[test]
    fn test_safety_gate_accepts_normal_html() {
        let normal = "<html><body><p>Hello</p></body></html>";
        assert!(WebFetchTool::validate_html_safety(normal).is_ok());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib web_fetch::tests::test_pre_clean -- 2>&1 | tail -10`

Expected: FAIL — `pre_clean_html` and `validate_html_safety` do not exist.

- [ ] **Step 3: Implement pre_clean_html and validate_html_safety**

Add these associated functions to `impl WebFetchTool` (before the `extract_title` method):

```rust
    /// Maximum HTML size in characters before safety rejection
    const MAX_HTML_SIZE: usize = 1_000_000;

    /// Validate HTML is safe to process (size and structure checks)
    pub(crate) fn validate_html_safety(html: &str) -> std::result::Result<(), ToolError> {
        if html.len() > Self::MAX_HTML_SIZE {
            return Err(ToolError::Execution(format!(
                "HTML too large: {} bytes (max {} bytes)",
                html.len(),
                Self::MAX_HTML_SIZE,
            )));
        }
        Ok(())
    }

    /// Pre-clean HTML by removing noise elements before content extraction.
    ///
    /// Removes: <script>, <style>, <noscript>, hidden elements,
    /// zero-width Unicode characters, and HTML comments.
    pub(crate) fn pre_clean_html(html: &str) -> String {
        // Step 1: Strip zero-width Unicode characters
        let html = html
            .replace('\u{200B}', "")  // Zero-width space
            .replace('\u{200C}', "")  // Zero-width non-joiner
            .replace('\u{200D}', "")  // Zero-width joiner
            .replace('\u{200E}', "")  // Left-to-right mark
            .replace('\u{200F}', "")  // Right-to-left mark
            .replace('\u{FEFF}', "")  // BOM / zero-width no-break space
            .replace('\u{2060}', ""); // Word joiner

        // Step 2: Remove HTML comments
        let comment_re = regex::Regex::new(r"<!--[\s\S]*?-->").unwrap();
        let html = comment_re.replace_all(&html, "").to_string();

        // Step 3: Remove script, style, noscript blocks (including content)
        let block_re = regex::Regex::new(
            r"(?is)<(script|style|noscript)[^>]*>[\s\S]*?</\1>"
        ).unwrap();
        let html = block_re.replace_all(&html, "").to_string();

        // Step 4: Remove hidden elements via DOM parsing
        let document = Html::parse_document(&html);
        let mut removals: Vec<scraper::node::NodeId> = Vec::new();

        for node_ref in document.tree.nodes() {
            if let Some(element) = node_ref.value().as_element() {
                let should_remove = 
                    // hidden attribute
                    element.attr("hidden").is_some()
                    // aria-hidden="true"
                    || element.attr("aria-hidden").map_or(false, |v| v == "true")
                    // style contains display:none or visibility:hidden
                    || element.attr("style").map_or(false, |s| {
                        let s = s.to_lowercase();
                        s.contains("display:none") || s.contains("display: none")
                            || s.contains("visibility:hidden") || s.contains("visibility: hidden")
                    })
                    // Common hidden CSS classes
                    || element.attr("class").map_or(false, |c| {
                        let c = c.to_lowercase();
                        c.contains("sr-only") || c.contains("visually-hidden")
                            || c.contains("d-none") || c.contains("screen-reader-only")
                    });

                if should_remove {
                    removals.push(node_ref.id());
                }
            }
        }

        // Rebuild HTML without removed nodes by serializing the cleaned DOM.
        // Since scraper's Html is immutable, we use a regex approach to strip
        // identified hidden elements by their attributes.
        let hidden_re = regex::Regex::new(
            r#"(?is)<[a-z][^>]*(?:hidden|aria-hidden\s*=\s*"true"|style\s*=\s*"[^"]*(?:display\s*:\s*none|visibility\s*:\s*hidden)[^"]*"|class\s*=\s*"[^"]*(?:sr-only|visually-hidden|d-none|screen-reader-only)[^"]*")[^>]*>[\s\S]*?</[a-z]+>"#
        ).unwrap();
        let html = hidden_re.replace_all(&html, "").to_string();

        html
    }
```

Note: The function uses regex-based removal since `scraper::Html` does not support mutable DOM manipulation. The DOM parse was used for detection logic reference but the actual removal uses regex patterns matching the same attributes.

Remove the unused `removals` vector and DOM parsing block — replace the implementation with this cleaner version:

```rust
    pub(crate) fn pre_clean_html(html: &str) -> String {
        // Step 1: Strip zero-width Unicode characters
        let html = html
            .replace('\u{200B}', "")
            .replace('\u{200C}', "")
            .replace('\u{200D}', "")
            .replace('\u{200E}', "")
            .replace('\u{200F}', "")
            .replace('\u{FEFF}', "")
            .replace('\u{2060}', "");

        // Step 2: Remove HTML comments
        let comment_re = regex::Regex::new(r"<!--[\s\S]*?-->").unwrap();
        let html = comment_re.replace_all(&html, "").to_string();

        // Step 3: Remove script, style, noscript blocks
        let block_re = regex::Regex::new(
            r"(?is)<(script|style|noscript)[^>]*>[\s\S]*?</\1>"
        ).unwrap();
        let html = block_re.replace_all(&html, "").to_string();

        // Step 4: Remove elements with hidden attributes/styles/classes
        let hidden_re = regex::Regex::new(
            r#"(?is)<([a-z]\w*)\s[^>]*(?:\bhidden\b|aria-hidden\s*=\s*"true"|style\s*=\s*"[^"]*(?:display\s*:\s*none|visibility\s*:\s*hidden)[^"]*"|class\s*=\s*"[^"]*(?:\bsr-only\b|\bvisually-hidden\b|\bd-none\b|\bscreen-reader-only\b)[^"]*")[^>]*>[\s\S]*?</\1>"#
        ).unwrap();
        let html = hidden_re.replace_all(&html, "").to_string();

        html
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib web_fetch::tests::test_pre_clean -- 2>&1 | tail -15`
Run: `cargo test -p alephcore --lib web_fetch::tests::test_safety_gate -- 2>&1 | tail -10`

Expected: All 5 new tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "feat(web_fetch): add pre-cleaning and safety gates"
```

---

### Task 4: Implement Readability + Markdown Extraction Pipeline

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs`

- [ ] **Step 1: Write failing tests for readability extraction and markdown conversion**

Add to the `tests` module:

```rust
    #[test]
    fn test_readability_extraction_produces_content() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test Article</title></head>
        <body>
            <nav><a href="/">Home</a><a href="/about">About</a></nav>
            <article>
                <h1>Main Article Title</h1>
                <p>This is the first paragraph of the article with enough content to be recognized by the readability algorithm as meaningful text content that should be extracted.</p>
                <p>This is the second paragraph providing additional detail about the topic being discussed in this article. It contains several sentences to ensure adequate length.</p>
                <h2>Section Two</h2>
                <ul>
                    <li>First item in the list</li>
                    <li>Second item in the list</li>
                    <li>Third item in the list</li>
                </ul>
                <p>A concluding paragraph that wraps up the discussion and provides final thoughts on the matter at hand.</p>
            </article>
            <footer>Copyright 2024 | Privacy Policy | Terms</footer>
        </body></html>"#;

        let tool = WebFetchTool::new();
        let (content, extractor) = tool.extract_content_enhanced(html, "https://example.com", &ExtractMode::Markdown);

        // Should use readability extractor
        assert!(matches!(extractor, Extractor::Readability), "Expected Readability extractor, got {:?}", extractor);
        // Should contain markdown headings
        assert!(content.contains('#'), "Expected markdown headings in output: {}", content);
        // Should NOT contain nav/footer noise
        assert!(!content.contains("Privacy Policy"), "Footer content should be removed: {}", content);
    }

    #[test]
    fn test_text_mode_produces_plain_text() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body><article>
            <h1>Title</h1>
            <p>This is a paragraph with enough content for readability to extract it properly as meaningful text content in the article body.</p>
            <p>Another paragraph with sufficient length to ensure the readability algorithm recognizes this as article content worth preserving.</p>
        </article></body></html>"#;

        let tool = WebFetchTool::new();
        let (content, _) = tool.extract_content_enhanced(html, "https://example.com", &ExtractMode::Text);

        // Text mode should not contain markdown syntax
        assert!(!content.contains("# "), "Text mode should not have markdown headings: {}", content);
        // But should contain the actual text
        assert!(content.contains("paragraph"), "Should contain article text: {}", content);
    }

    #[test]
    fn test_fallback_to_selector_on_minimal_html() {
        let html = "<html><body><p>Short</p></body></html>";
        let tool = WebFetchTool::new();
        let (_, extractor) = tool.extract_content_enhanced(html, "https://example.com", &ExtractMode::Markdown);

        // Readability should fail on minimal content, falling back to selector
        assert!(matches!(extractor, Extractor::Selector), "Expected Selector fallback, got {:?}", extractor);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib web_fetch::tests::test_readability -- 2>&1 | tail -10`

Expected: FAIL — `extract_content_enhanced` does not exist.

- [ ] **Step 3: Implement extract_content_enhanced**

Add these methods to `impl WebFetchTool`:

```rust
    /// Extract content using Readability algorithm, returning clean HTML.
    /// Returns None if Readability fails or produces insufficient content.
    fn extract_with_readability(&self, html: &str, url: &str) -> Option<String> {
        let readable = readability::extractor::extract(
            &mut html.as_bytes(),
            &url.parse().ok()?,
        ).ok()?;

        let content = readable.content;
        if content.trim().is_empty() || content.trim().len() < self.min_content_length {
            return None;
        }
        Some(content)
    }

    /// Convert HTML fragment to Markdown using htmd
    fn html_to_markdown(&self, html: &str) -> String {
        htmd::convert(html).unwrap_or_else(|_| self.clean_text(&self.strip_tags(html)))
    }

    /// Strip HTML tags, keeping only text content (fallback for failed markdown conversion)
    fn strip_tags(&self, html: &str) -> String {
        let tag_re = regex::Regex::new(r"<[^>]+>").unwrap();
        tag_re.replace_all(html, "").to_string()
    }

    /// Enhanced content extraction with Readability + Markdown pipeline.
    ///
    /// Tries Readability first, falls back to selector-based extraction.
    /// Returns (content, extractor_used).
    pub(crate) fn extract_content_enhanced(
        &self,
        raw_html: &str,
        url: &str,
        mode: &ExtractMode,
    ) -> (String, Extractor) {
        // Pre-clean HTML
        let cleaned_html = Self::pre_clean_html(raw_html);

        // Try Readability extraction
        if let Some(readable_html) = self.extract_with_readability(&cleaned_html, url) {
            let content = match mode {
                ExtractMode::Markdown => self.html_to_markdown(&readable_html),
                ExtractMode::Text => self.clean_text(&self.strip_tags(&readable_html)),
            };
            if content.len() > self.min_content_length {
                return (self.truncate_content(content), Extractor::Readability);
            }
        }

        // Fallback: selector-based extraction (existing logic)
        let document = Html::parse_document(&cleaned_html);
        let content = self.extract_content(&document);
        let content = match mode {
            ExtractMode::Markdown => content, // selector path returns plain text regardless
            ExtractMode::Text => content,
        };
        (self.truncate_content(content), Extractor::Selector)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib web_fetch::tests -- 2>&1 | tail -20`

Expected: All tests PASS. Note: the readability test may need the article HTML to be long enough for the algorithm to detect it. If `test_readability_extraction_produces_content` fails because Readability returns None, increase the paragraph lengths in the test HTML.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "feat(web_fetch): implement Readability + Markdown extraction pipeline"
```

---

### Task 5: Wire Enhanced Pipeline into call_impl

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs` (the `call_impl` method)

- [ ] **Step 1: Update call_impl to use the enhanced extraction pipeline**

Replace the content extraction section in `call_impl` (approximately lines 144-173). The current code does:

```rust
        // Parse HTML
        let document = Html::parse_document(&html_content);
        let title = self.extract_title(&document);
        let content = self.extract_content(&document);
        // ... notify + wrap + return
```

Replace with:

```rust
        // Safety gate: reject oversized HTML
        Self::validate_html_safety(&html_content).map_err(|e| {
            notify_tool_result(Self::NAME, &e.to_string(), false);
            e
        })?;

        // Extract title from raw HTML (before pre-cleaning)
        let document = Html::parse_document(&html_content);
        let title = self.extract_title(&document);
        debug!("Extracted title: {:?}", title);

        // Enhanced extraction: Readability + Markdown with selector fallback
        let (content, extractor) = self.extract_content_enhanced(
            &html_content,
            &args.url,
            &args.extract_mode,
        );
        debug!("Extracted {} chars via {:?} extractor", content.len(), extractor);

        // Notify success
        let extractor_name = match extractor {
            Extractor::Readability => "readability",
            Extractor::Selector => "selector",
        };
        let result_summary = format!(
            "已获取网页内容 ({} 字符, {})",
            content.len(),
            extractor_name,
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        // Wrap with external content boundary markers
        let wrapped_content = wrap_external_content(
            &content,
            ContentSource::WebFetch { url: args.url.clone() },
        );

        Ok(WebFetchResult {
            url: args.url,
            title,
            content: wrapped_content,
            extractor,
        })
```

- [ ] **Step 2: Run all tests**

Run: `cargo test -p alephcore --lib web_fetch -- 2>&1 | tail -20`

Expected: All tests PASS.

- [ ] **Step 3: Run cargo check to verify full compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`

Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "feat(web_fetch): wire enhanced extraction into call_impl"
```

---

### Task 6: Add enable_readability Policy Field

**Files:**
- Modify: `src/config/types/policies/web_fetch.rs`
- Modify: `src/builtin_tools/web_fetch.rs`

- [ ] **Step 1: Add enable_readability field to WebFetchPolicy**

In `src/config/types/policies/web_fetch.rs`, add to the `WebFetchPolicy` struct after `content_selectors`:

```rust
    /// Whether to use Readability algorithm for content extraction
    /// When false, falls back to CSS selector-based extraction
    /// Default: true
    #[serde(default = "default_enable_readability")]
    pub enable_readability: bool,
```

Add the default function:

```rust
fn default_enable_readability() -> bool {
    true
}
```

Update `Default` impl to include:

```rust
            enable_readability: default_enable_readability(),
```

- [ ] **Step 2: Update WebFetchTool to respect the policy**

In `web_fetch.rs`, add a field to `WebFetchTool`:

```rust
    /// Whether Readability extraction is enabled
    enable_readability: bool,
```

Update `new()` to set `enable_readability: true`.

Update `with_policy()` to set `enable_readability: policy.enable_readability`.

Update `Clone` impl to include `enable_readability: self.enable_readability`.

Update `extract_content_enhanced` to skip readability when disabled:

```rust
        // Try Readability extraction (if enabled)
        if self.enable_readability {
            if let Some(readable_html) = self.extract_with_readability(&cleaned_html, url) {
                // ... existing readability path
            }
        }
```

- [ ] **Step 3: Add test for policy toggle**

In `web_fetch.rs` tests:

```rust
    #[test]
    fn test_readability_disabled_uses_selector() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body><article>
            <h1>Title</h1>
            <p>Long enough paragraph for readability to normally extract this content from the article body with sufficient detail.</p>
            <p>Another paragraph ensuring adequate length for the readability algorithm to process.</p>
        </article></body></html>"#;

        let mut tool = WebFetchTool::new();
        tool.enable_readability = false;
        let (_, extractor) = tool.extract_content_enhanced(html, "https://example.com", &ExtractMode::Markdown);

        assert!(matches!(extractor, Extractor::Selector), "Should use Selector when readability disabled");
    }
```

- [ ] **Step 4: Run all tests**

Run: `cargo test -p alephcore --lib web_fetch -- 2>&1 | tail -15`
Run: `cargo test -p alephcore --lib policies::web_fetch -- 2>&1 | tail -10`

Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config/types/policies/web_fetch.rs src/builtin_tools/web_fetch.rs
git commit -m "feat(web_fetch): add enable_readability policy toggle"
```

---

### Task 7: Integration Test and Final Verification

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs` (integration test)

- [ ] **Step 1: Update existing integration test**

Update the existing `test_web_fetch_call` test (marked `#[ignore]`) to verify Markdown output:

```rust
    #[tokio::test]
    #[ignore] // Requires network connection
    async fn test_web_fetch_call() {
        let tool = WebFetchTool::new();
        let args = WebFetchArgs {
            url: "https://example.com".to_string(),
            extract_mode: ExtractMode::Markdown,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result);

        let result = result.unwrap();
        assert_eq!(result.url, "https://example.com");
        assert!(result.title.is_some(), "Expected title to be present");
        assert!(!result.content.is_empty(), "Expected content to be present");
        // Verify extractor metadata is populated
        assert!(matches!(result.extractor, Extractor::Readability | Extractor::Selector));
    }
```

- [ ] **Step 2: Verify SSRF tests still pass**

Run: `cargo test -p alephcore --lib web_fetch::tests::test_ssrf -- 2>&1 | tail -10`

Expected: All SSRF tests PASS (unchanged).

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`

Expected: All tests PASS, no regressions.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -10`

Expected: No warnings or errors.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "test(web_fetch): update integration test for enhanced extraction"
```

---

## Verification Checklist

After all tasks complete:

- [ ] `cargo check -p alephcore` — compiles without errors
- [ ] `cargo test -p alephcore --lib web_fetch` — all unit tests pass
- [ ] `cargo test -p alephcore --lib policies::web_fetch` — policy tests pass
- [ ] `cargo clippy -p alephcore -- -D warnings` — no lint warnings
- [ ] No changes to SSRF protection (`security/ssrf/`)
- [ ] No changes to content sanitization (`security/content_sanitizer.rs`)
- [ ] `WebFetchArgs` without `extract_mode` still works (defaults to Markdown)
- [ ] `WebFetchResult` now includes `extractor` field
