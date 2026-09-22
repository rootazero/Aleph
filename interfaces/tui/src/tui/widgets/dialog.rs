// Dialog widget: renders an inline confirmation dialog for AskUser events
// as a centered overlay with a question and selectable options.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{ApprovalState, DialogState};
use crate::tui::theme::theme;

/// The answer buffer as it should appear on screen.
///
/// Masked when the question asked for a credential — the cleartext never leaves
/// [`DialogState::input`], so a screen share, a scrollback, or a terminal
/// recording sees dots. (The property that makes `secret` more than cosmetic is
/// enforced server-side: `clarification::ask` refuses to hand a secret question
/// to a messaging channel at all, so this overlay and the Panel are the only
/// places it can be typed.)
///
/// Tail-truncated rather than head-truncated: what the user just typed is what
/// they need to see.
fn input_display(input: &str, secret: bool, width: usize) -> String {
    let shown: String = if secret {
        "•".repeat(input.chars().count())
    } else {
        input.to_string()
    };
    let len = shown.chars().count();
    if width == 0 || len <= width {
        return shown;
    }
    shown.chars().skip(len - width).collect()
}

/// The one-line key legend for the overlay's current mode.
///
/// Mode-specific because the wrong legend is worse than none: this overlay
/// swallows `Esc` (the run is parked on a oneshot), so the legend is the only
/// place a user learns that a typed answer is possible at all.
fn hint_for(dialog: &DialogState) -> &'static str {
    if dialog.typing {
        if dialog.has_quick_pick() {
            "Type your answer · Enter send · Tab back to the list"
        } else if dialog.multi_select {
            "Type numbers separated by commas, or your own answer · Enter send"
        } else {
            "Type your answer · Enter send"
        }
    } else {
        "1-9 select · ↑↓ move · Enter confirm · Tab type your own"
    }
}

/// Render the confirmation dialog as a centered overlay.
pub fn render_dialog(frame: &mut Frame, dialog: &DialogState, area: Rect) {
    // Calculate dialog dimensions
    let dialog_width = area.width.clamp(20, 50);
    let option_count = u16::try_from(dialog.options.len()).unwrap_or(u16::MAX);
    // Height: 2 borders + 1 blank + question lines (estimate 2) + 1 blank +
    // options + 1 input + 1 hint
    let dialog_height = (option_count.saturating_add(8)).min(area.height);

    // Center the dialog
    let dialog_rect = centered_rect(dialog_width, dialog_height, area);

    // Clear background behind the dialog
    frame.render_widget(Clear, dialog_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().warning))
        .title(" Agent needs your input ");

    let inner = block.inner(dialog_rect);
    frame.render_widget(block, dialog_rect);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Split inner area into question + options + answer line + hint
    let chunks = Layout::vertical([
        Constraint::Length(1),            // blank line
        Constraint::Min(2),               // question
        Constraint::Length(1),            // blank line
        Constraint::Length(option_count), // options (absent for free text)
        Constraint::Length(1),            // typed answer
        Constraint::Length(1),            // hint line
    ])
    .split(inner);

    let question_area = chunks.get(1).copied().unwrap_or_default();
    let options_area = chunks.get(3).copied().unwrap_or_default();
    let input_area = chunks.get(4).copied().unwrap_or_default();
    let hint_area = chunks.get(5).copied().unwrap_or_default();

    // Render question
    let question = Paragraph::new(Line::from(Span::styled(
        dialog.question.clone(),
        Style::default().fg(theme().primary),
    )))
    .wrap(Wrap { trim: true });
    frame.render_widget(question, question_area);

    // Render options
    let mut option_lines: Vec<Line> = dialog
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let is_selected = i == dialog.selected;
            let style = if is_selected {
                Style::default()
                    .fg(theme().primary)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme().muted)
            };
            Line::from(Span::styled(format!("  [{}] {}", i + 1, opt), style))
        })
        .collect();

    // If the option list overflows the visible area, signal that there is
    // more below — without this, the user has no way to know the list was
    // truncated and pressing a high number key silently does nothing.
    let total_options = dialog.options.len();
    let visible_options = options_area.height as usize;
    if total_options > visible_options && visible_options >= 2 {
        // Trade one option row for the indicator so the hint actually fits.
        let reserve = visible_options - 1;
        let hidden = total_options - reserve;
        option_lines.truncate(reserve);
        option_lines.push(Line::from(Span::styled(
            format!("  \u{2193} {hidden} more"),
            Style::default().fg(theme().muted),
        )));
    }

    let options_widget = Paragraph::new(option_lines);
    frame.render_widget(options_widget, options_area);

    // Render the typed answer. Always present, including in pick mode where it
    // is empty: a permanently reserved line is how the user discovers that
    // typing is an option on a question that also offers a menu.
    let caret = if dialog.typing { "▏" } else { "" };
    let prefix = "> ";
    let room = usize::from(input_area.width).saturating_sub(prefix.len() + caret.len());
    let typed = input_display(&dialog.input, dialog.secret, room);
    let input_style = if dialog.typing {
        Style::default().fg(theme().primary)
    } else {
        Style::default().fg(theme().muted)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{prefix}{typed}{caret}"),
            input_style,
        ))),
        input_area,
    );

    // Render hint
    let hint = Paragraph::new(Line::from(Span::styled(
        hint_for(dialog),
        Style::default().fg(theme().muted),
    )));
    frame.render_widget(hint, hint_area);
}

