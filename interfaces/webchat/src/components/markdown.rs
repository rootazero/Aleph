//! Markdown renderer component with syntax highlighting.
//!
//! Uses pulldown-cmark for Markdown parsing and syntect for code block highlighting.

use crate::state::typewriter::TypewriterClock;
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

    let parser = Parser::new_ext(content, options).map(sanitize_link_event);

    let mut html_output = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_content = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_content.clear();
                code_lang.clear();
                if let CodeBlockKind::Fenced(lang) = kind {
                    let lang_str = lang.as_ref().trim();
                    // Take only the first word (ignore metadata after space)
                    if let Some(first) = lang_str.split_whitespace().next() {
                        code_lang.push_str(first);
                    }
                }
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
                // Render non-code events via pulldown-cmark's HTML renderer.
                // Escape raw HTML/inline HTML so assistant markdown cannot inject
                // scripts or event handlers into the page.
                match other {
                    Event::Html(text) | Event::InlineHtml(text) => {
                        html_output.push_str(&html_escape(text.as_ref()));
                    }
                    _ => {
                        pulldown_cmark::html::push_html(&mut html_output, std::iter::once(other));
                    }
                }
            }
        }
    }

    html_output
}

/// Streaming cursor markup — kept inline after streamed text so it never
/// creates a second line when the bubble is empty.
const STREAMING_CURSOR_HTML: &str = r#"<span class="inline-block w-[3px] h-4 rounded-full bg-gradient-to-b from-primary to-primary/40 animate-pulse ml-0.5 align-text-bottom"></span>"#;

/// Render streaming content plus an inline cursor. When content is empty,
/// emit a non-breaking space so the bubble keeps a single text line of
/// height instead of collapsing or showing a cursor on its own line.
fn render_streaming_with_cursor(content: &str) -> String {
    if content.is_empty() {
        "&nbsp;".to_string()
    } else {
        format!("{}{}", render_streaming(content), STREAMING_CURSOR_HTML)
    }
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
    let Some(theme) = ts
        .themes
        .get(theme_name)
        .or_else(|| ts.themes.values().next())
    else {
        return html_escape(code);
    };

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

/// Allow only a small set of link schemes. Reject `javascript:` and other
/// pseudo-URL schemes to prevent XSS when the rendered HTML is assigned to
/// innerHTML.
fn sanitize_link_url(url: &str) -> String {
    let trimmed = url.trim();
    if let Some((scheme, _)) = trimmed.split_once(':') {
        let scheme = scheme.to_lowercase();
        if scheme == "http" || scheme == "https" || scheme == "mailto" {
            return trimmed.to_string();
        }
        // Disallowed scheme: render as a no-op anchor instead.
        return format!("#disallowed-{}", scheme);
    }
    trimmed.to_string()
}

/// Rewrite link and image destination URLs to block dangerous URI schemes
/// before the rendered HTML is fed into innerHTML.
fn sanitize_link_event(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe_url = sanitize_link_url(&dest_url);
            Event::Start(Tag::Link {
                link_type,
                dest_url: safe_url.into(),
                title,
                id,
            })
        }
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe_url = sanitize_link_url(&dest_url);
            Event::Start(Tag::Image {
                link_type,
                dest_url: safe_url.into(),
                title,
                id,
            })
        }
        event => event,
    }
}

