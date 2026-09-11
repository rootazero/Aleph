// The one-row key hint under the composer.
//
// `⏵⏵ ctrl+o expand · ctrl+end bottom · /help commands`
//
// # What this line is for, and what it is deliberately not
//
// It names KEYS. The status bar names STATE (model, context, cost, the
// conversation's knobs). Keeping that split is why the exec tier is not
// echoed here even though the spec's sketch has `⏵⏵ auto` — the tier already
// has exactly one home, on the status line, and a second rendering of it is
// the same fact in two places waiting to disagree (判据 §1).
//
// Every key named here must be bound. The three forms below are chosen by the
// same conditions, in the same order, that `keys::handle_global_key` uses to
// decide what Ctrl+C means — see `the_hint_names_what_ctrl_c_will_actually_do`,
// which drives the real handler rather than restating the rule.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use shared_ui_logic::transcript::SemanticColor;

use crate::tui::theme::palette;

/// The `⏵⏵` marker that opens the line.
pub const MARKER: &str = "\u{23f5}\u{23f5} ";

/// While a run is in flight: the interrupt comes first, because it is the one
/// thing a reader may urgently want and cannot guess.
pub const RUNNING: &str = "ctrl+c interrupt \u{b7} ctrl+o expand \u{b7} ctrl+end bottom";

/// While the composer holds text: the two keys that send it, and what Ctrl+C
/// means *here* (clear the draft — there is no run to cancel).
///
/// These words used to be the input box's title, which B6 removed along with
/// the box.
pub const TYPING: &str = "enter send \u{b7} \\+enter newline \u{b7} ctrl+c clear";

/// Idle.
pub const IDLE: &str = "ctrl+o expand \u{b7} ctrl+end bottom \u{b7} /help commands";

/// Which of the three forms the current state calls for.
///
/// `running` wins over `typing` because `handle_global_key` checks
/// `current_run` before it checks the composer: with both true, Ctrl+C
/// cancels the run and the draft is left alone.
#[must_use]
pub const fn hint_text(running: bool, typing: bool) -> &'static str {
    if running {
        RUNNING
    } else if typing {
        TYPING
    } else {
        IDLE
    }
}

/// Paint the hint row.
pub fn render_hint_line(frame: &mut Frame, area: Rect, running: bool, typing: bool) {
    let dim = Style::default().fg(palette().color(SemanticColor::Dim));
    let line = Line::from(vec![
        Span::styled(MARKER.to_string(), dim),
        Span::styled(hint_text(running, typing).to_string(), dim),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Action, AppState};
    use crate::tui::event::TermEvent;
    use crate::tui::keys::handle_terminal_event;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use tui_textarea::TextArea;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> TermEvent {
        let mut k = KeyEvent::new(code, modifiers);
        k.kind = KeyEventKind::Press;
        TermEvent::Key(k)
    }

    #[test]
    fn a_run_in_flight_outranks_a_half_written_draft() {
        assert_eq!(hint_text(true, true), RUNNING);
        assert_eq!(hint_text(true, false), RUNNING);
        assert_eq!(hint_text(false, true), TYPING);
        assert_eq!(hint_text(false, false), IDLE);
    }

    /// The guard that keeps this line from lying: the word after `ctrl+c`
    /// is checked against what the key handler *does* in that same state,
    /// not against a second copy of the rule.
    ///
    /// Reddens if `handle_global_key`'s Ctrl+C branches are reordered (the
    /// draft-clear arm moved above the cancel arm) without this line being
    /// reworded to match.
    #[test]
    fn the_hint_names_what_ctrl_c_will_actually_do() {
        let ctrl_c = key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        // Running, with a draft in the composer: the hint says "interrupt".
        let mut state = AppState::new("s".into(), "m".into());
        state.current_run = Some("run-1".into());
        let mut ta = TextArea::from(vec!["half a thought".to_string()]);
        assert_eq!(hint_text(true, true), RUNNING);
        assert!(RUNNING.contains("ctrl+c interrupt"));
        assert!(
            matches!(handle_terminal_event(&mut state, &mut ta, &ctrl_c), Action::CancelRun(id) if id == "run-1"),
            "the hint promises an interrupt, so Ctrl+C must cancel the run"
        );

        // Idle, with a draft: the hint says "clear", and the draft goes.
        let mut state = AppState::new("s".into(), "m".into());
        let mut ta = TextArea::from(vec!["half a thought".to_string()]);
        assert_eq!(hint_text(false, true), TYPING);
        assert!(TYPING.contains("ctrl+c clear"));
        let _ = handle_terminal_event(&mut state, &mut ta, &ctrl_c);
        assert_eq!(
            ta.lines(),
            [""],
            "the hint promises a clear, so Ctrl+C must empty the composer"
        );
    }

    /// Every key this line names has to exist. `/help` is checked as a
    /// command elsewhere; the three key names are checked here against the
    /// bindings B5 added, so deleting one of those arms cannot leave the
    /// hint advertising it.
    #[test]
    fn the_keys_the_idle_form_names_are_bound() {
        let mut state = AppState::new("s".into(), "m".into());
        let mut ta = TextArea::default();

        assert!(IDLE.contains("ctrl+o"));
        assert!(matches!(
            handle_terminal_event(
                &mut state,
                &mut ta,
                &key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            ),
            Action::ToggleExpandAll
        ));

        assert!(IDLE.contains("ctrl+end"));
        assert!(matches!(
            handle_terminal_event(
                &mut state,
                &mut ta,
                &key(KeyCode::End, KeyModifiers::CONTROL)
            ),
            Action::ScrollToBottom
        ));
    }
}
