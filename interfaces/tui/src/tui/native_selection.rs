//! Host-terminal text selection, without a synthetic selection or clipboard.
//!
//! F6 releases mouse capture and freezes drawing; drag and copy using the host
//! terminal's own bindings (Windows Terminal normally uses Ctrl+Shift+C).
//! F6 or Esc resumes the TUI. Gateway events may continue updating app state;
//! only the visible frame is frozen. The caller must gate every draw on `active`.
//!
//! Platform limits: this selects visible terminal cells, not off-screen chat
//! history, and may include UI chrome and visual line wraps. Copy bindings,
//! selection persistence and resize/reflow while frozen belong to the emulator.
//! Some hosts consume the first Esc to clear their selection: press Esc again
//! or F6 to resume. Legacy Windows console QuickEdit can also suspend input;
//! clear its selection first. Raw mode stays enabled so exit keys still work.
//! Crossterm 0.28 uses console APIs for capture on Windows (even with an in-memory
//! writer), so tests inject the capture operation instead of touching a console.

use std::io::{self, Write};

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
};

use super::event::TermEvent;

/// Selection owns the last painted frame, not any application state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NativeSelection {
    active: bool,
    restore_mouse_capture: bool,
}

impl NativeSelection {
    /// Do not enable capture on exit if startup never successfully enabled it.
    pub(super) const fn new(mouse_capture: bool) -> Self {
        Self {
            active: false,
            restore_mouse_capture: mouse_capture,
        }
    }

    pub(super) const fn active(self) -> bool {
        self.active
    }

    /// Return true when consumed; only false events may reach normal routing.
    /// Cursor visibility stays owned by ratatui, including when an overlay is up.
    pub(super) fn handle_event<W: Write>(
        &mut self,
        event: &TermEvent,
        writer: &mut W,
    ) -> io::Result<bool> {
        self.handle_event_with(event, |capture| {
            if capture {
                execute!(writer, EnableMouseCapture)
            } else {
                execute!(writer, DisableMouseCapture)
            }
        })
    }

    fn handle_event_with(
        &mut self,
        event: &TermEvent,
        mut set_capture: impl FnMut(bool) -> io::Result<()>,
    ) -> io::Result<bool> {
        let next = match event {
            TermEvent::Key(key) if unmodified(key, KeyCode::F(6)) => !self.active,
            TermEvent::Key(key)
                if self.active
                    && (unmodified(key, KeyCode::Esc)
                        || unmodified(key, KeyCode::Char('\x1b'))) =>
            {
                false
            }
            // A resize changes the host selection coordinates. Resume first,
            // then let the normal router redraw against the new geometry.
            TermEvent::Resize if self.active => false,
            // This includes queued mouse events, paste, resize and normal keys:
            // none may act on a stale hit-test table or an invisible composer.
            _ => return Ok(self.active),
        };
        if self.restore_mouse_capture {
            set_capture(!next)?;
        }
        // Do not claim a successful transition when the terminal write failed.
        self.active = next;
        Ok(true)
    }
}