/// Render the tool-approval overlay: a red-bordered modal a parked Ask-tier run
/// is waiting on. Deliberately distinct from [`render_dialog`] (AskUser) so a
/// security decision never looks like an ordinary agent question. Shares only
/// [`centered_rect`] — the layout is copied rather than abstracted (two
/// consumers; the wrong abstraction would cost more than the duplication).
pub fn render_approval(frame: &mut Frame, approval: &ApprovalState, area: Rect) {
    let width = area.width.clamp(28, 60);
    let option_count = u16::try_from(approval.decisions.len()).unwrap_or(3);
    // 2 borders + 1 blank + question (2) + 1 blank + options + 1 hint
    let height = (option_count.saturating_add(7)).min(area.height);
    let rect = centered_rect(width, height, area);

    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().error))
        .title(" \u{26a0} Tool approval required ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(1),                   // blank line
        Constraint::Min(2),                      // command (+ reason)
        Constraint::Length(1),                   // blank line
        Constraint::Length(option_count.max(1)), // decisions
        Constraint::Length(1),                   // hint line
    ])
    .split(inner);

    let question_area = chunks.get(1).copied().unwrap_or_default();
    let options_area = chunks.get(3).copied().unwrap_or_default();
    let hint_area = chunks.get(4).copied().unwrap_or_default();

    // Command being gated, plus the server's reason (dim) when present.
    let mut question_lines = vec![Line::from(Span::styled(
        approval.command.clone(),
        Style::default().fg(theme().primary),
    ))];
    if let Some(reason) = &approval.reason {
        question_lines.push(Line::from(Span::styled(
            format!("Reason: {reason}"),
            Style::default().fg(theme().muted),
        )));
    }
    frame.render_widget(
        Paragraph::new(question_lines).wrap(Wrap { trim: true }),
        question_area,
    );

    let option_lines: Vec<Line> = approval
        .decisions
        .iter()
        .enumerate()
        .map(|(i, (label, _decision))| {
            let style = if i == approval.selected {
                Style::default()
                    .fg(theme().primary)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme().muted)
            };
            Line::from(Span::styled(format!("  [{}] {}", i + 1, label), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(option_lines), options_area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Number key or ↑↓ + Enter to decide".to_string(),
            Style::default().fg(theme().muted),
        ))),
        hint_area,
    );
}

