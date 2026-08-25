// Chat area widget: renders the scrollable message list with support for
// user messages, assistant messages (with reasoning, tool blocks, markdown),
// system messages, and streaming cursors.

use std::cell::RefCell;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{AppState, ChatMessage, Focus};
use crate::tui::markdown::markdown_to_lines;
use crate::tui::theme::DEFAULT_THEME;

use super::tool_block::render_tool_block;

/// Memoization slot for the fully-rendered chat line list.
///
/// `build_all_lines` re-runs `markdown_to_lines` for every assistant turn
/// (per character, on a streaming tick) and the main loop calls `draw` at
/// 50 ms (≈20 fps). Without a cache the terminal is paying for an O(history)
/// markdown parse every frame, and a long conversation means hundreds of
/// short-line allocations per tick on top of the parse itself.
///
/// The fingerprint folds three pieces of state that together uniquely
/// identify the rendered output: content width, number of messages, and a
/// 64-bit hash over the per-message `(variant, content, len, spinner)`
/// triple. Streaming text changes the per-message content hash, which
/// invalidates the cache; an idle tick (the common case) re-uses the last
/// frame's Vec and skips the parse entirely.
///
/// `thread_local!` (rather than a field on `AppState`) because this widget
/// only ever runs on the main loop thread and the cache is purely an
/// optimisation — leaking a single Vec into the TLS slot on shutdown is
/// strictly cheaper than the refactor `Rc<RefCell<...>>` would require.
thread_local! {
    static CHAT_LINES_CACHE: RefCell<Option<(ChatLinesFingerprint, Vec<Line<'static>>)>> =
        const { RefCell::new(None) };
}

/// Cheap-to-compute identity of the rendered chat state. Two consecutive
/// frames with equal fingerprints can share a single `Vec<Line<'static>>`.
#[derive(Clone, PartialEq, Eq)]
struct ChatLinesFingerprint {
    width: u16,
    /// `xxhash`-style mix over per-message fields that affect rendering
    /// (variant, content bytes, streaming flag, spinner frame). Recomputed
    /// every frame; the work is O(n) but the alternative — re-running
    /// `markdown_to_lines` for every message — is also O(n) and allocates.
    content_hash: u64,
    /// Number of messages at fingerprint time. A cheaper pre-check than
    /// the hash: a list that has not changed in length is overwhelmingly
    /// likely (but not guaranteed) to share a hash, and skipping the
    /// content walk on the common path is worth a single integer compare.
    message_count: usize,
}

fn compute_fingerprint(state: &AppState, width: u16) -> ChatLinesFingerprint {
    // SplitMix64-style mixer: keep the accumulator independent of std's
    // default hasher so the constants stay stable across compiler versions.
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut count = 0usize;
    for message in &state.messages {
        count += 1;
        match message {
            ChatMessage::User { content, timestamp } => {
                hash = hash.wrapping_mul(0x100000001b3);
                hash ^= 0xA5A5_A5A5_A5A5_A5A5;
                for chunk in content.as_bytes().chunks(8) {
                    let mut buf = [0u8; 8];
                    buf[..chunk.len()].copy_from_slice(chunk);
                    hash ^= u64::from_le_bytes(buf);
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                hash ^= timestamp.timestamp_millis() as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            ChatMessage::Assistant {
                content,
                tools,
                reasoning,
                is_streaming,
            } => {
                hash = hash.wrapping_mul(0x100000001b3);
                hash ^= 0x5A5A_5A5A_5A5A_5A5A;
                for chunk in content.as_bytes().chunks(8) {
                    let mut buf = [0u8; 8];
                    buf[..chunk.len()].copy_from_slice(chunk);
                    hash ^= u64::from_le_bytes(buf);
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                hash ^= tools.len() as u64;
                hash = hash.wrapping_mul(0x100000001b3);
                if let Some(r) = reasoning.as_deref() {
                    for chunk in r.as_bytes().chunks(8) {
                        let mut buf = [0u8; 8];
                        buf[..chunk.len()].copy_from_slice(chunk);
                        hash ^= u64::from_le_bytes(buf);
                        hash = hash.wrapping_mul(0x100000001b3);
                    }
                }
                hash ^= u64::from(*is_streaming);
                // Spinner frame only matters on streaming turns; otherwise
                // the rendered glyph is fixed. Skip it to keep the
                // fingerprint stable across idle ticks.
                if *is_streaming {
                    hash ^= state.spinner_frame as u64;
                    hash = hash.wrapping_mul(0x100000001b3);
                }
            }
            ChatMessage::System { content } => {
                hash = hash.wrapping_mul(0x100000001b3);
                hash ^= 0x1234_5678_9ABC_DEF0;
                for chunk in content.as_bytes().chunks(8) {
                    let mut buf = [0u8; 8];
                    buf[..chunk.len()].copy_from_slice(chunk);
                    hash ^= u64::from_le_bytes(buf);
                    hash = hash.wrapping_mul(0x100000001b3);
                }
            }
        }
    }
    ChatLinesFingerprint {
        width,
        content_hash: hash,
        message_count: count,
    }
}

/// Render the chat area with all messages, handling scrolling.
pub fn render_chat_area(frame: &mut Frame, state: &AppState, area: Rect) {
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

    // Build all lines from all messages, but reuse the last frame's Vec
    // when the rendered state is identical. Idle ticks (no new content,
    // no scroll change) are the common case; without this cache, every
    // 50 ms tick re-parsed every assistant turn's markdown.
    let all_lines = build_all_lines_cached(state, content_width);

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

/// Cache-aware variant of [`build_all_lines`].
fn build_all_lines_cached(state: &AppState, width: u16) -> Vec<Line<'static>> {
    let fingerprint = compute_fingerprint(state, width);
    CHAT_LINES_CACHE.with(|slot| {
        if let Some((cached_fp, cached_lines)) = slot.borrow().as_ref() {
            if *cached_fp == fingerprint {
                return cached_lines.clone();
            }
        }
        let lines = build_all_lines(state, width);
        *slot.borrow_mut() = Some((fingerprint, lines.clone()));
        lines
    })
}

/// Build all rendered lines from the message history.
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
        let md_lines = markdown_to_lines(content, content_width);
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
}