/// Lightweight streaming renderer — escapes HTML, tracks code fences, no Markdown parse.
///
/// Much cheaper than full Markdown: O(n) string scan with HTML escape only.
/// Used during streaming; replaced by full `MarkdownRenderer` on completion.
fn render_streaming(content: &str) -> String {
    let mut html = String::with_capacity(content.len() * 2);
    let mut in_fence = false;

    for line in content.split('\n') {
        if line.starts_with("```") {
            if in_fence {
                // Close fence
                html.push_str("</code></pre></div>");
                in_fence = false;
            } else {
                // Open fence
                let fence_lang = line.trim_start_matches('`').trim().to_string();
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
#[must_use]
pub fn StreamingRenderer(content: String) -> impl IntoView {
    let html = render_streaming_with_cursor(&content);

    view! {
        <div class="markdown-body text-sm leading-relaxed streaming-content" inner_html=html />
    }
}

/// Monotonic clock in milliseconds (page-load relative), or `None` when
/// `performance` is unavailable. The renderer treats `None` as "no usable clock"
/// and reveals all arrived text at once (degrade to instant, never hide text)
/// rather than stalling the sweep.
fn now_ms() -> Option<f64> {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
}

/// Assistant-message renderer that paces character reveal at
/// `behavior.typing_speed` (chars/sec) via the shared [`TypewriterClock`], then
/// switches to full Markdown once the sweep catches up.
///
/// Unlike a stream-gated renderer, the reveal is **decoupled from
/// `is_streaming`**: it keeps advancing after the backend finishes until it has
/// revealed the whole final text, so a response that generates faster than `cps`
/// still animates instead of dumping on completion. The per-message cursor lives
/// in the clock keyed on `message_id`, so it survives the per-token remount of a
/// streaming bubble (the keyed `<For>` recreates the bubble on every delta) and
/// the sweep stays continuous.
///
/// Routing:
/// - No cursor and not streaming → a message loaded from history (never streamed
///   live this session): render full Markdown immediately, no tick subscription
///   (keeps a long transcript cheap — only the one live bubble ticks at 30fps).
/// - Reveal caught up + stream finished → prune the cursor and render full
///   Markdown, then stop ticking (the finished bubble goes static).
/// - Otherwise → paced streaming preview, advancing on each animation tick.
///
/// When no clock is in context (e.g. storybook) it degrades to a static render.
#[component]
#[must_use]
pub fn TypewriterRenderer(
    content: String,
    message_id: String,
    is_streaming: bool,
) -> impl IntoView {
    let Some(clock) = use_context::<TypewriterClock>() else {
        return if is_streaming {
            view! { <StreamingRenderer content=content /> }.into_any()
        } else {
            view! { <MarkdownRenderer content=content /> }.into_any()
        };
    };

    // History: a completed message with no live reveal cursor was never streamed
    // this session — show it in full at once, with no reactive tick dependency.
    if !is_streaming && !clock.has_reveal(&message_id) {
        return view! { <MarkdownRenderer content=content /> }.into_any();
    }

    // Hold content/id in StoredValues so the per-tick closure borrows them
    // instead of cloning the (potentially large) accumulated text 30×/sec.
    let content = StoredValue::new(content);
    let total = content.with_value(|c| c.chars().count());
    let message_id = StoredValue::new(message_id);

    let html = move || {
        // No monotonic clock → cannot pace; reveal everything arrived so far.
        let Some(now) = now_ms() else {
            return content.with_value(|c| {
                if is_streaming {
                    render_streaming_with_cursor(c)
                } else {
                    render_markdown(c)
                }
            });
        };
        // Read pacing params untracked: the animation is driven by `tick` (which
        // re-reads them fresh every ~33ms, so a live speed-slider change still
        // lands within a frame), while a finished/history bubble takes no cps
        // subscription and so never re-runs — its pruned cursor can't be
        // resurrected into a replayed sweep.
        let revealed = clock.advance_for(
            &message_id.get_value(),
            total,
            now,
            clock.cps.get_untracked(),
            clock.instant.get_untracked(),
        );
        if revealed >= total {
            if is_streaming {
                // Caught up to the content that has arrived so far. Keep ticking
                // (heartbeat, see `advance_reveal`) so the next delta paces
                // smoothly; show everything available with the streaming cursor.
                clock.tick.track();
                content.with_value(|c| render_streaming_with_cursor(c))
            } else {
                // Stream finished AND fully revealed → static final Markdown.
                // Prune the cursor and read no tick, so the bubble stops
                // re-rendering.
                clock.finish(&message_id.get_value());
                content.with_value(|c| render_markdown(c))
            }
        } else {
            // Still sweeping — advance on each ~30fps animation tick.
            clock.tick.track();
            content.with_value(|c| {
                let shown: String = c.chars().take(revealed).collect();
                render_streaming_with_cursor(&shown)
            })
        }
    };

    // Click-to-skip: a mid-sweep click jumps the reveal to the full arrived
    // text (`TypewriterClock::skip`, which sets the cursor rather than
    // dropping it — dropping would re-anchor a still-streaming bubble at
    // zero). The pointer affordance shows only while a live cursor exists.
    let sweeping = move || {
        clock.tick.track();
        clock.has_reveal(&message_id.get_value())
    };
    let on_skip = move |_| {
        let id = message_id.get_value();
        if clock.has_reveal(&id) {
            clock.skip(&id, total);
        }
    };

    view! {
        <div
            class="markdown-body text-sm leading-relaxed streaming-content"
            class:cursor-pointer=move || sweeping()
            on:click=on_skip
            inner_html=html
        />
    }
    .into_any()
}

/// A Leptos component that renders Markdown content with syntax-highlighted code blocks.
#[component]
#[must_use]
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
    // render_markdown's lang_label escape is NOT tested here (web_sys constraint);
    // it shares the same html_escape() call as render_streaming and is
    // structurally identical.

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