/// Calculate a centered rect within the given area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_in_large_area() {
        let area = Rect::new(0, 0, 100, 40);
        let r = centered_rect(50, 10, area);
        assert_eq!(r.x, 25);
        assert_eq!(r.y, 15);
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 10);
    }

    #[test]
    fn centered_rect_clamps_to_area() {
        let area = Rect::new(0, 0, 20, 10);
        let r = centered_rect(50, 20, area);
        // Width and height should be clamped
        assert_eq!(r.width, 20);
        assert_eq!(r.height, 10);
    }

    /// A credential must never be legible on screen — a screen share, a
    /// scrollback buffer and a terminal recording all outlive the question.
    #[test]
    fn a_secret_answer_is_masked_character_for_character() {
        let shown = input_display("hunter2", true, 40);
        assert_eq!(shown, "•••••••");
        assert!(!shown.contains("hunter"));
        // Non-secret input is shown as typed.
        assert_eq!(input_display("hunter2", false, 40), "hunter2");
    }

    /// Tail, not head: what the user just typed is what they need to see.
    #[test]
    fn an_overlong_answer_shows_its_tail() {
        assert_eq!(input_display("abcdefghij", false, 4), "ghij");
        // A zero-width line must not panic.
        assert_eq!(input_display("abc", false, 0), "abc");
    }

    /// The legend is the only place a user learns typing is possible at all —
    /// this overlay swallows `Esc`, so a wrong legend is a trapped user.
    #[test]
    fn the_legend_follows_the_mode() {
        let mut dialog = DialogState {
            session_key: "k".into(),
            question: "?".into(),
            options: vec!["a".into()],
            selected: 0,
            multi_select: false,
            secret: false,
            input: String::new(),
            typing: false,
        };
        assert!(hint_for(&dialog).contains("Tab type your own"));
        dialog.typing = true;
        assert!(hint_for(&dialog).contains("back to the list"));

        // No menu ⇒ no mention of a list that is not there.
        dialog.options.clear();
        assert!(!hint_for(&dialog).contains("list"));
        assert!(hint_for(&dialog).contains("Enter send"));

        dialog.multi_select = true;
        assert!(hint_for(&dialog).contains("commas"));
    }

    /// Render `dialog` into a `(w, h)` test backend and return the visible
    /// text as a single string (rows joined by '\n'). Mirrors the helper
    /// `command_palette` uses so a regression on what the user actually sees
    /// is caught here, not only in a code review.
    fn paint(dialog: &DialogState, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).expect("backend");
        term.draw(|f| render_dialog(f, dialog, f.area()))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        buf.cell((x, y))
                            .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn dialog_with(options: Vec<&'static str>) -> DialogState {
        DialogState {
            session_key: "k".into(),
            question: "Pick one".into(),
            options: options.into_iter().map(String::from).collect(),
            selected: 0,
            multi_select: false,
            secret: false,
            input: String::new(),
            typing: false,
        }
    }

    /// **B8's guard.** When the menu outgrows the box, the user has no way to
    /// know there is more to scroll to without a visible affordance — the
    /// question looks complete and pressing the next number key silently
    /// does nothing. Pin the indicator so a future "let's drop the trailing
    /// row, it never has content" cleanup is caught here.
    #[test]
    fn dialog_shows_more_indicator_when_options_exceed_visible() {
        // Eight options in a 12-row frame: the dialog borders eat two rows
        // and the question / answer / blanks take another four, leaving the
        // list visibly truncated. The rest MUST be signalled.
        let dialog = dialog_with(vec![
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
        ]);
        let screen = paint(&dialog, 50, 12);
        assert!(
            screen.contains("more"),
            "no scroll indicator on screen:\n{screen}"
        );
    }

    /// The indicator is for "there is more below", not decoration. Showing
    /// it on a short list is a louder lie than the silent scroll — the user
    /// presses Up/Down expecting hidden rows and gets nothing.
    #[test]
    fn dialog_omits_more_indicator_when_options_fit() {
        // Three options in a roomy 24-row frame: there is nothing to scroll.
        let dialog = dialog_with(vec!["alpha", "bravo", "charlie"]);
        let screen = paint(&dialog, 60, 24);
        assert!(
            !screen.contains("more"),
            "spurious scroll indicator on a short list:\n{screen}"
        );
    }
}
