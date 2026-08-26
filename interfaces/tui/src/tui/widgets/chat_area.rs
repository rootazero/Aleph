// Chat area widget: renders the scrollable message list with support for
// user messages, assistant messages (with reasoning, tool blocks, markdown),
// system messages, and streaming cursors.

use std::collections::HashMap;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{AppState, ChatMessage, Focus};
use crate::tui::markdown::{markdown_to_lines, markdown_to_lines_incremental};
use crate::tui::theme::DEFAULT_THEME;

use super::tool_block::render_tool_block;

/// Per-message rendered-line cache, owned by `AppState` across frames (see
/// `render_chat_area`'s caller in `render.rs` for where it's threaded
/// through). Keyed by the message's index in `state.messages` — safe because
/// a cache entry also validates against the message's own variant kind and
/// content length before being trusted (see `build_all_lines_cached`); a
/// coincidental `(kind, len)` match at a shifted index is the only failure
/// mode, and it self-heals the next frame once content actually diverges.
#[derive(Debug, Default)]
pub struct LineCache {
    entries: HashMap<usize, CachedEntry>,
    /// Fine-grained incremental cache for the ONE currently-streaming
    /// message's markdown conversion (see `markdown_to_lines_incremental`).
    /// Distinct from `entries` above: that whole-message cache deliberately
    /// never caches a streaming message (its content grows every tick), so
    /// this is the only cache the streaming message gets, and it caches at
    /// the safe-prefix-offset granularity rather than the whole message.
    /// Carries its own `width` (mirroring `CachedEntry::width` below) so a
    /// mid-stream resize invalidates it instead of serving prefix lines
    /// wrapped for the old pane width.
    streaming_markdown_cache: Option<(usize, u16, Vec<Line<'static>>)>,
    /// The message index this `streaming_markdown_cache` belongs to. Reset
    /// (along with the cache above) whenever the streaming message changes
    /// — e.g. a new turn starts streaming — so the new message doesn't
    /// inherit a stale safe-offset from the previous one.
    streaming_message_idx: Option<usize>,
}

#[derive(Debug)]
struct CachedEntry {
    kind: MessageKind,
    content_len: usize,
    width: u16,
    lines: Vec<Line<'static>>,
}

/// Cheap discriminant for `ChatMessage`, used only to invalidate the cache
/// safely across `messages.insert(at, ...)` (peer messages can be inserted
/// before the streaming tail, shifting its index — see
/// `app/events.rs::StreamEvent::...` peer-message handling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    User,
    Assistant,
    System,
}

fn message_kind_and_len(message: &ChatMessage) -> (MessageKind, usize) {
    match message {
        ChatMessage::User { content, .. } => (MessageKind::User, content.len()),
        ChatMessage::Assistant { content, .. } => (MessageKind::Assistant, content.len()),
        ChatMessage::System { content } => (MessageKind::System, content.len()),
    }
}

