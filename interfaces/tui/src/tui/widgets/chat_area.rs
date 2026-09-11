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

use shared_ui_logic::transcript::{
    turn_summary_text, RowBody, RowStatus, ToolRow, TranscriptEntry,
};

use crate::tui::app::{AppState, Focus};
use crate::tui::markdown::{
    markdown_to_lines, markdown_to_lines_incremental, StreamLines, StreamPrefix,
};
use crate::tui::theme::theme;

use super::tool_row::{render_tool_group, render_tool_row, KEY_MODALITY};

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

/// Cheap discriminant for a `TranscriptEntry`, used only to invalidate the
/// cache safely across `messages.insert(at, ...)` (peer messages can be
/// inserted before the streaming tail, shifting its index — see
/// `app/events.rs::StreamEvent::...` peer-message handling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    ToolGroup,
    TurnSummary,
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

/// A tool row's rendered appearance changes without its text changing — the
/// status settles, a duration arrives, a body is replaced by a diff — so its
/// fingerprint has to cover those, not just a string.
fn tool_fingerprint(row: &ToolRow) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    row.id.hash(&mut h);
    row.summary.display_name.hash(&mut h);
    row.summary.args_text.hash(&mut h);
    row.expanded.hash(&mut h);
    match &row.status {
        RowStatus::Pending => 0u8.hash(&mut h),
        RowStatus::Running { .. } => 1u8.hash(&mut h),
        RowStatus::Ok { duration_ms } => {
            2u8.hash(&mut h);
            duration_ms.hash(&mut h);
        }
        RowStatus::Err {
            duration_ms,
            message,
        } => {
            3u8.hash(&mut h);
            duration_ms.hash(&mut h);
            message.hash(&mut h);
        }
    }
    match &row.body {
        RowBody::None => 0u8.hash(&mut h),
        RowBody::Text(t) => {
            1u8.hash(&mut h);
            content_fingerprint(t).hash(&mut h);
        }
        RowBody::FileChanges(c) => {
            2u8.hash(&mut h);
            c.len().hash(&mut h);
            for change in c {
                change.path.hash(&mut h);
                change.added.hash(&mut h);
                change.removed.hash(&mut h);
                change.hunks.len().hash(&mut h);
            }
        }
    }
    h.finish()
}

fn message_kind_and_fingerprint(message: &TranscriptEntry) -> (MessageKind, u64) {
    match message {
        TranscriptEntry::UserText { text, .. } => (MessageKind::User, content_fingerprint(text)),
        TranscriptEntry::AssistantText { markdown, .. } => {
            (MessageKind::Assistant, content_fingerprint(markdown))
        }
        TranscriptEntry::Reasoning { text, .. } => {
            (MessageKind::Reasoning, content_fingerprint(text))
        }
        // A RUNNING row is deliberately never cached by the caller (its
        // spinner is a function of the clock), so this fingerprint only has to
        // be right for settled ones.
        TranscriptEntry::Tool(row) => (MessageKind::Tool, tool_fingerprint(row)),
        TranscriptEntry::ToolGroup(g) => {
            let mut acc = 0u64;
            for r in &g.rows {
                acc = acc.rotate_left(7) ^ tool_fingerprint(r);
            }
            (MessageKind::ToolGroup, acc)
        }
        TranscriptEntry::TurnSummary(s) => (
            MessageKind::TurnSummary,
            content_fingerprint(&turn_summary_text(s)),
        ),
        TranscriptEntry::SystemNotice { text, .. } => {
            (MessageKind::System, content_fingerprint(text))
        }
    }
}

