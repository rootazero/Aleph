//! Markdown renderer component with syntax highlighting.
//!
//! Uses pulldown-cmark for Markdown parsing and syntect for code block highlighting.

use crate::state::typewriter::TypewriterClock;
use crate::views::chat::state::ChatMessage;
use crate::views::chat::timeline;
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
    // Protocol-relative URLs (`//evil.com/x`) contain no colon, so a
    // `split_once(':')` check alone lets them through — yet a browser still
    // navigates them by inheriting the panel's scheme. Reject them up-front
    // so a `[label](//evil.com)` cannot redirect off-origin under `target=_blank`.
    if trimmed.starts_with("//") {
        return "#disallowed-protocol-relative".to_string();
    }
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

/// Extend a cached stable-prefix render with newly-revealed text.
///
/// `revealed_prefix` is the full text revealed so far (`content` truncated
/// to the typewriter's current `revealed` char count) — NOT just the new
/// characters, since [`shared_ui_logic::markdown_stream::safe_freeze_offset`]
/// needs to see any fence/reference-link-def state spanning the cached
/// boundary and the newly-revealed text. Mutates `cached_html` /
/// `cached_offset` in place and returns whether the boundary advanced: the
/// no-progress path is deliberately copy-free (the old signature returned
/// fresh copies, which made every no-advance animation frame an O(prefix)
/// `String` clone).
fn extend_stable_prefix(
    cached_html: &mut String,
    cached_offset: &mut usize,
    revealed_prefix: &str,
) -> bool {
    // A cached offset can outlive the content it was computed against:
    // `set_step_text` replaces a still-streaming bubble's content wholesale
    // (shorter authoritative text can replace a longer streamed preview)
    // without necessarily flipping `is_streaming`, so `cached_offset` may no
    // longer be a valid boundary into `revealed_prefix`. Treat that as "no
    // cache" rather than trusting a stale offset — falls back to full
    // reprocessing of `revealed_prefix` from scratch, per this module's
    // "never wrong in the unsafe direction" contract.
    if revealed_prefix.get(..*cached_offset).is_none() {
        cached_html.clear();
        *cached_offset = 0;
    }
    match shared_ui_logic::markdown_stream::safe_freeze_offset(revealed_prefix, *cached_offset) {
        Some(new_offset) if new_offset > *cached_offset => {
            let delta = &revealed_prefix[*cached_offset..new_offset];
            cached_html.push_str(&render_streaming(delta));
            *cached_offset = new_offset;
            true
        }
        _ => false,
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

/// Map a character-count reveal position to a byte offset, incrementally.
///
/// `prev` is the `(revealed_chars, byte_offset)` pair returned for the same
/// message on the previous frame; the fast path scans only the characters
/// revealed since then. A full rescan happens only when the cursor can't be
/// trusted: a backwards move, or a stored offset that no longer lands on a
/// char boundary of the (wholesale-swapped) content. Returns the honest
/// `(chars_covered, byte_offset)` pair — `chars_covered` is normally
/// `revealed`, but clamps if `revealed` overshoots the content.
fn advance_byte_cursor(content: &str, prev: (usize, usize), revealed: usize) -> (usize, usize) {
    let base = if revealed >= prev.0 && content.is_char_boundary(prev.1) {
        prev
    } else {
        (0, 0)
    };
    let mut chars_done = base.0;
    let mut bytes = base.1;
    for ch in content[bytes..].chars() {
        if chars_done >= revealed {
            break;
        }
        bytes += ch.len_utf8();
        chars_done += 1;
    }
    (chars_done, bytes)
}

/// Per-frame output of the reveal computation — see [`TypewriterRenderer`].
#[derive(Clone, Copy, PartialEq)]
enum Frame {
    /// Full markdown, no animation (history, or the sweep finished).
    Static,
    /// Two-zone live preview: the stable prefix covers `..safe_offset` (byte
    /// index into content), the tail zone shows `safe_offset..revealed_bytes`
    /// plus the cursor.
    Live {
        revealed_bytes: usize,
        safe_offset: usize,
    },
}

/// Assistant-message renderer that paces character reveal at
/// `behavior.typing_speed` (chars/sec) via the shared [`TypewriterClock`], then
/// switches to full Markdown once the sweep catches up.
///
/// The component is MOUNTED ONCE per row and reads its message through the
/// row's `Memo` — its predecessor took owned `content`/`message_id` props and
/// had to be re-invoked by a `{move || message.with(...)}` closure on every
/// streamed token, which tore down and rebuilt the whole bubble subtree (new
/// DOM node, fresh `StoredValue`s, full-content `inner_html` re-parse) per
/// chunk.
///
/// Rendering is TWO-ZONE (codex's stable/tail split mapped to the DOM):
/// - a `display:contents` stable zone whose `inner_html` updates only when
///   the safe-freeze boundary advances (line granularity — see
///   [`extend_stable_prefix`]), and
/// - a `display:contents` tail zone holding the unfrozen remainder + the
///   streaming cursor, updated per animation tick with O(tail) work.
/// `display:contents` keeps both zones in the parent's flow, so the split is
/// layout-identical to the old single-string `inner_html`.
///
/// The reveal cursor counts characters (cps is a char rate) but rendering
/// slices bytes, so [`advance_byte_cursor`] maintains an incremental
/// char→byte mapping — O(newly-revealed chars) per frame instead of the old
/// `chars().take(revealed).collect()` full rescan + allocation.
///
/// Routing:
/// - No cursor and not streaming → a message loaded from history (never streamed
///   live this session): full Markdown, no tick subscription (keeps a long
///   transcript cheap — only live bubbles tick at 30fps).
/// - Reveal caught up + stream finished → prune the cursor, switch to full
///   Markdown, unsubscribe the tick (the finished bubble goes static).
/// - Otherwise → two-zone live preview, advancing on each animation tick.
///   While caught up mid-stream the frame value stops changing, so downstream
///   effects do zero work until the next delta (the predecessor re-rendered
///   the FULL accumulated text with cursor 30x/s in that state).
///
/// When no clock is in context (e.g. storybook) it degrades to a reactive
/// static render.
#[component]
#[must_use]
pub fn TypewriterRenderer(message: Memo<Option<ChatMessage>>) -> impl IntoView {
    let Some(clock) = use_context::<TypewriterClock>() else {
        return view! {
            <div
                class="markdown-body text-sm leading-relaxed streaming-content"
                inner_html=move || {
                    message.with(|m| {
                        m.as_ref()
                            .map(|m| {
                                if m.is_streaming {
                                    render_streaming_with_cursor(&m.content)
                                } else {
                                    render_markdown(&m.content)
                                }
                            })
                            .unwrap_or_default()
                    })
                }
            />
        }
        .into_any();
    };

    // Per-content-change snapshot of the fields pacing math needs. Recomputed
    // once per chunk (Memo), NOT per frame — `chars().count()` is O(content)
    // and the predecessor paid it at every re-invocation.
    let snap = Memo::new(move |_| {
        message.with(|m| {
            m.as_ref()
                .map(|m| (timeline::reveal_key(m), m.content.chars().count(), m.is_streaming))
        })
    });

    // Incremental char→byte cursor for the reveal position: the previous
    // frame's `(revealed_chars, byte_offset)`, plus the message identity it
    // was computed against (`begin_step` renames + bumps iteration, so the
    // reveal key changes across steps and forces a re-anchor).
    let byte_cursor = StoredValue::new_local((0usize, 0usize));
    let byte_cursor_id = StoredValue::new_local(String::new());

    // The one per-tick computation: advance the reveal cursor, extend the
    // frozen prefix, return what the zones need. `Frame::Static` carries no
    // numbers, so once a bubble goes static its frame value stops changing
    // and every downstream memo goes quiet.
    let frame = Memo::new(move |_| {
        let (id, total_chars, is_streaming) = snap.get()?;
        // History fast path: no live cursor, not streaming → static markdown
        // with NO tick subscription (`tick` is only read below this point, so
        // the subscription set of a static frame is empty).
        if !is_streaming && !clock.has_reveal(&id) {
            return Some((id, Frame::Static));
        }
        clock.tick.track();
        // No monotonic clock → cannot pace; reveal everything that arrived
        // (degrade to instant, never hide text).
        let revealed = match now_ms() {
            Some(now) => clock.advance_for(
                &id,
                total_chars,
                now,
                clock.cps.get_untracked(),
                clock.instant.get_untracked(),
            ),
            None => total_chars,
        };
        if revealed >= total_chars && !is_streaming {
            // Stream finished AND fully revealed → static final Markdown.
            // Prune the cursor so this bubble never ticks again.
            clock.finish(&id);
            return Some((id, Frame::Static));
        }
        let (revealed_bytes, safe_offset) = message.with_untracked(|m| {
            let content = &m.as_ref()?.content;
            if byte_cursor_id.get_value() != id {
                byte_cursor_id.set_value(id.clone());
                byte_cursor.set_value((0, 0));
            }
            let cursor = advance_byte_cursor(content, byte_cursor.get_value(), revealed);
            byte_cursor.set_value(cursor);
            let revealed_bytes = cursor.1;
            if !is_streaming {
                // Reveal hasn't caught up but the stream already ended —
                // finalize may have swapped `content` wholesale, so a cached
                // prefix could describe text that's no longer there. Drop it
                // and render this frame uncached; the lag-floor window bounds
                // how long this lasts.
                clock.clear_stable_prefix(&id);
                return Some((revealed_bytes, 0));
            }
            let revealed_prefix = content.get(..revealed_bytes)?;
            let safe_offset = clock.update_stable_prefix(&id, |html, off| {
                extend_stable_prefix(html, off, revealed_prefix);
                *off
            });
            Some((revealed_bytes, safe_offset))
        })?;
        Some((
            id,
            Frame::Live {
                revealed_bytes,
                safe_offset,
            },
        ))
    });

    // Gate for the Static↔Live branch switch: the branch closure rebuilds
    // the subtree only when this flips (once per message lifetime), not on
    // every frame.
    let is_static = Memo::new(move |_| !matches!(frame.get(), Some((_, Frame::Live { .. }))));

    // Full markdown for the Static branch, recomputed only when the message
    // changes while the branch is mounted (a finished bubble's content is
    // final, so this settles immediately).
    let full_html =
        move || message.with(|m| m.as_ref().map(|m| render_markdown(&m.content)).unwrap_or_default());

    // Stable zone: the DOM write is gated on `(id, safe_offset)`, so the
    // frozen prefix's HTML is re-read (and re-parsed by the browser) only
    // when the freeze boundary actually advances — line granularity, not
    // frame granularity.
    let stable_key = Memo::new(move |_| match frame.get() {
        Some((id, Frame::Live { safe_offset, .. })) => Some((id, safe_offset)),
        _ => None,
    });
    let stable_html = move || {
        stable_key.with(|k| {
            k.as_ref()
                .and_then(|(id, _)| clock.stable_prefix_for(id))
                .map(|(html, _)| html)
                .unwrap_or_default()
        })
    };

    // Tail zone: unfrozen remainder + cursor. Re-runs per frame while the
    // frame value keeps changing (active sweep), with O(tail) work; when
    // caught up mid-stream the frame value is stable and this stays quiet.
    let tail_html = move || {
        let (revealed_bytes, safe_offset) = match frame.get() {
            Some((_, Frame::Live {
                revealed_bytes,
                safe_offset,
            })) => (revealed_bytes, safe_offset),
            _ => return String::new(),
        };
        message.with_untracked(|m| {
            m.as_ref()
                .map(|m| {
                    let content = &m.content;
                    let Some(revealed) = content.get(..revealed_bytes) else {
                        return render_streaming_with_cursor(content);
                    };
                    if revealed.is_empty() {
                        // Match the old empty-content case: a non-breaking
                        // space keeps the bubble one text line tall instead
                        // of collapsing or showing a cursor on its own line.
                        return "&nbsp;".to_string();
                    }
                    let tail = revealed.get(safe_offset..).unwrap_or(revealed);
                    format!("{}{}", render_streaming(tail), STREAMING_CURSOR_HTML)
                })
                .unwrap_or_default()
        })
    };

    // Click-to-skip: a mid-sweep click jumps the reveal to the full arrived
    // text (`TypewriterClock::skip`, which sets the cursor rather than
    // dropping it — dropping would re-anchor a still-streaming bubble at
    // zero). The pointer affordance shows only while a live frame exists.
    let sweeping = move || matches!(frame.get(), Some((_, Frame::Live { .. })));
    let on_skip = move |_| {
        if let Some((id, total_chars, _)) = snap.get_untracked() {
            if clock.has_reveal(&id) {
                clock.skip(&id, total_chars);
            }
        }
    };

    view! {
        {move || if is_static.get() {
            view! { <div class="markdown-body text-sm leading-relaxed" inner_html=full_html /> }
                .into_any()
        } else {
            view! {
                <div
                    class="markdown-body text-sm leading-relaxed streaming-content"
                    class:cursor-pointer=move || sweeping()
                    on:click=on_skip
                >
                    <div class="contents" inner_html=stable_html></div>
                    <div class="contents" inner_html=tail_html></div>
                </div>
            }
                .into_any()
        }}
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
    use super::{
        advance_byte_cursor, extend_stable_prefix, render_markdown, render_streaming,
        sanitize_link_url,
    };

    #[test]
    fn byte_cursor_advances_incrementally_over_ascii() {
        let content = "hello world";
        let c1 = advance_byte_cursor(content, (0, 0), 5);
        assert_eq!(c1, (5, 5));
        let c2 = advance_byte_cursor(content, c1, 11);
        assert_eq!(c2, (11, 11));
    }

    #[test]
    fn byte_cursor_counts_chars_not_bytes_for_cjk() {
        // 3 bytes per CJK char: 4 chars == 12 bytes.
        let content = "你好世界 done";
        let c1 = advance_byte_cursor(content, (0, 0), 4);
        assert_eq!(c1, (4, 12));
        let c2 = advance_byte_cursor(content, c1, 9);
        assert_eq!(c2, (9, content.len()));
        assert!(content.is_char_boundary(c2.1));
    }

    #[test]
    fn byte_cursor_rescans_when_the_reveal_moves_backwards() {
        let content = "abcdef";
        let forward = advance_byte_cursor(content, (0, 0), 5);
        let back = advance_byte_cursor(content, forward, 2);
        assert_eq!(back, (2, 2));
    }

    #[test]
    fn byte_cursor_rescans_when_the_stored_offset_is_not_a_boundary() {
        // Wholesale content swap: the old byte offset lands mid-char in the
        // new content — must rescan from zero rather than panic.
        let content = "你好";
        let c = advance_byte_cursor(content, (5, 1), 1);
        assert_eq!(c, (1, 3));
    }

    #[test]
    fn byte_cursor_clamps_when_revealed_overshoots_the_content() {
        let content = "abc";
        let c = advance_byte_cursor(content, (0, 0), 10);
        assert_eq!(c, (3, 3));
    }

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

    #[test]
    fn sanitize_link_rejects_dangerous_schemes() {
        // `javascript:` is the obvious one — click-to-exec under the
        // panel origin.
        assert!(sanitize_link_url("javascript:alert(1)").starts_with("#disallowed-"));
        // `data:` can carry an inline HTML body that XSS-pivots on click.
        assert!(sanitize_link_url("data:text/html,<script>alert(1)</script>")
            .starts_with("#disallowed-"));
        // Protocol-relative URLs contain no colon, so a naïve
        // `split_once(':')` lets them through — yet the browser still
        // navigates them, inheriting the panel's scheme. Reject up-front.
        assert!(sanitize_link_url("//evil.example/path").starts_with("#disallowed-"));
    }

    #[test]
    fn sanitize_link_keeps_allowed_schemes() {
        assert_eq!(sanitize_link_url("https://example.com"), "https://example.com");
        assert_eq!(sanitize_link_url("http://example.com"), "http://example.com");
        assert_eq!(sanitize_link_url("mailto:a@b"), "mailto:a@b");
        // Relative links must survive — they only resolve once the panel
        // routes them.
        assert_eq!(sanitize_link_url("/docs/page"), "/docs/page");
    }

    #[test]
    fn extend_stable_prefix_appends_only_the_new_safe_delta() {
        let mut html = String::new();
        let mut offset = 0;
        assert!(extend_stable_prefix(&mut html, &mut offset, "line one\nline two\n"));
        assert_eq!(offset, "line one\nline two\n".len());
        assert!(html.contains("line one"));
        assert!(html.contains("line two"));

        // Simulate the next tick: more text arrived, cache reused.
        let before = html.clone();
        assert!(extend_stable_prefix(
            &mut html,
            &mut offset,
            "line one\nline two\nline three\n"
        ));
        assert_eq!(offset, "line one\nline two\nline three\n".len());
        assert!(html.starts_with(&before), "must extend, not rebuild");
        assert!(html.contains("line three"));
    }

    #[test]
    fn extend_stable_prefix_no_ops_when_no_safe_progress_exists() {
        // `cached_offset` must be a value `safe_freeze_offset` could actually
        // have returned — i.e. a complete-line boundary (here: 0, "nothing
        // cached yet"). An unclosed fence with no other complete line makes
        // no further progress safe, so the cache must pass through unchanged.
        let mut html = "<cached>".to_string();
        let mut offset = 0;
        assert!(!extend_stable_prefix(&mut html, &mut offset, "```rust\n"));
        assert_eq!(html, "<cached>");
        assert_eq!(offset, 0);
    }

    #[test]
    fn extend_stable_prefix_falls_back_safely_when_cached_offset_outlives_shrunk_content() {
        // set_step_text can replace a still-streaming bubble's content with
        // shorter authoritative text without flipping is_streaming — the
        // stale cached_html must be discarded, not reused, once the cached
        // offset no longer fits the new content.
        let mut html = "<p>this describes text that no longer exists</p>".to_string();
        let mut offset = 60;
        extend_stable_prefix(&mut html, &mut offset, "short\n");
        assert_eq!(offset, "short\n".len());
        assert!(
            !html.contains("no longer exists"),
            "must discard stale cached html, not reuse it"
        );
    }

    #[test]
    fn extend_stable_prefix_does_not_panic_when_cached_offset_exceeds_shrunk_len() {
        // Regression: must not panic (byte-index-out-of-bounds) when a
        // wholesale content replacement leaves cached_offset pointing past
        // the end of the new, shorter text.
        let mut html = "<cached>".to_string();
        let mut offset = 100;
        extend_stable_prefix(&mut html, &mut offset, "ab\n");
        assert!(offset <= "ab\n".len());
    }
}
