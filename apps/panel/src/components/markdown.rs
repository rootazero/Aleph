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
                        lang_str
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                let highlighted = highlight_code(&code_content, &code_lang);
                let lang_label = if code_lang.is_empty() {
                    "code"
                } else {
                    &code_lang
                };

                html_output.push_str(&format!(
                    r#"<div class="code-block-wrapper relative group my-3"><div class="flex items-center justify-between px-3 py-1.5 bg-surface-sunken/50 rounded-t-lg border border-b-0 border-border text-xs text-text-tertiary"><span>{lang_label}</span><button class="copy-btn opacity-0 group-hover:opacity-100 transition-opacity px-2 py-0.5 rounded text-text-secondary hover:text-text-primary hover:bg-surface-raised" onclick="navigator.clipboard.writeText(this.closest('.code-block-wrapper').querySelector('code').textContent)">Copy</button></div><pre class="rounded-b-lg border border-border bg-surface-sunken overflow-x-auto p-3 text-sm leading-relaxed"><code>{highlighted}</code></pre></div>"#,
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

    let theme = ts
        .themes
        .get("base16-ocean.dark")
        .unwrap_or_else(|| ts.themes.values().next().expect("no themes available"));

    highlighted_html_for_string(code, ss, syntax, theme).unwrap_or_else(|_| html_escape(code))
}

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// A Leptos component that renders Markdown content with syntax-highlighted code blocks.
#[component]
pub fn MarkdownRenderer(content: String) -> impl IntoView {
    let html = render_markdown(&content);

    view! {
        <div class="markdown-body text-sm leading-relaxed" inner_html=html />
    }
}
