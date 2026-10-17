//! Local configuration is not conversational content.
use super::{
    app::{Action, AppState, Focus},
    event::TermEvent,
    keys, render,
    theme::theme,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;

#[test]
fn picker_navigation_and_escape_leave_transcript_and_draft_untouched() {
    let mut state = AppState::new("theme-local".into(), "model".into());
    state.add_user_message("keep this".into(), Vec::new());
    let before = state.messages.len();
    state.open_theme_picker(vec!["amber".into(), "tide".into()], Vec::new());
    let mut draft = TextArea::default();
    draft.insert_str("unfinished draft");
    let action = keys::handle_terminal_event(
        &mut state,
        &mut draft,
        &TermEvent::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    assert!(matches!(action, Action::None));
    assert_eq!(state.theme_picker.as_ref().unwrap().selected, 1);
    let action = keys::handle_terminal_event(
        &mut state,
        &mut draft,
        &TermEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    assert!(matches!(action, Action::CloseOverlay));
    state.close_overlay();
    assert!(state.theme_picker.is_none());
    assert_eq!(state.focus, Focus::Input);
    assert_eq!(state.messages.len(), before);
    assert_eq!(draft.lines(), &["unfinished draft"]);
}

#[test]
fn pending_question_theme_command_never_becomes_an_answer() {
    let mut state = AppState::new("theme-local".into(), "model".into());
    state.handle_gateway_event(aleph_protocol::StreamEvent::AskUser {
        request_id: "theme-test".into(),
        run_id: "parked".into(),
        seq: 1,
        session_key: "theme-local".into(),
        question: "Answer later".into(),
        options: Vec::new(),
        questions: Vec::new(),
        answered: 0,
    });
    let owner = state.interaction.active_owner().cloned();
    state.interaction.active_dialog_mut().unwrap().input = "/theme list".into();
    let mut draft = TextArea::default();
    let action = keys::handle_terminal_event(
        &mut state,
        &mut draft,
        &TermEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(matches!(
        action,
        Action::LocalCommand(super::slash::LocalCommand::Theme { .. })
    ));
    assert_eq!(state.interaction.active_owner(), owner.as_ref());
    state.open_theme_picker(vec!["amber".into()], Vec::new());
    state.close_overlay();
    assert_eq!(state.focus, Focus::Dialog);
    assert_eq!(state.interaction.active_owner(), owner.as_ref());
}

#[test]
fn theme_picker_renders_choices_and_local_feedback_above_composer() {
    let mut state = AppState::new("theme-local".into(), "model".into());
    state.open_theme_picker(
        vec!["amber".into(), "tide".into(), "paper".into()],
        Vec::new(),
    );
    state.theme_picker.as_mut().unwrap().feedback = Some("Theme: amber · applied".into());
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| render::render(f, &mut state, &TextArea::default()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("TUI themes"));
    assert!(text.contains("tide"));
    assert!(text.contains("Theme: amber"));
    assert!(buffer.content.iter().any(|cell| cell.fg == theme().heading));
    assert_eq!(state.messages.len(), 0);
}