/// Render the chat area with all messages, handling scrolling.
pub fn render_chat_area(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let border_color = match state.focus {
        Focus::Chat => DEFAULT_THEME.border_focused,
        _ => DEFAULT_THEME.border,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(" Chat ");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let content_width = inner.width;
    let visible_height = inner.height as usize;

    // Build all lines from all messages, but reuse cached per-message
    // renders for unchanged messages (see `LineCache`). Idle ticks (no new
    // content, no scroll change) are the common case; without this cache,
    // every 50 ms tick re-parsed every assistant turn's markdown.
    let all_lines = build_all_lines_cached(
        &state.messages,
        state.verbose,
        state.spinner_frame,
        content_width,
        &mut state.chat_line_cache,
    );

    // Calculate the visible window based on scroll state
    let total_lines = all_lines.len();
    let visible_lines = if state.auto_scroll {
        // Show the last visible_height lines
        let start = total_lines.saturating_sub(visible_height);
        all_lines.get(start..).unwrap_or(&[])
    } else {
        // scroll_offset = how many lines from the bottom we've scrolled up.
        // Clamp it to the renderable range so a large offset (Home maps to
        // usize::MAX/2, or held PageUp) can never push the whole window
        // off-screen and blank the chat.
        let max_offset = total_lines.saturating_sub(visible_height);
        let offset = state.scroll_offset.min(max_offset);
        let end = total_lines.saturating_sub(offset);
        let start = end.saturating_sub(visible_height);
        all_lines.get(start..end).unwrap_or(&[])
    };

    let paragraph = Paragraph::new(visible_lines.to_vec()).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Build all rendered lines from the message history.
///
/// Uncached: `render_chat_area` uses [`build_all_lines_cached`] instead. This
/// is kept as the reference implementation the cache is checked against (see
/// `build_all_lines_cached_matches_uncached_output`) and for tests that don't
/// care about caching — hence `#[cfg(test)]` rather than dead-code warnings.
#[cfg(test)]
fn build_all_lines(state: &AppState, width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    for message in &state.messages {
        match message {
            ChatMessage::User { content, timestamp } => {
                render_user_message(content, timestamp, width, &mut lines);
            }
            ChatMessage::Assistant {
                content,
                tools,
                reasoning,
                is_streaming,
            } => {
                render_assistant_message(
                    content,
                    tools,
                    reasoning.as_deref(),
                    *is_streaming,
                    state.verbose,
                    state.spinner_frame,
                    width,
                    &mut lines,
                    None,
                );
            }
            ChatMessage::System { content } => {
                render_system_message(content, width, &mut lines);
            }
        }
        // Add a blank line between messages
        lines.push(Line::default());
    }

    lines
}

/// Cached variant of [`build_all_lines`]. Produces identical output (see
/// `build_all_lines_cached_matches_uncached_output`); the only difference is
/// that unchanged messages skip re-formatting.
///
/// Takes `messages`/`verbose`/`spinner_frame` as separate parameters rather
/// than `&AppState` deliberately: the caller (`render_chat_area`) needs to
/// pass `&state.messages` (shared) alongside `&mut state.chat_line_cache`
/// (exclusive) in the same call. Rust's disjoint-field borrowing allows that
/// when the call site borrows fields directly, but NOT if this function took
/// `state: &AppState` as one opaque parameter — the compiler can't see
/// through that to know only `messages`/`verbose`/`spinner_frame` are read.
fn build_all_lines_cached(
    messages: &[ChatMessage],
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    cache: &mut LineCache,
) -> Vec<Line<'static>> {
    let streaming_idx = messages.iter().position(|m| {
        matches!(
            m,
            ChatMessage::Assistant {
                is_streaming: true,
                ..
            }
        )
    });
    if cache.streaming_message_idx != streaming_idx {
        cache.streaming_markdown_cache = None;
        cache.streaming_message_idx = streaming_idx;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (idx, message) in messages.iter().enumerate() {
        let (kind, content_len) = message_kind_and_len(message);
        let is_streaming_now = matches!(
            message,
            ChatMessage::Assistant {
                is_streaming: true,
                ..
            }
        );
        let hit = cache
            .entries
            .get(&idx)
            .filter(|e| e.kind == kind && e.content_len == content_len && e.width == width);
        let message_lines = if let Some(entry) = hit {
            entry.lines.clone()
        } else {
            let mut buf = Vec::new();
            match message {
                ChatMessage::User { content, timestamp } => {
                    render_user_message(content, timestamp, width, &mut buf);
                }
                ChatMessage::Assistant {
                    content,
                    tools,
                    reasoning,
                    is_streaming,
                } => {
                    render_assistant_message(
                        content,
                        tools,
                        reasoning.as_deref(),
                        *is_streaming,
                        verbose,
                        spinner_frame,
                        width,
                        &mut buf,
                        if *is_streaming {
                            Some(&mut cache.streaming_markdown_cache)
                        } else {
                            None
                        },
                    );
                }
                ChatMessage::System { content } => {
                    render_system_message(content, width, &mut buf);
                }
            }
            // A streaming message's spinner/tool-block content can change
            // every tick without `content_len` changing (e.g. tool status),
            // so don't cache it — it would serve stale tool-block state.
            // Everything else (settled messages) is safe to cache.
            if !is_streaming_now {
                cache.entries.insert(
                    idx,
                    CachedEntry {
                        kind,
                        content_len,
                        width,
                        lines: buf.clone(),
                    },
                );
            } else {
                cache.entries.remove(&idx);
            }
            buf
        };
        lines.extend(message_lines);
        lines.push(Line::default());
    }
    // Drop cache entries for indices beyond the current message count (a
    // conversation switch or `.clear()` shrinks the vec).
    cache.entries.retain(|idx, _| *idx < messages.len());
    lines
}

/// Render a user message with blue prefix bar.
fn render_user_message(
    content: &str,
    timestamp: &chrono::DateTime<chrono::Utc>,
    width: u16,
    lines: &mut Vec<Line<'static>>,
) {
    let prefix_style = Style::default().fg(DEFAULT_THEME.user);
    let time_str = timestamp.format("%H:%M").to_string();

    // Header: ┃ You  12:34
    lines.push(Line::from(vec![
        Span::styled("\u{2503} ", prefix_style),
        Span::styled(
            "You".to_string(),
            Style::default()
                .fg(DEFAULT_THEME.user)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {time_str}"),
            Style::default().fg(DEFAULT_THEME.muted),
        ),
    ]));

    // Content lines with prefix
    let content_width = width.saturating_sub(2); // account for "┃ " prefix
    let md_lines = markdown_to_lines(content, content_width);
    for md_line in md_lines {
        let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
        spans.extend(md_line.spans);
        lines.push(Line::from(spans));
    }
}

/// Render an assistant message with green prefix bar, reasoning, tools, and content.
#[allow(clippy::too_many_arguments)]
fn render_assistant_message(
    content: &str,
    tools: &[crate::tui::app::ToolExecution],
    reasoning: Option<&str>,
    is_streaming: bool,
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    streaming_cache: Option<&mut Option<(usize, u16, Vec<Line<'static>>)>>,
) {
    let prefix_style = Style::default().fg(DEFAULT_THEME.assistant);

    // Header: ┃ Aleph
    lines.push(Line::from(vec![
        Span::styled("\u{2503} ", prefix_style),
        Span::styled(
            "Aleph".to_string(),
            Style::default()
                .fg(DEFAULT_THEME.assistant)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Reasoning (only if verbose mode)
    if verbose {
        if let Some(reasoning_text) = reasoning {
            let reasoning_style = Style::default().fg(DEFAULT_THEME.reasoning);
            let reasoning_prefix = Style::default().fg(DEFAULT_THEME.muted);
            let content_width = width.saturating_sub(4); // account for "┃ ┊ " prefix

            for reason_line in reasoning_text.lines() {
                if reason_line.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("\u{2503} ", prefix_style),
                        Span::styled("\u{250a} ", reasoning_prefix),
                    ]));
                    continue;
                }

                // Simple wrapping for reasoning text
                let wrapped = textwrap::wrap(reason_line, content_width as usize);
                for wrapped_line in wrapped {
                    lines.push(Line::from(vec![
                        Span::styled("\u{2503} ", prefix_style),
                        Span::styled("\u{250a} ", reasoning_prefix),
                        Span::styled(wrapped_line.into_owned(), reasoning_style),
                    ]));
                }
            }

            // Blank line after reasoning
            lines.push(Line::from(vec![Span::styled("\u{2503} ", prefix_style)]));
        }
    }

    // Tool blocks
    let tool_width = width.saturating_sub(2); // account for "┃ " prefix
    for tool in tools {
        let tool_lines = render_tool_block(tool, spinner_frame, tool_width);
        for tool_line in tool_lines {
            let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
            spans.extend(tool_line.spans);
            lines.push(Line::from(spans));
        }
    }

    // Content (markdown rendered)
    if !content.is_empty() {
        let content_width = width.saturating_sub(2);
        let md_lines = match streaming_cache {
            Some(cache) => {
                let (_offset, lines) = markdown_to_lines_incremental(content, content_width, cache);
                lines
            }
            None => markdown_to_lines(content, content_width),
        };
        for md_line in md_lines {
            let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
            spans.extend(md_line.spans);
            lines.push(Line::from(spans));
        }
    }

    // Streaming cursor
    if is_streaming {
        lines.push(Line::from(vec![
            Span::styled("\u{2503} ", prefix_style),
            Span::styled(
                "\u{258d}".to_string(), // ▍
                Style::default().fg(DEFAULT_THEME.assistant),
            ),
        ]));
    }
}

/// Render a system message with yellow text and indentation.
///
/// System content is frequently multi-line (e.g. `/help`, `/usage`, `/replay`
/// output joined with `\n`) and individual lines may be wider than the pane.
/// ratatui does not treat an embedded `\n` inside a `Span` as a row break, and
/// the chat scroll window counts *logical* `Line`s — so emitting the whole
/// message as one `Span` both mis-renders it and desyncs the scroll height from
/// the physical rows, which clips the newest content off-screen. Split on `\n`
/// and wrap each physical line to the content width so every emitted `Line` is
/// `<= width` and the logical-line window matches the rendered rows.
fn render_system_message(content: &str, width: u16, lines: &mut Vec<Line<'static>>) {
    let style = Style::default().fg(DEFAULT_THEME.system);
    let content_width = (width.saturating_sub(2)).max(1) as usize; // account for "  " indent
    for raw_line in content.split('\n') {
        if raw_line.is_empty() {
            lines.push(Line::from(vec![Span::styled("  ", style)]));
            continue;
        }
        for wrapped in textwrap::wrap(raw_line, content_width) {
            lines.push(Line::from(vec![
                Span::styled("  ", style),
                Span::styled(wrapped.into_owned(), style),
            ]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_lines_with_system_message() {
        let state = AppState::new("test".into(), "claude".into());
        let lines = build_all_lines(&state, 80);
        // Should have at least the welcome system message + blank line
        assert!(lines.len() >= 2);
    }

    #[test]
    fn build_lines_with_user_and_assistant() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { content, .. } = state.current_assistant_mut() {
            content.push_str("Hi there!");
        }

        let lines = build_all_lines(&state, 80);
        // Should have lines for: system + blank + user header + user content + blank
        // + assistant header + assistant content + blank
        assert!(lines.len() >= 6);

        // Check that user header contains "You"
        let has_you = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("You"))
        });
        assert!(has_you, "Should contain 'You' header");

        // Check that assistant header contains "Aleph"
        let has_aleph = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("Aleph"))
        });
        assert!(has_aleph, "Should contain 'Aleph' header");
    }

    #[test]
    fn streaming_message_shows_cursor() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();

        let lines = build_all_lines(&state, 80);
        // Should contain the streaming cursor character ▍
        let has_cursor = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains('\u{258d}'))
        });
        assert!(has_cursor, "Streaming message should show cursor");
    }

    #[test]
    fn non_streaming_message_no_cursor() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { is_streaming, .. } = state.current_assistant_mut() {
            *is_streaming = false;
        }

        let lines = build_all_lines(&state, 80);
        let has_cursor = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains('\u{258d}'))
        });
        assert!(!has_cursor, "Non-streaming message should not show cursor");
    }

    #[test]
    fn zero_width_area_does_not_panic() {
        let state = AppState::new("test".into(), "claude".into());
        let lines = build_all_lines(&state, 0);
        // Should not panic, may produce empty or minimal output
        let _ = lines;
    }

    #[test]
    fn system_message_splits_and_wraps() {
        let mut state = AppState::new("test".into(), "claude".into());
        // Multi-line content with a line wider than the pane.
        let long = "x".repeat(60);
        state.add_system_message(format!("line one\n{long}"));

        let mut lines = Vec::new();
        // Render just the system messages via build_all_lines at a narrow width.
        let all = build_all_lines(&state, 20);
        lines.extend(all);

        // No rendered Line may contain an embedded newline (would mis-render and
        // desync the scroll height).
        for line in &lines {
            for span in &line.spans {
                assert!(
                    !span.content.as_ref().contains('\n'),
                    "system message span must not contain an embedded newline"
                );
            }
        }
        // The 60-char line at width 20 must have wrapped to multiple rows.
        let x_rows = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content.as_ref().contains('x')))
            .count();
        assert!(x_rows > 1, "long system line should wrap to > 1 row");
    }

    #[test]
    fn reasoning_shown_only_in_verbose() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { reasoning, .. } = state.current_assistant_mut() {
            *reasoning = Some("thinking...".to_string());
        }

        // Non-verbose: reasoning should not appear
        let lines = build_all_lines(&state, 80);
        let has_thinking = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("thinking"))
        });
        assert!(
            !has_thinking,
            "Reasoning should not show in non-verbose mode"
        );

        // Verbose: reasoning should appear
        state.verbose = true;
        let lines = build_all_lines(&state, 80);
        let has_thinking = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("thinking"))
        });
        assert!(has_thinking, "Reasoning should show in verbose mode");
    }

    #[test]
    fn build_all_lines_reuses_cached_lines_for_unchanged_messages() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant {
            content,
            is_streaming,
            ..
        } = state.current_assistant_mut()
        {
            content.push_str("Hi there!");
            *is_streaming = false;
        }

        let mut cache = LineCache::default();
        let first = build_all_lines_cached(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
        );
        let second = build_all_lines_cached(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
        );
        assert_eq!(first, second);
        // Cache must actually have been populated, not silently bypassed.
        assert!(!cache.entries.is_empty());
    }

    #[test]
    fn build_all_lines_invalidates_on_content_change() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { is_streaming, .. } = state.current_assistant_mut() {
            // Must be non-streaming: `build_all_lines_cached` never caches a
            // streaming message (its spinner/tool content can change every
            // tick without `content_len` changing), so a still-streaming
            // message here would make the first call below a no-op cache
            // insert and the test would pass regardless of whether
            // `content_len` invalidation actually works.
            *is_streaming = false;
        }
        let mut cache = LineCache::default();
        let _first = build_all_lines_cached(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
        );
        assert!(
            !cache.entries.is_empty(),
            "the message must actually be cached before we test invalidation"
        );
        if let ChatMessage::Assistant { content, .. } = state.current_assistant_mut() {
            content.push_str("new text");
        }
        let updated = build_all_lines_cached(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
        );
        let has_new_text = updated.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("new text"))
        });
        assert!(
            has_new_text,
            "changed content must not serve a stale cache entry"
        );
    }

    #[test]
    fn build_all_lines_invalidates_on_width_change() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_system_message("x".repeat(60));
        let mut cache = LineCache::default();
        let wide = build_all_lines_cached(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
        );
        let narrow = build_all_lines_cached(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            20,
            &mut cache,
        );
        assert_ne!(
            wide.len(),
            narrow.len(),
            "resize must reformat, not reuse the wide cache"
        );
    }

    #[test]
    fn build_all_lines_cached_matches_uncached_output() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { content, .. } = state.current_assistant_mut() {
            content.push_str("Hi there!");
        }
        let mut cache = LineCache::default();
        let cached = build_all_lines_cached(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
        );
        let uncached = build_all_lines(&state, 80);
        assert_eq!(cached, uncached, "caching must not change what's rendered");
    }
}
