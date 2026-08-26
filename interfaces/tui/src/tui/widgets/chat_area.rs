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
use crate::tui::markdown::{markdown_to_lines, markdown_to_lines_incremental, StreamLines, StreamPrefix};
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
    /// The `Rc`-shared prefix lines are reused across frames with zero deep
    /// copies; a mid-stream resize invalidates via `StreamPrefix::width`.
    streaming_markdown_cache: Option<StreamPrefix>,
    /// The message index this `streaming_markdown_cache` belongs to. Reset
    /// (along with the cache above) whenever the streaming message changes
    /// — e.g. a new turn starts streaming — so the new message doesn't
    /// inherit a stale safe-offset from the previous one.
    streaming_message_idx: Option<usize>,
}

#[derive(Debug)]
struct CachedEntry {
    kind: MessageKind,
    /// Sampled fingerprint of the message content (see
    /// [`content_fingerprint`]) — a bare length match can serve lines
    /// rendered from *different* content after a same-length replacement
    /// (e.g. a peer message inserted before the tail shifting indices).
    fingerprint: u64,
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

/// O(1) sampled content fingerprint for cache validation: length plus the
/// first and last 32 bytes. A full hash would re-scan every settled message
/// on every frame — the exact cost the cache exists to avoid — while a bare
/// length can't tell a same-length replacement apart from a hit.
fn content_fingerprint(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let bytes = content.as_bytes();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    let edge = 32.min(bytes.len());
    bytes[..edge].hash(&mut h);
    bytes[bytes.len() - edge..].hash(&mut h);
    h.finish()
}

fn message_kind_and_fingerprint(message: &ChatMessage) -> (MessageKind, u64) {
    match message {
        ChatMessage::User { content, .. } => (MessageKind::User, content_fingerprint(content)),
        ChatMessage::Assistant { content, .. } => {
            (MessageKind::Assistant, content_fingerprint(content))
        }
        ChatMessage::System { content } => (MessageKind::System, content_fingerprint(content)),
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

    // Render only the visible window: pass 1 ensures every message's lines
    // exist (settled ones in the per-message cache, the streaming one via
    // its incremental prefix cache) and records line counts; pass 2 clones
    // out just the rows the window intersects. The previous implementation
    // assembled the FULL transcript (`entry.lines.clone()` per message,
    // then `visible_lines.to_vec()` on top) on every frame — O(transcript)
    // deep copies per draw, 20x/s while a spinner runs.
    let (_total_lines, visible) = build_visible_lines(
        &state.messages,
        state.verbose,
        state.spinner_frame,
        content_width,
        &mut state.chat_line_cache,
        state.auto_scroll,
        state.scroll_offset,
        visible_height,
    );

    let paragraph = Paragraph::new(visible).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Build all rendered lines from the message history.
///
/// Uncached: `render_chat_area` uses [`build_visible_lines`] instead. This
/// is kept as the reference implementation the cache/windowing is checked
/// against (see `build_all_lines_cached_matches_uncached_output`) and for
/// tests that don't care about caching — hence `#[cfg(test)]` rather than
/// dead-code warnings.
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

/// Cached full-transcript build for tests: a window tall enough to cover
/// everything (`usize::MAX`) makes [`build_visible_lines`] return the whole
/// transcript, exercising the same cache machinery the production path uses.
#[cfg(test)]
fn build_all_lines_cached(
    messages: &[ChatMessage],
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    cache: &mut LineCache,
) -> Vec<Line<'static>> {
    let (_total, lines) =
        build_visible_lines(messages, verbose, spinner_frame, width, cache, true, 0, usize::MAX);
    lines
}

/// Render only the lines inside the current scroll window.
///
/// Returns `(total_lines, visible_lines)`. Pass 1 materializes every
/// message's rendered lines exactly once per change (settled messages hit
/// the per-message [`LineCache`] entry, the streaming message reuses its
/// `Rc`-shared frozen prefix and re-renders only the unfrozen tail) and
/// records per-message heights; pass 2 walks the cumulative offsets and
/// clones out ONLY the rows the window intersects — per-frame cost is
/// O(messages) pointer arithmetic plus O(visible rows) of cloning, never
/// O(transcript).
///
/// Each message occupies `rendered_lines + 1` rows: the blank separator the
/// old `build_all_lines` pushed after every message is folded into the
/// height so window arithmetic stays exact.
#[allow(clippy::too_many_arguments)]
fn build_visible_lines(
    messages: &[ChatMessage],
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    cache: &mut LineCache,
    auto_scroll: bool,
    scroll_offset: usize,
    visible_height: usize,
) -> (usize, Vec<Line<'static>>) {
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

    // Pass 1: ensure lines exist, record heights (rendered lines + 1 blank
    // separator per message).
    let mut heights: Vec<usize> = Vec::with_capacity(messages.len());
    let mut streaming_head: Vec<Line<'static>> = Vec::new();
    let mut streaming_content: Option<StreamLines> = None;
    for (idx, message) in messages.iter().enumerate() {
        if Some(idx) == streaming_idx {
            // A streaming message's spinner/tool-block content can change
            // every tick without its text changing, so it is never cached in
            // `entries` — it would serve stale tool-block state. Its markdown
            // content still gets the incremental prefix cache.
            cache.entries.remove(&idx);
            if let ChatMessage::Assistant {
                content,
                tools,
                reasoning,
                ..
            } = message
            {
                render_assistant_head(
                    reasoning.as_deref(),
                    tools,
                    verbose,
                    spinner_frame,
                    width,
                    &mut streaming_head,
                );
                if !content.is_empty() {
                    streaming_content = Some(markdown_to_lines_incremental(
                        content,
                        width.saturating_sub(2),
                        &mut cache.streaming_markdown_cache,
                    ));
                }
                let content_rows = streaming_content.as_ref().map_or(0, StreamLines::line_count);
                // +1 streaming cursor, +1 blank separator.
                heights.push(streaming_head.len() + content_rows + 2);
            }
        } else {
            let (kind, fingerprint) = message_kind_and_fingerprint(message);
            let hit = cache
                .entries
                .get(&idx)
                .filter(|e| e.kind == kind && e.fingerprint == fingerprint && e.width == width);
            match hit {
                Some(entry) => heights.push(entry.lines.len() + 1),
                None => {
                    let mut buf = Vec::new();
                    render_settled_message(message, verbose, spinner_frame, width, &mut buf);
                    heights.push(buf.len() + 1);
                    cache.entries.insert(
                        idx,
                        CachedEntry {
                            kind,
                            fingerprint,
                            width,
                            lines: buf,
                        },
                    );
                }
            }
        }
    }
    // Drop cache entries for indices beyond the current message count (a
    // conversation switch or `.clear()` shrinks the vec).
    cache.entries.retain(|idx, _| *idx < messages.len());

    let total_lines: usize = heights.iter().sum();
    let (start, end) = visible_window(total_lines, visible_height, auto_scroll, scroll_offset);

    // Pass 2: clone out only the intersecting rows.
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut pos = 0usize;
    for (idx, _) in messages.iter().enumerate() {
        let height = heights[idx];
        let mstart = pos;
        pos += height;
        if pos <= start || mstart >= end {
            continue;
        }
        let lo = start.saturating_sub(mstart);
        let hi = (end - mstart).min(height);
        // Body rows occupy [0, height-1); the blank separator sits at
        // height-1.
        let body_hi = hi.min(height - 1);
        if lo < body_hi {
            if Some(idx) == streaming_idx {
                copy_streaming_slice(
                    &streaming_head,
                    streaming_content.as_ref(),
                    lo,
                    body_hi,
                    &mut out,
                );
            } else if let Some(entry) = cache.entries.get(&idx) {
                out.extend(entry.lines[lo..body_hi].iter().cloned());
            }
        }
        if hi == height {
            out.push(Line::default());
        }
    }
    (total_lines, out)
}

/// The `[start, end)` row range the viewport shows — the exact arithmetic
/// the old `render_chat_area` did on a fully-materialized `Vec`, preserved
/// verbatim so scroll behavior is unchanged.
fn visible_window(
    total_lines: usize,
    visible_height: usize,
    auto_scroll: bool,
    scroll_offset: usize,
) -> (usize, usize) {
    if auto_scroll {
        (total_lines.saturating_sub(visible_height), total_lines)
    } else {
        // Clamp a large offset (Home maps to usize::MAX/2, or held PageUp)
        // so it can never push the whole window off-screen and blank the
        // chat.
        let max_offset = total_lines.saturating_sub(visible_height);
        let offset = scroll_offset.min(max_offset);
        let end = total_lines.saturating_sub(offset);
        (end.saturating_sub(visible_height), end)
    }
}

/// Copy the `[lo, hi)` slice of the streaming message's rows into `out`,
/// applying the assistant prefix bar to content rows (deferred to here so
/// off-window rows never pay for it). Row layout: head (header + reasoning
/// + tool blocks) | content | cursor.
fn copy_streaming_slice(
    head: &[Line<'static>],
    content: Option<&StreamLines>,
    lo: usize,
    hi: usize,
    out: &mut Vec<Line<'static>>,
) {
    let head_len = head.len();
    let content_len = content.map_or(0, StreamLines::line_count);

    let head_hi = hi.min(head_len);
    if lo < head_hi {
        out.extend(head[lo..head_hi].iter().cloned());
    }

    let c_lo = lo.saturating_sub(head_len).min(content_len);
    let c_hi = hi.saturating_sub(head_len).min(content_len);
    if let Some(sl) = content {
        let prefix_style = Style::default().fg(DEFAULT_THEME.assistant);
        for i in c_lo..c_hi {
            if let Some(line) = sl.get(i) {
                let mut spans = Vec::with_capacity(line.spans.len() + 1);
                spans.push(Span::styled("\u{2503} ", prefix_style));
                spans.extend(line.spans.iter().cloned());
                out.push(Line::from(spans));
            }
        }
    }

    let cursor_idx = head_len + content_len;
    if lo <= cursor_idx && cursor_idx < hi {
        out.push(streaming_cursor_line());
    }
}

/// Render a settled (non-streaming) message into `out`.
fn render_settled_message(
    message: &ChatMessage,
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    out: &mut Vec<Line<'static>>,
) {
    match message {
        ChatMessage::User { content, timestamp } => {
            render_user_message(content, timestamp, width, out);
        }
        ChatMessage::Assistant {
            content,
            tools,
            reasoning,
            ..
        } => {
            render_assistant_head(reasoning.as_deref(), tools, verbose, spinner_frame, width, out);
            if !content.is_empty() {
                let md_lines = markdown_to_lines(content, width.saturating_sub(2));
                push_prefixed_content(out, md_lines);
            }
        }
        ChatMessage::System { content } => {
            render_system_message(content, width, out);
        }
    }
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

/// Render an assistant message with green prefix bar, reasoning, tools, and
/// content. Test-only reference path (see `build_all_lines`): the production
/// path renders settled messages through `render_settled_message` and the
/// streaming message through `render_assistant_head` +
/// `markdown_to_lines_incremental` + `copy_streaming_slice`.
#[cfg(test)]
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
) {
    render_assistant_head(reasoning, tools, verbose, spinner_frame, width, lines);
    if !content.is_empty() {
        let md_lines = markdown_to_lines(content, width.saturating_sub(2));
        push_prefixed_content(lines, md_lines);
    }
    if is_streaming {
        lines.push(streaming_cursor_line());
    }
}

/// Everything above an assistant message's markdown content: the `┃ Aleph`
/// header, verbose reasoning, and tool blocks. Shared by the settled path
/// and the streaming path so both lay out identically.
fn render_assistant_head(
    reasoning: Option<&str>,
    tools: &[crate::tui::app::ToolExecution],
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    lines: &mut Vec<Line<'static>>,
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
}

/// Append markdown-rendered content lines, each prefixed with the assistant
/// `┃ ` bar.
fn push_prefixed_content(lines: &mut Vec<Line<'static>>, md_lines: Vec<Line<'static>>) {
    let prefix_style = Style::default().fg(DEFAULT_THEME.assistant);
    for md_line in md_lines {
        let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
        spans.extend(md_line.spans);
        lines.push(Line::from(spans));
    }
}

/// The `┃ ▍` line shown under a still-streaming assistant message.
fn streaming_cursor_line() -> Line<'static> {
    let prefix_style = Style::default().fg(DEFAULT_THEME.assistant);
    Line::from(vec![
        Span::styled("\u{2503} ", prefix_style),
        Span::styled(
            "\u{258d}".to_string(), // ▍
            Style::default().fg(DEFAULT_THEME.assistant),
        ),
    ])
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

    #[test]
    fn build_visible_lines_window_matches_the_full_build_sliced() {
        // The windowed production path must be byte-identical to slicing the
        // full reference build — for both scroll modes and for a window that
        // cuts through the middle of a message (the blank-separator and
        // streaming-cursor boundary cases live in that cut).
        let mut state = AppState::new("test".into(), "claude".into());
        for i in 0..6 {
            state.add_user_message(format!("question {i}"));
            state.ensure_assistant_message();
            if let ChatMessage::Assistant {
                content,
                is_streaming,
                ..
            } = state.current_assistant_mut()
            {
                content.push_str(&format!("answer {i}\nwith a second line"));
                *is_streaming = i == 5; // only the last one stays streaming
            }
        }
        let full = build_all_lines(&state, 80);
        let mut cache = LineCache::default();

        // Auto-scroll bottom window.
        let height = 7;
        let (total, visible) = build_visible_lines(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
            true,
            0,
            height,
        );
        assert_eq!(total, full.len());
        assert_eq!(visible, full[full.len() - height..].to_vec());

        // Scrolled-up window (auto_scroll off, offset from the bottom).
        let (_total, visible) = build_visible_lines(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
            false,
            10,
            height,
        );
        let end = full.len() - 10;
        assert_eq!(visible, full[end - height..end].to_vec());

        // A second call with unchanged state must serve the same window from
        // cache (this is the per-frame steady state).
        let (_total, visible2) = build_visible_lines(
            &state.messages,
            state.verbose,
            state.spinner_frame,
            80,
            &mut cache,
            false,
            10,
            height,
        );
        assert_eq!(visible, visible2);
    }
}
