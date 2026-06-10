//! Markdown renderer component with syntax highlighting.
//!
//! Uses pulldown-cmark for Markdown parsing and syntect for code block highlighting.

use leptos::prelude::*;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::sync::LazyLock;
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Render a Markdown string to HTML with syntax-highlighted code blocks.
fn render_markdown(content: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(content, options);

    let mut html_output = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_content = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_content.clear();
                code_lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        let lang_str = lang.as_ref().trim();
                        // Take only the first word (ignore metadata after space)
                        lang_str.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                let highlighted = highlight_code(&code_content, &code_lang);
                // Escape the fence info-string: it is raw user/remote content
                // interpolated into inner_html below, so an info-string with no
                // whitespace (e.g. `<script>…`) would otherwise inject markup.
                let lang_label = if code_lang.is_empty() {
                    "code".to_string()
                } else {
                    html_escape(&code_lang)
                };

                html_output.push_str(&format!(
                    r#"<div class="code-block-wrapper"><div class="code-block-header"><span>{lang_label}</span><button class="copy-btn" onclick="navigator.clipboard.writeText(this.closest('.code-block-wrapper').querySelector('code').textContent);var b=this;if(b._t)clearTimeout(b._t);b.textContent='Copied!';b.classList.add('copied');b._t=setTimeout(function(){{b.textContent='Copy';b.classList.remove('copied')}},1500)">Copy</button></div><pre><code>{highlighted}</code></pre></div>"#,
                ));
            }
            Event::Text(text) if in_code_block => {
                code_content.push_str(text.as_ref());
            }
            other => {
                // Render non-code events via pulldown-cmark's HTML renderer
                pulldown_cmark::html::push_html(&mut html_output, std::iter::once(other));
            }
        }
    }

    html_output
}

/// Highlight code using syntect. Falls back to HTML-escaped plain text on failure.
fn highlight_code(code: &str, lang: &str) -> String {
    if lang.is_empty() {
        return html_escape(code);
    }

    let ss = &*SYNTAX_SET;
    let ts = &*THEME_SET;

    let syntax = ss
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    // Match the syntax foreground to the active UI mode: a light theme in light
    // mode keeps code blocks airy (the dark theme's heavy black slab clashed
    // with the light palette). The block's background comes from our own
    // `surface-sunken` token, so syntect's hardcoded background is stripped.
    let theme_name = if is_dark_mode() {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let theme = ts
        .themes
        .get(theme_name)
        .unwrap_or_else(|| ts.themes.values().next().expect("no themes available"));

    match highlighted_html_for_string(code, ss, syntax, theme) {
        Ok(html) => strip_syntect_background(html),
        Err(_) => html_escape(code),
    }
}

/// Whether the UI is in dark mode — drives the syntax theme choice.
///
/// Mirrors `appearance::root()`: explicit `dark`/`light` class on the document
/// element wins; with neither (System mode) we follow the OS preference.
fn is_dark_mode() -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };
    if let Some(el) = win.document().and_then(|d| d.document_element()) {
        let cls = el.class_list();
        if cls.contains("dark") {
            return true;
        }
        if cls.contains("light") {
            return false;
        }
    }
    win.match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(false)
}

/// Strip syntect's wrapping `<pre style="background-color:…">…</pre>`, leaving
/// only the highlighted spans. The surrounding `surface-sunken` block then
/// supplies a theme-adaptive background instead of syntect's hardcoded slab.
fn strip_syntect_background(html: String) -> String {
    let after_open = html
        .find('>')
        .map(|i| &html[i + 1..])
        .unwrap_or(html.as_str());
    let inner = after_open
        .trim_end_matches('\n')
        .strip_suffix("</pre>")
        .unwrap_or(after_open);
    inner.trim_matches('\n').to_string()
}

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Lightweight streaming renderer — escapes HTML, tracks code fences, no Markdown parse.
///
/// Much cheaper than full Markdown: O(n) string scan with HTML escape only.
/// Used during streaming; replaced by full MarkdownRenderer on completion.
fn render_streaming(content: &str) -> String {
    let mut html = String::with_capacity(content.len() * 2);
    let mut in_fence = false;
    let mut fence_lang: String;

    for line in content.split('\n') {
        if line.starts_with("```") {
            if in_fence {
                // Close fence
                html.push_str("</code></pre></div>");
                in_fence = false;
            } else {
                // Open fence
                fence_lang = line.trim_start_matches('`').trim().to_string();
                // Escape: the fence info-string is raw content interpolated
                // into inner_html below (see render_markdown for the same fix).
                let lang_label = if fence_lang.is_empty() {
                    "code".to_string()
                } else {
                    html_escape(&fence_lang)
                };
                html.push_str(&format!(
                    r#"<div class="code-block-wrapper"><div class="code-block-header"><span>{lang_label}</span></div><pre><code>"#,
                ));
                in_fence = true;
            }
        } else if in_fence {
            html.push_str(&html_escape(line));
            html.push('\n');
        } else {
            // Plain text: escape and convert newlines
            html.push_str(&html_escape(line));
            html.push_str("<br>");
        }
    }

    // If still in fence (incomplete), close it
    if in_fence {
        html.push_str("</code></pre></div>");
    }

    html
}

/// A Leptos component for streaming content — lightweight rendering without full Markdown parse.
///
/// Tracks code fences and escapes HTML, but does not process Markdown syntax.
/// Switches to MarkdownRenderer on completion for full formatting.
#[component]
pub fn StreamingRenderer(content: String) -> impl IntoView {
    let html = render_streaming(&content);

    view! {
        <div class="markdown-body text-sm leading-relaxed streaming-content" inner_html=html />
    }
}

/// A Leptos component that renders Markdown content with syntax-highlighted code blocks.
#[component]
pub fn MarkdownRenderer(content: String) -> impl IntoView {
    let html = render_markdown(&content);

    view! {
        <div class="markdown-body text-sm leading-relaxed" inner_html=html />
    }
}

#[cfg(test)]
mod tests {
    use super::{render_markdown, render_streaming};

    // ⚠️ Host-test safety: render_markdown with a *language-tagged* fence
    // calls is_dark_mode() → web_sys::window(), which panics off-wasm.
    // Markdown-side tests therefore use bare ``` fences only (the empty
    // lang takes highlight_code's early escape path); the info-string
    // escape regression is covered via render_streaming, which never
    // touches web_sys.

    #[test]
    fn markdown_code_block_emits_semantic_classes() {
        let html = render_markdown("```\nlet x = 1;\n```");
        assert!(html.contains(r#"<div class="code-block-wrapper">"#));
        assert!(html.contains(r#"<div class="code-block-header">"#));
        assert!(html.contains(r#"<button class="copy-btn""#));
        assert!(html.contains("<pre><code>"));
        // legacy inline utility soup must be gone
        assert!(!html.contains("bg-surface-sunken"));
    }

    #[test]
    fn streaming_code_block_matches_semantic_classes() {
        let html = render_streaming("```rust\nlet x = 1;\n");
        assert!(html.contains(r#"<div class="code-block-wrapper">"#));
        assert!(html.contains(r#"<div class="code-block-header">"#));
        // streaming variant has no copy button
        assert!(!html.contains("copy-btn"));
        // unclosed fence is auto-closed
        assert!(html.ends_with("</code></pre></div>"));
    }

    #[test]
    fn streaming_escapes_fence_info_string() {
        let html = render_streaming("```<script>alert(1)</script>\ncode\n```");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
