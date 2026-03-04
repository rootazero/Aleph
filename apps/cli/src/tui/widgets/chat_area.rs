// Chat area widget: renders the scrollable message list with support for
// user messages, assistant messages (with reasoning, tool blocks, markdown),
// system messages, and streaming cursors.

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

    // Build all lines from all messages
    let all_lines = build_all_lines(state, content_width);

    // Calculate the visible window based on scroll state
    let total_lines = all_lines.len();
    let visible_lines = if state.auto_scroll {
        // Show the last visible_height lines
        let start = total_lines.saturating_sub(visible_height);
        &all_lines[start..]
    } else {
        // scroll_offset = how many lines from the bottom we've scrolled up
        let end = total_lines.saturating_sub(state.scroll_offset);
        let start = end.saturating_sub(visible_height);
        &all_lines[start..end]
    };

    let paragraph = Paragraph::new(visible_lines.to_vec()).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
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
                render_system_message(content, &mut lines);
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
            format!("  {}", time_str),
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
                        Span::styled(
                            wrapped_line.into_owned(),
                            reasoning_style,
                        ),
                    ]));
                }
            }

            // Blank line after reasoning
            lines.push(Line::from(vec![Span::styled(
                "\u{2503} ", prefix_style,
            )]));
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
fn render_system_message(content: &str, lines: &mut Vec<Line<'static>>) {
    let style = Style::default().fg(DEFAULT_THEME.system);
    lines.push(Line::from(vec![
        Span::styled("  ", style),
        Span::styled(content.to_string(), style),
    ]));
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
        assert!(!has_thinking, "Reasoning should not show in non-verbose mode");

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
