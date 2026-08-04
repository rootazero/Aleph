//! Content extraction helpers for the web fetch tool.

use super::cache::resolve_relative_urls;
use super::types::{ExtractMode, Extractor};
use crate::builtin_tools::error::ToolError;
use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use tracing::debug;

// Pre-compiled regexes for HTML cleaning (compiled once, reused forever).
// Patterns are static literals; a build-time failure here indicates a regex
// bug that should be caught in tests, so we use unreachable! to signal an
// invariant rather than panicking in library code.

static RE_COMMENTS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)<!--.*?-->").unwrap_or_else(|e| unreachable!("invalid regex RE_COMMENTS: {e}"))
});
static RE_SCRIPT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?si)<script(\s[^>]*)?>.*?</script\s*>")
        .unwrap_or_else(|e| unreachable!("invalid regex RE_SCRIPT: {e}"))
});
static RE_STYLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?si)<style(\s[^>]*)?>.*?</style\s*>")
        .unwrap_or_else(|e| unreachable!("invalid regex RE_STYLE: {e}"))
});
static RE_NOSCRIPT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?si)<noscript(\s[^>]*)?>.*?</noscript\s*>")
        .unwrap_or_else(|e| unreachable!("invalid regex RE_NOSCRIPT: {e}"))
});
static RE_HIDDEN_ATTR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?si)<[^>]+\shidden(\s[^>]*)?>.*?</[^>]+>")
        .unwrap_or_else(|e| unreachable!("invalid regex RE_HIDDEN_ATTR: {e}"))
});
static RE_ARIA_HIDDEN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?si)<[^>]+\saria-hidden\s*=\s*["']true["'][^>]*>.*?</[^>]+>"#)
        .unwrap_or_else(|e| unreachable!("invalid regex RE_ARIA_HIDDEN: {e}"))
});
static RE_DISPLAY_NONE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?si)<[^>]+\sstyle\s*=\s*["'][^"']*(?:display\s*:\s*none|visibility\s*:\s*hidden)[^"']*["'][^>]*>.*?</[^>]+>"#,
    )
    .unwrap_or_else(|e| unreachable!("invalid regex RE_DISPLAY_NONE: {e}"))
});
static RE_SR_CLASSES: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?si)<[^>]+\sclass\s*=\s*["'][^"']*(?:sr-only|visually-hidden|d-none|screen-reader-only)[^"']*["'][^>]*>.*?</[^>]+>"#,
    )
    .unwrap_or_else(|e| unreachable!("invalid regex RE_SR_CLASSES: {e}"))
});
static RE_STRIP_TAGS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"<[^>]+>").unwrap_or_else(|e| unreachable!("invalid regex RE_STRIP_TAGS: {e}"))
});

// Surface `<time datetime="…">` machine timestamps into the element's own
// text. News/listing pages frequently carry the absolute publication time
// ONLY in the `datetime` attribute (visible text is relative, e.g. "2h ago",
// or rendered later by JS). Both `.text()` extraction and Markdown
// conversion drop attributes, so without this the timestamp is lost — which
// is what made fetched pages report "publish time: not provided".
static RE_TIME_DATETIME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?si)<time\b[^>]*\bdatetime\s*=\s*["']([^"']+)["'][^>]*>(.*?)</time>"#)
        .unwrap_or_else(|e| unreachable!("invalid regex RE_TIME_DATETIME: {e}"))
});