fn unmodified(key: &KeyEvent, code: KeyCode) -> bool {
    key.kind == KeyEventKind::Press && key.code == code && key.modifiers == KeyModifiers::NONE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    fn key(code: KeyCode) -> TermEvent {
        TermEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn wheel_or_click(kind: MouseEventKind) -> TermEvent {
        TermEvent::Mouse(MouseEvent {
            kind,
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn f6_releases_capture_and_f6_or_escape_restores_it() {
        for exit in [KeyCode::F(6), KeyCode::Esc] {
            let mut selection = NativeSelection::new(true);
            let mut captures = Vec::new();
            assert!(!selection.active());
            assert!(selection
                .handle_event_with(&key(KeyCode::F(6)), |capture| {
                    captures.push(capture);
                    Ok(())
                })
                .unwrap());
            assert!(selection.active());
            assert_eq!(captures, [false]);
            assert!(selection
                .handle_event_with(&key(exit), |capture| {
                    captures.push(capture);
                    Ok(())
                })
                .unwrap());
            assert!(!selection.active());
            assert_eq!(captures, [false, true]);
        }
    }

    #[test]
    fn frozen_input_is_consumed_and_normal_input_returns_to_existing_router() {
        let mut selection = NativeSelection::new(true);
        let events = [
            wheel_or_click(MouseEventKind::ScrollUp),
            wheel_or_click(MouseEventKind::ScrollDown),
            wheel_or_click(MouseEventKind::Down(MouseButton::Left)),
            wheel_or_click(MouseEventKind::Drag(MouseButton::Left)),
            key(KeyCode::Char('a')),
            key(KeyCode::Enter),
            TermEvent::Paste("must not send\na message".into()),
            TermEvent::Key(KeyEvent::new(KeyCode::F(6), KeyModifiers::SHIFT)),
        ];
        for active in [false, true, false] {
            if selection.active() != active {
                selection
                    .handle_event_with(&key(KeyCode::F(6)), |_| Ok(()))
                    .unwrap();
            }
            for event in &events {
                assert_eq!(
                    selection
                        .handle_event_with(event, |_| panic!("not a transition"))
                        .unwrap(),
                    active,
                    "{event:?}"
                );
                assert_eq!(selection.active(), active);
            }
        }
        // Esc must reach the normal overlay handler again after resuming.
        assert!(!selection
            .handle_event_with(&key(KeyCode::Esc), |_| panic!("not a transition"))
            .unwrap());
    }

    #[test]
    fn resize_and_escape_character_resume_without_using_stale_coordinates() {
        let mut selection = NativeSelection::new(true);
        let mut captures = Vec::new();
        selection
            .handle_event_with(&key(KeyCode::F(6)), |capture| {
                captures.push(capture);
                Ok(())
            })
            .unwrap();
        assert!(selection.active());
        assert!(selection
            .handle_event_with(&TermEvent::Resize, |capture| {
                captures.push(capture);
                Ok(())
            })
            .unwrap());
        assert!(!selection.active());
        assert_eq!(captures, [false, true]);

        selection
            .handle_event_with(&key(KeyCode::F(6)), |_| Ok(()))
            .unwrap();
        assert!(selection.active());
        assert!(selection
            .handle_event_with(&key(KeyCode::Char('\x1b')), |_| Ok(()))
            .unwrap());
        assert!(!selection.active());
    }

    #[test]
    fn repeat_and_release_cannot_toggle_selection() {
        let mut selection = NativeSelection::new(true);
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            let mut key = KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE);
            key.kind = kind;
            assert!(!selection
                .handle_event_with(&TermEvent::Key(key), |_| panic!("not a transition"))
                .unwrap());
            assert!(!selection.active());
        }
    }

    #[test]
    fn failed_capture_is_not_reported_as_a_successful_transition() {
        let mut selection = NativeSelection::new(true);
        for active in [false, true] {
            selection.active = active;
            let error = selection
                .handle_event_with(&key(KeyCode::F(6)), |_| {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "capture failed"))
                })
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert_eq!(selection.active(), active);
        }
    }

    #[test]
    fn startup_without_capture_never_enables_it_on_exit() {
        let mut selection = NativeSelection::new(false);
        for code in [KeyCode::F(6), KeyCode::Esc] {
            assert!(selection
                .handle_event_with(&key(code), |_| panic!("capture was never ours"))
                .unwrap());
        }
        assert!(!selection.active());
    }

    #[test]
    fn main_loop_gates_the_real_draw_and_routes_only_unconsumed_events() {
        let source = include_str!("mod.rs").replace('\r', "");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let code = production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let main_loop = code.split("async fn main_loop").nth(1).unwrap();
        assert_eq!(main_loop.matches("terminal.draw(").count(), 2);
        assert!(main_loop.contains(
            "if needs_redraw && !native_selection.active() {\n            terminal.draw("
        ));
        assert!(main_loop.contains(
            "let action = if native_selection.handle_event(&te, terminal.backend_mut())? {\n                    needs_redraw = !native_selection.active();\n                    Action::None\n                } else {\n                    needs_redraw = true;\n                    keys::handle_terminal_event(state, textarea, &te)"
        ));
        assert!(main_loop.contains("widgets::hint_line::render_selection_hint(frame)"));
    }
}