/// Render the chat area with all messages, handling scrolling.
pub fn render_chat_area(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let border_color = match state.focus {
        Focus::Chat => theme().border_focused,
        _ => theme().border,
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
        now_ms(),
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
    build_all_lines_at(state, width, 0)
}

#[cfg(test)]
fn build_all_lines_at(state: &AppState, width: u16, now_ms: u64) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for message in &state.messages {
        render_settled_message(message, state.verbose, now_ms, width, &mut lines);
        // The streaming cursor is the one thing the settled path does not
        // draw, because the cached path never renders a streaming entry
        // through it.
        if matches!(
            message,
            TranscriptEntry::AssistantText {
                streaming: true,
                ..
            }
        ) {
            lines.push(streaming_cursor_line());
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
    messages: &[TranscriptEntry],
    verbose: bool,
    now_ms: u64,
    width: u16,
    cache: &mut LineCache,
) -> Vec<Line<'static>> {
    let (_total, lines) =
        build_visible_lines(messages, verbose, now_ms, width, cache, true, 0, usize::MAX);
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
    messages: &[TranscriptEntry],
    verbose: bool,
    now_ms: u64,
    width: u16,
    cache: &mut LineCache,
    auto_scroll: bool,
    scroll_offset: usize,
    visible_height: usize,
) -> (usize, Vec<Line<'static>>) {
    let streaming_idx = messages.iter().position(|m| {
        matches!(
            m,
            TranscriptEntry::AssistantText {
                streaming: true,
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
    let mut streaming_content: Option<StreamLines> = None;
    for (idx, message) in messages.iter().enumerate() {
        if Some(idx) == streaming_idx {
            // The streaming entry's text grows every tick, so it is never in
            // `entries`; its markdown still gets the incremental prefix cache.
            // It no longer carries a "head" — reasoning and tool rows are
            // their own entries, each cached or re-rendered on its own terms.
            cache.entries.remove(&idx);
            if let TranscriptEntry::AssistantText { markdown, .. } = message {
                if !markdown.is_empty() {
                    streaming_content = Some(markdown_to_lines_incremental(
                        markdown,
                        width.saturating_sub(2),
                        &mut cache.streaming_markdown_cache,
                    ));
                }
                let content_rows = streaming_content
                    .as_ref()
                    .map_or(0, StreamLines::line_count);
                // +1 streaming cursor, +1 blank separator.
                heights.push(content_rows + 2);
            }
        } else {
            // A running row's glyph is a function of the clock, so its cached
            // lines are stale the moment they are stored — drop the entry and
            // let the miss below re-render it. Settled rows cache normally,
            // which is what keeps a long tool-heavy transcript cheap.
            if matches!(
                message,
                TranscriptEntry::Tool(row) if matches!(row.status, RowStatus::Running { .. })
            ) {
                cache.entries.remove(&idx);
            }
            let (kind, fingerprint) = message_kind_and_fingerprint(message);
            let hit = cache
                .entries
                .get(&idx)
                .filter(|e| e.kind == kind && e.fingerprint == fingerprint && e.width == width);
            match hit {
                Some(entry) => heights.push(entry.lines.len() + 1),
                None => {
                    let mut buf = Vec::new();
                    render_settled_message(message, verbose, now_ms, width, &mut buf);
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
                copy_streaming_slice(streaming_content.as_ref(), lo, body_hi, &mut out);
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

/// Copy the `[lo, hi)` slice of the streaming entry's rows into `out`,
/// applying the assistant prefix bar to content rows (deferred to here so
/// off-window rows never pay for it). Row layout: content | cursor.
///
/// There is no "head" any more: reasoning and tool rows used to be rendered
/// above the streaming message because they lived inside it, and they are
/// separate entries now — each windowed, cached and invalidated on its own.
fn copy_streaming_slice(
    content: Option<&StreamLines>,
    lo: usize,
    hi: usize,
    out: &mut Vec<Line<'static>>,
) {
    let content_len = content.map_or(0, StreamLines::line_count);

    let c_lo = lo.min(content_len);
    let c_hi = hi.min(content_len);
    if let Some(sl) = content {
        let prefix_style = Style::default().fg(theme().assistant);
        for i in c_lo..c_hi {
            if let Some(line) = sl.get(i) {
                let mut spans = Vec::with_capacity(line.spans.len() + 1);
                spans.push(Span::styled("\u{2503} ", prefix_style));
                spans.extend(line.spans.iter().cloned());
                out.push(Line::from(spans));
            }
        }
    }

    if lo <= content_len && content_len < hi {
        out.push(streaming_cursor_line());
    }
}

/// Wall clock in unix ms — the unit the shared spinner and duration helpers
/// take. A pre-1970 clock yields 0 rather than a panic.
fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// Render a settled (non-streaming) entry into `out`.
///
/// Tool rows are NOT prefixed with the assistant `┃ ` bar: they are peers of
/// the text now, not decoration inside a message, and indenting them under a
/// bar would put back the visual nesting the model change removed.
fn render_settled_message(
    message: &TranscriptEntry,
    verbose: bool,
    now_ms: u64,
    width: u16,
    out: &mut Vec<Line<'static>>,
) {
    match message {
        TranscriptEntry::UserText { text, at_ms, .. } => {
            render_user_message(text, *at_ms, width, out);
        }
        TranscriptEntry::AssistantText { markdown, .. } => {
            if !markdown.is_empty() {
                let md_lines = markdown_to_lines(markdown, width.saturating_sub(2));
                push_prefixed_content(out, md_lines);
            }
        }
        TranscriptEntry::Reasoning { text, .. } => {
            if verbose {
                render_reasoning(text, width, out);
            }
        }
        TranscriptEntry::Tool(row) => {
            out.extend(render_tool_row(row, now_ms, width, KEY_MODALITY));
        }
        TranscriptEntry::ToolGroup(group) => {
            out.extend(render_tool_group(group, now_ms, width, KEY_MODALITY));
        }
        TranscriptEntry::TurnSummary(summary) => {
            out.push(Line::from(Span::styled(
                turn_summary_text(summary),
                Style::default().fg(theme().muted),
            )));
        }
        TranscriptEntry::SystemNotice { text, .. } => {
            render_system_message(text, width, out);
        }
    }
}

/// Render a user message with blue prefix bar.
fn render_user_message(
    content: &str,
    at_ms: Option<u64>,
    width: u16,
    lines: &mut Vec<Line<'static>>,
) {
    let prefix_style = Style::default().fg(theme().user);
    // No clock for a message this client was never told a time for. Falling
    // back to `now` would date a restored message to the moment it was
    // restored, which is a fact the header would be stating and getting wrong.
    let time_str = at_ms
        .and_then(|ms| i64::try_from(ms).ok())
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| format!("  {}", t.format("%H:%M")))
        .unwrap_or_default();

    // Header: ┃ You  12:34
    lines.push(Line::from(vec![
        Span::styled("\u{2503} ", prefix_style),
        Span::styled(
            "You".to_string(),
            Style::default()
                .fg(theme().user)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(time_str, Style::default().fg(theme().muted)),
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

/// A reasoning entry, shown only under `/verbose`.
///
/// Its own entry now rather than a field on the assistant message, so it
/// renders where it happened instead of always above the whole turn.
fn render_reasoning(text: &str, width: u16, lines: &mut Vec<Line<'static>>) {
    let prefix_style = Style::default().fg(theme().assistant);
    let reasoning_style = Style::default().fg(theme().reasoning);
    let reasoning_prefix = Style::default().fg(theme().muted);
    let content_width = width.saturating_sub(4); // account for "┃ ┊ " prefix

    for reason_line in text.lines() {
        if reason_line.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("\u{2503} ", prefix_style),
                Span::styled("\u{250a} ", reasoning_prefix),
            ]));
            continue;
        }
        for wrapped_line in textwrap::wrap(reason_line, content_width as usize) {
            lines.push(Line::from(vec![
                Span::styled("\u{2503} ", prefix_style),
                Span::styled("\u{250a} ", reasoning_prefix),
                Span::styled(wrapped_line.into_owned(), reasoning_style),
            ]));
        }
    }
}

/// Append markdown-rendered content lines, each prefixed with the assistant
/// `┃ ` bar.
fn push_prefixed_content(lines: &mut Vec<Line<'static>>, md_lines: Vec<Line<'static>>) {
    let prefix_style = Style::default().fg(theme().assistant);
    for md_line in md_lines {
        let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
        spans.extend(md_line.spans);
        lines.push(Line::from(spans));
    }
}

/// The `┃ ▍` line shown under a still-streaming assistant message.
fn streaming_cursor_line() -> Line<'static> {
    let prefix_style = Style::default().fg(theme().assistant);
    Line::from(vec![
        Span::styled("\u{2503} ", prefix_style),
        Span::styled(
            "\u{258d}".to_string(), // ▍
            Style::default().fg(theme().assistant),
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
    let style = Style::default().fg(theme().system);
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
        if let TranscriptEntry::AssistantText {
            markdown: content, ..
        } = state.current_assistant_mut()
        {
            content.push_str("Hi there!");
        }

        let lines = build_all_lines(&state, 80);
        assert!(lines.len() >= 6);

        let has_you = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("You"))
        });
        assert!(has_you, "Should contain 'You' header");

        let has_reply = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().contains("Hi there!"))
        });
        assert!(has_reply, "the assistant's text must be rendered");
    }

    /// The whole point of the model change: a tool row sits where it ran.
    ///
    /// # When this goes red
    ///
    /// It could not even be written before — tools lived inside
    /// `ChatMessage::Assistant`, so every one of a turn's calls rendered above
    /// all of that turn's text and this ordering was unrepresentable. Put the
    /// rows back under the message and the second text lands above the tool.
    #[test]
    fn a_tool_row_renders_between_the_texts_that_surround_it() {
        use serde_json::json;
        let mut state = AppState::new("test".into(), "claude".into());
        state.messages.clear();

        state.append_assistant_content("before the call");
        state.start_tool_execution("c1".into(), "file_read".into(), &json!({"path": "a.rs"}));
        state.append_assistant_content("after the call");

        let rendered: Vec<String> = build_all_lines(&state, 80)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let idx = |needle: &str| {
            rendered
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle} not rendered: {rendered:?}"))
        };
        assert!(
            idx("before the call") < idx("Read(a.rs)"),
            "text that preceded the call must render above it: {rendered:?}"
        );
        assert!(
            idx("Read(a.rs)") < idx("after the call"),
            "text that followed the call must render below it: {rendered:?}"
        );
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
        if let TranscriptEntry::AssistantText {
            streaming: is_streaming,
            ..
        } = state.current_assistant_mut()
        {
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
        state.messages.push(TranscriptEntry::Reasoning {
            id: "r1".into(),
            text: "thinking...".into(),
            collapsed: true,
        });

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
        if let TranscriptEntry::AssistantText {
            markdown,
            streaming,
            ..
        } = state.current_assistant_mut()
        {
            markdown.push_str("Hi there!");
            *streaming = false;
        }

        let mut cache = LineCache::default();
        let first = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        let second = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        assert_eq!(first, second);
        // Cache must actually have been populated, not silently bypassed.
        assert!(!cache.entries.is_empty());
    }

    #[test]
    fn build_all_lines_invalidates_on_content_change() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();
        if let TranscriptEntry::AssistantText {
            streaming: is_streaming,
            ..
        } = state.current_assistant_mut()
        {
            // Must be non-streaming: `build_all_lines_cached` never caches a
            // streaming message (its spinner/tool content can change every
            // tick without `content_len` changing), so a still-streaming
            // message here would make the first call below a no-op cache
            // insert and the test would pass regardless of whether
            // `content_len` invalidation actually works.
            *is_streaming = false;
        }
        let mut cache = LineCache::default();
        let _first = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        assert!(
            !cache.entries.is_empty(),
            "the message must actually be cached before we test invalidation"
        );
        if let TranscriptEntry::AssistantText {
            markdown: content, ..
        } = state.current_assistant_mut()
        {
            content.push_str("new text");
        }
        let updated = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
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
        let wide = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
        let narrow = build_all_lines_cached(&state.messages, state.verbose, 0, 20, &mut cache);
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
        if let TranscriptEntry::AssistantText {
            markdown: content, ..
        } = state.current_assistant_mut()
        {
            content.push_str("Hi there!");
        }
        let mut cache = LineCache::default();
        let cached = build_all_lines_cached(&state.messages, state.verbose, 0, 80, &mut cache);
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
            if let TranscriptEntry::AssistantText {
                markdown,
                streaming,
                ..
            } = state.current_assistant_mut()
            {
                markdown.push_str(&format!("answer {i}\nwith a second line"));
                *streaming = i == 5; // only the last one stays streaming
            }
        }
        let full = build_all_lines(&state, 80);
        let mut cache = LineCache::default();

        // Auto-scroll bottom window.
        let height = 7;
        let (total, visible) = build_visible_lines(
            &state.messages,
            state.verbose,
            0,
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
            0,
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
            0,
            80,
            &mut cache,
            false,
            10,
            height,
        );
        assert_eq!(visible, visible2);
    }
}