/// Extract the page title from <title> tag
pub(crate) fn extract_title(document: &Html) -> Option<String> {
    let selector = Selector::parse("title").ok()?;
    document
        .select(&selector)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Clean whitespace from text (collapse multiple spaces)
pub(crate) fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip HTML tags from a string using a simple regex, leaving plain text.
fn strip_tags(html: &str) -> String {
    let text = RE_STRIP_TAGS.replace_all(html, " ").to_string();
    clean_text(&text)
}

/// Convert an HTML string to Markdown using `htmd`. Falls back to
/// tag-stripping on conversion failure.
pub(crate) fn html_to_markdown(html: &str) -> String {
    match htmd::convert(html) {
        Ok(md) => md,
        Err(_) => strip_tags(html),
    }
}

/// Remove noise from raw HTML before extraction:
/// zero-width Unicode, HTML comments, script/style/noscript blocks,
/// and elements that are visually hidden.
pub(crate) fn pre_clean_html(html: &str) -> String {
    // 1. Strip zero-width / invisible Unicode characters
    let zero_width: &[char] = &[
        '\u{200B}', // ZERO WIDTH SPACE
        '\u{200C}', // ZERO WIDTH NON-JOINER
        '\u{200D}', // ZERO WIDTH JOINER
        '\u{200E}', // LEFT-TO-RIGHT MARK
        '\u{200F}', // RIGHT-TO-LEFT MARK
        '\u{FEFF}', // ZERO WIDTH NO-BREAK SPACE (BOM)
        '\u{2060}', // WORD JOINER
    ];
    let cleaned: String = html.chars().filter(|c| !zero_width.contains(c)).collect();

    // 2. Remove HTML comments
    let cleaned = RE_COMMENTS.replace_all(&cleaned, "").to_string();

    // 3. Remove <script>, <style>, <noscript> blocks with their content.
    let cleaned = RE_SCRIPT.replace_all(&cleaned, "").to_string();
    let cleaned = RE_STYLE.replace_all(&cleaned, "").to_string();
    let cleaned = RE_NOSCRIPT.replace_all(&cleaned, "").to_string();

    // 4. Remove elements with hidden attribute
    let cleaned = RE_HIDDEN_ATTR.replace_all(&cleaned, "").to_string();

    // 5. Remove elements with aria-hidden="true"
    let cleaned = RE_ARIA_HIDDEN.replace_all(&cleaned, "").to_string();

    // 6. Remove elements with display:none or visibility:hidden in style attribute
    let cleaned = RE_DISPLAY_NONE.replace_all(&cleaned, "").to_string();

    // 7. Remove elements with screen-reader / visually-hidden CSS classes
    let cleaned = RE_SR_CLASSES.replace_all(&cleaned, "").to_string();

    // 8. Surface <time datetime="…"> machine timestamps into the visible
    //    text so absolute publication times survive text/Markdown
    //    extraction (the attribute itself is dropped by both).
    RE_TIME_DATETIME
        .replace_all(&cleaned, "<time>${2} (${1})</time>")
        .to_string()
}

/// Reject HTML that exceeds the response budget to prevent `DoS`.
pub(crate) fn validate_html_safety(
    html: &str,
    max_bytes: usize,
) -> std::result::Result<(), ToolError> {
    // The response-byte gate already bounds anything reaching here;
    // this is the matching upper bound on the decoded HTML string. The
    // former 1 MB cap rejected ordinary modern news pages (1–4 MB of HTML)
    // *before* the readability/markdown extractor could reduce them to
    // clean, length-capped text — defeating the whole point of fetching.
    if html.len() > max_bytes {
        return Err(ToolError::Execution(format!(
            "HTML too large: {} bytes (max {} bytes)",
            html.len(),
            max_bytes,
        )));
    }
    Ok(())
}

/// Run the Mozilla Readability algorithm on the HTML and return clean
/// article HTML. Returns `None` when the result is empty or too short.
fn extract_with_readability(html: &str, url: &str, min_content_length: usize) -> Option<String> {
    use url::Url;

    let parsed_url = Url::parse(url).ok()?;
    let mut cursor = std::io::Cursor::new(html.as_bytes());
    let product = readability::extractor::extract(&mut cursor, &parsed_url).ok()?;

    if product.content.len() < min_content_length {
        return None;
    }
    Some(product.content)
}

/// Enhanced extraction pipeline: pre-clean → Readability → Markdown/Text.
/// Falls back to the legacy selector-based extractor when Readability
/// fails or the result is too short.
pub(crate) fn extract_content_enhanced(
    raw_html: &str,
    url: &str,
    mode: &ExtractMode,
    enable_readability: bool,
    min_content_length: usize,
    max_content_length: usize,
) -> (String, Extractor) {
    let cleaned_html = pre_clean_html(raw_html);

    // Try Readability extraction (if enabled)
    if enable_readability {
        if let Some(article_html) = extract_with_readability(&cleaned_html, url, min_content_length)
        {
            let content = match mode {
                ExtractMode::Markdown => html_to_markdown(&article_html),
                ExtractMode::Text => {
                    let text = strip_tags(&article_html);
                    clean_text(&text)
                }
            };
            let content = truncate_content(content, max_content_length);
            return (content, Extractor::Readability);
        }
    }

    // Fallback: legacy CSS-selector-based extraction. Parse the
    // *cleaned* HTML so the selector path also benefits from script/style
    // removal and `<time>` timestamp surfacing.
    let document = Html::parse_document(&cleaned_html);
    let content = extract_content(&document, mode, url, min_content_length, max_content_length);
    (content, Extractor::Selector)
}

/// Extract main content using priority-ordered selectors, honouring the
/// requested `mode`. In Markdown mode the selected element is converted to
/// Markdown (preserving `<a href>` links and `<time>` timestamps) with
/// relative URLs resolved against `base_url`; the legacy plain-`.text()`
/// path dropped every link and timestamp, which is what made index/listing
/// pages report "original link / publish time not provided".
fn extract_content(
    document: &Html,
    mode: &ExtractMode,
    base_url: &str,
    min_content_length: usize,
    max_content_length: usize,
) -> String {
    // Content selectors in priority order
    let selectors = [
        "article",
        "main",
        ".content",
        ".post-content",
        "#content",
        "body",
    ];

    for selector_str in selectors {
        if let Ok(selector) = Selector::parse(selector_str) {
            if let Some(el) = document.select(&selector).next() {
                let content = render_element(&el, mode, base_url);
                if content.len() > min_content_length {
                    debug!(
                        "Using selector '{}' with {} chars",
                        selector_str,
                        content.len()
                    );
                    return truncate_content(content, max_content_length);
                }
            }
        }
    }

    // Fallback: return whatever we can get from body
    if let Ok(selector) = Selector::parse("body") {
        if let Some(el) = document.select(&selector).next() {
            return truncate_content(render_element(&el, mode, base_url), max_content_length);
        }
    }

    String::new()
}

/// Render a selected element to the requested output format. Markdown mode
/// preserves links/timestamps and resolves relative URLs against
/// `base_url`; Text mode keeps the legacy whitespace-collapsed plain text.
fn render_element(el: &ElementRef<'_>, mode: &ExtractMode, base_url: &str) -> String {
    match mode {
        ExtractMode::Markdown => {
            let raw = el.html();
            let resolved = resolve_relative_urls(&raw, base_url);
            html_to_markdown(&resolved)
        }
        ExtractMode::Text => clean_text(&el.text().collect::<String>()),
    }
}

/// Truncate content to maximum length.
///
/// Takes the `String` by value and hands it back untouched when it fits: this
/// runs on whole fetched pages, so routing the common path through a `&str`
/// helper would copy the entire document to produce an identical result.
pub(crate) fn truncate_content(content: String, max_content_length: usize) -> String {
    if content.chars().count() <= max_content_length {
        return content;
    }
    format!(
        "{}...",
        crate::utils::text_format::truncate_chars(&content, max_content_length)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_text() {
        let text = "  Hello   world  \n\t  test  ";
        let cleaned = clean_text(text);
        assert_eq!(cleaned, "Hello world test");
    }

    #[test]
    fn test_pre_clean_removes_script_and_style() {
        let html = r#"<html><body>
            <script>alert('xss')</script>
            <style>.hide { display: none; }</style>
            <p>Visible content here</p>
        </body></html>"#;
        let cleaned = pre_clean_html(html);
        assert!(
            !cleaned.contains("alert"),
            "Script content should be removed: {}",
            cleaned
        );
        assert!(
            !cleaned.contains(".hide"),
            "Style content should be removed: {}",
            cleaned
        );
        assert!(
            cleaned.contains("Visible content here"),
            "Visible text should remain: {}",
            cleaned
        );
    }

    #[test]
    fn test_pre_clean_removes_hidden_elements() {
        let html = r#"<html><body>
            <div style="display:none">Hidden div</div>
            <div aria-hidden="true">Aria hidden div</div>
            <p>Visible paragraph</p>
        </body></html>"#;
        let cleaned = pre_clean_html(html);
        assert!(
            !cleaned.contains("Hidden div"),
            "display:none should be removed: {}",
            cleaned
        );
        assert!(
            !cleaned.contains("Aria hidden div"),
            "aria-hidden should be removed: {}",
            cleaned
        );
        assert!(
            cleaned.contains("Visible paragraph"),
            "Visible text should remain: {}",
            cleaned
        );
    }

    #[test]
    fn test_pre_clean_strips_zero_width_chars() {
        let html = "<html><body><p>Hello\u{200B}World\u{FEFF}Test</p></body></html>";
        let cleaned = pre_clean_html(html);
        assert!(
            cleaned.contains("HelloWorldTest"),
            "Zero-width chars should be stripped: {}",
            cleaned
        );
    }

    #[test]
    fn test_safety_gate_rejects_oversized_html() {
        // Only truly pathological input (beyond the 10 MB response budget) is
        // rejected; the byte gate already bounds anything that reaches here.
        const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
        let huge = "a".repeat(MAX_RESPONSE_BYTES + 1);
        assert!(validate_html_safety(&huge, MAX_RESPONSE_BYTES).is_err());
    }

    #[test]
    fn test_safety_gate_accepts_large_news_page() {
        // Real news section pages routinely ship 1–4 MB of HTML (e.g. BBC
        // Middle East ≈ 3.6 MB). These must pass the gate so the readability
        // extractor can reduce them to clean text — the old 1 MB cap rejected
        // them outright before extraction.
        const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
        let big = "a".repeat(3_600_000);
        assert!(validate_html_safety(&big, MAX_RESPONSE_BYTES).is_ok());
    }

    #[test]
    fn test_safety_gate_accepts_normal_html() {
        const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
        let normal = "<html><body><p>Hello</p></body></html>";
        assert!(validate_html_safety(normal, MAX_RESPONSE_BYTES).is_ok());
    }

    #[test]
    fn test_readability_extraction_produces_markdown() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test Article</title></head>
        <body>
            <nav><a href="/">Home</a> | <a href="/about">About</a> | <a href="/contact">Contact</a></nav>
            <article>
                <h1>Main Article Title</h1>
                <p>This is the first paragraph of the article with enough content to be recognized by the readability algorithm as meaningful text content that should be extracted and preserved in the output.</p>
                <p>This is the second paragraph providing additional detail about the topic being discussed in this article. It contains several sentences to ensure adequate length for proper extraction.</p>
                <h2>Section Two</h2>
                <ul>
                    <li>First item in the list</li>
                    <li>Second item in the list</li>
                    <li>Third item in the list</li>
                </ul>
                <p>A concluding paragraph that wraps up the discussion and provides final thoughts on the matter at hand. This ensures the article has sufficient content density.</p>
            </article>
            <footer><p>Copyright 2024 | Privacy Policy | Terms of Service</p></footer>
        </body></html>"#;

        let (content, _extractor) = extract_content_enhanced(
            html,
            "https://example.com/article",
            &ExtractMode::Markdown,
            true,
            100,
            10_000,
        );

        // Should produce non-empty content
        assert!(!content.is_empty(), "Content should not be empty");
        // Should contain article text
        assert!(
            content.contains("Main Article Title") || content.contains("first paragraph"),
            "Should contain article content: {}",
            &content[..content.len().min(500)]
        );
    }

    #[test]
    fn test_text_mode_produces_plain_text() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body><article>
            <h1>Title Here</h1>
            <p>This is a paragraph with enough content for readability to extract it properly as meaningful text content in the article body section.</p>
            <p>Another paragraph with sufficient length to ensure the readability algorithm recognizes this as article content worth preserving in output.</p>
        </article></body></html>"#;

        let (content, _) = extract_content_enhanced(
            html,
            "https://example.com",
            &ExtractMode::Text,
            true,
            100,
            10_000,
        );

        // Should contain the actual text
        assert!(
            content.contains("paragraph") || content.contains("Title"),
            "Should contain article text: {}",
            content
        );
    }

    #[test]
    fn test_fallback_to_selector_on_minimal_html() {
        let html = "<html><body><p>Short</p></body></html>";
        let (_, extractor) = extract_content_enhanced(
            html,
            "https://example.com",
            &ExtractMode::Markdown,
            true,
            100,
            10_000,
        );

        assert!(
            matches!(extractor, Extractor::Selector),
            "Expected Selector fallback, got {:?}",
            extractor
        );
    }

    #[test]
    fn test_readability_disabled_uses_selector() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body><article>
            <h1>Title</h1>
            <p>Long enough paragraph for readability to normally extract this content from the article body with sufficient detail and words.</p>
            <p>Another paragraph ensuring adequate length for the readability algorithm to process and recognize as valid content.</p>
        </article></body></html>"#;

        let (_, extractor) = extract_content_enhanced(
            html,
            "https://example.com",
            &ExtractMode::Markdown,
            false,
            100,
            10_000,
        );

        assert!(
            matches!(extractor, Extractor::Selector),
            "Should use Selector when readability disabled, got {:?}",
            extractor
        );
    }

    // ─── Selector fallback: link & timestamp preservation (regression) ──
    //
    // Index/listing pages make Readability fall back to the selector path.
    // The legacy path flattened the element with `.text()`, dropping every
    // `<a href>` URL and `<time>` timestamp — which is why fetched pages
    // reported "original link: not provided / publish time: not provided".

    #[test]
    fn selector_fallback_preserves_and_resolves_links_as_markdown() {
        // Headline links, no single article body → disable Readability to
        // force the selector fallback path deterministically.
        let html = r#"<!DOCTYPE html><html><head><title>News</title></head>
        <body><main>
            <a href="/a/one">First headline describing an important world event today</a>
            <a href="two.html">Second headline with additional descriptive words here</a>
            <a href="https://other.test/x">Third already-absolute headline link with words</a>
        </main></body></html>"#;
        let (content, extractor) = extract_content_enhanced(
            html,
            "https://news.test/world/index.html",
            &ExtractMode::Markdown,
            false,
            100,
            10_000,
        );
        assert!(matches!(extractor, Extractor::Selector));
        // Links survive as Markdown rather than being stripped to bare text.
        assert!(
            content.contains("]("),
            "expected markdown links, got: {content}"
        );
        // Root-relative resolved against the host.
        assert!(
            content.contains("https://news.test/a/one"),
            "root-relative link not resolved: {content}"
        );
        // Path-relative resolved against the base directory.
        assert!(
            content.contains("https://news.test/world/two.html"),
            "path-relative link not resolved: {content}"
        );
        // Already-absolute link left intact.
        assert!(
            content.contains("https://other.test/x"),
            "absolute link altered: {content}"
        );
    }

    #[test]
    fn selector_fallback_text_mode_stays_plain() {
        let html = r#"<html><body><main>
            <a href="/x">A deliberately long headline sentence placed here so the main block clearly clears the minimum content length gate without any markdown</a>
        </main></body></html>"#;
        let (content, _) = extract_content_enhanced(
            html,
            "https://news.test/",
            &ExtractMode::Text,
            false,
            100,
            10_000,
        );
        assert!(
            !content.contains("]("),
            "text mode must stay plain: {content}"
        );
        assert!(
            !content.contains("http"),
            "text mode must not surface URLs: {content}"
        );
        assert!(content.contains("long headline sentence"), "got: {content}");
    }

    #[test]
    fn time_datetime_is_surfaced_into_extracted_content() {
        let html = r#"<html><body><main>
            <a href="/a/1">A sufficiently long headline so the main content block clears the minimum length gate cleanly</a>
            <time datetime="2026-06-20T08:30:00Z">8 hours ago</time>
        </main></body></html>"#;
        let (content, _) = extract_content_enhanced(
            html,
            "https://news.test/",
            &ExtractMode::Markdown,
            false,
            100,
            10_000,
        );
        assert!(
            content.contains("2026-06-20T08:30:00Z"),
            "absolute timestamp should survive extraction: {content}"
        );
    }

    #[test]
    fn pre_clean_surfaces_time_datetime() {
        let html = r#"<time datetime="2026-06-20T08:30:00Z">8 hours ago</time>"#;
        let cleaned = pre_clean_html(html);
        assert!(
            cleaned.contains("8 hours ago"),
            "visible text kept: {cleaned}"
        );
        assert!(
            cleaned.contains("2026-06-20T08:30:00Z"),
            "datetime surfaced into text: {cleaned}"
        );
    }
}
