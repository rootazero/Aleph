// Terminal event collection.
//
// Spawns a blocking task that polls crossterm events every 50ms and
// forwards Key/Resize events to an async mpsc channel. The main loop
// receives from this channel alongside gateway events.

use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEvent};
use std::time::Duration;
use tokio::sync::mpsc;

/// Terminal events relevant to the TUI.
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// A keyboard event
    Key(KeyEvent),
    /// Terminal was resized
    Resize,
    /// A bracketed paste. Multi-line safe (inserted verbatim, never routed
    /// through the Enter/send path). Unix/macOS only — crossterm's Windows
    /// console source does not synthesize `Event::Paste`.
    Paste(String),
    /// A wheel turn or left-button click. High-rate drag/move/release records
    /// are discarded before this layer; only these events are consumed by the
    /// TUI. Only arrives while mouse capture is on; a terminal that refused
    /// capture simply never produces one, which is why every affordance a
    /// click reaches also has a key (spec §8).
    Mouse(MouseEvent),
}

/// Strip ANSI/OSC escape sequences and non-printing control characters from
/// text delivered by the terminal's bracketed-paste mechanism.
///
/// Crossterm forwards the raw bytes the terminal reports, so a paste that
/// contains terminal control sequences can otherwise drive the cursor, clear
/// the screen, or inject OSC 52 clipboard requests. We keep only printable
/// text, horizontal tab, and newline; everything else is dropped.
///
/// Implementation note: the previous version iterated bytes, which silently
/// corrupted any UTF-8 multi-byte character whose lead byte landed before an
/// OSC sequence and whose continuation bytes landed inside it — the lead
/// byte was kept (printed), the continuations were dropped as part of the
/// OSC body, and `String::from_utf8` then failed (returning empty). Iterating
/// `chars()` instead keeps the multi-byte unit atomic: the OSC handler
/// consumes whole characters, so a UTF-8 char either escapes cleanly or is
/// dropped entirely — it can never be split across the boundary.
fn sanitize_pasted_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // ESC: consume a full ANSI/OSC sequence.
            '\u{1b}' => {
                match chars.peek().copied() {
                    Some('[') => {
                        chars.next(); // '['
                        while let Some(&n) = chars.peek() {
                            if ('\u{20}'..='\u{3F}').contains(&n) {
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        if let Some(&n) = chars.peek() {
                            if ('\u{40}'..='\u{7E}').contains(&n) {
                                chars.next();
                            }
                        }
                    }
                    Some(']') => {
                        chars.next(); // ']'
                        loop {
                            match chars.next() {
                                Some('\u{07}') | None => break,
                                Some('\u{1b}') => {
                                    if chars.peek() == Some(&'\\') {
                                        chars.next();
                                    }
                                    break;
                                }
                                Some(_) => continue,
                            }
                        }
                    }
                    _ => {
                        if let Some(&n) = chars.peek() {
                            if ('\u{40}'..='\u{5F}').contains(&n) {
                                chars.next();
                            }
                        }
                    }
                }
            }
            // Drop NUL, CR, DEL.
            '\0' | '\r' | '\u{7f}' => {}
            // Drop C0 controls except newline and tab.
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            // Keep everything else (printable ASCII + all multi-byte UTF-8).
            c => out.push(c),
        }
    }

    out
}

/// Map a raw crossterm event to a [`TermEvent`], or `None` to discard it.
///
/// Only key *presses* are forwarded: crossterm's Windows console backend emits
/// both `KeyEventKind::Press` and `::Release` for every keystroke (enhanced
/// terminals additionally emit `::Repeat`). Without this gate every binding
/// that bypasses the input textarea — command-palette navigation, chat scroll,
/// Enter-send, the Ctrl+C quit cascade — would fire twice per keystroke on
/// Windows. On Unix/legacy terminals only `Press` is emitted, so this is a
/// harmless no-op there.
fn map_event(ev: Event) -> Option<TermEvent> {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => Some(TermEvent::Key(key)),
        Event::Resize(_, _) => Some(TermEvent::Resize),
        Event::Paste(text) => Some(TermEvent::Paste(sanitize_pasted_text(&text))),
        Event::Mouse(m)
            if matches!(
                m.kind,
                crossterm::event::MouseEventKind::ScrollUp
                    | crossterm::event::MouseEventKind::ScrollDown
                    | crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
            ) =>
        {
            Some(TermEvent::Mouse(m))
        }
        Event::Mouse(_) => None,
        _ => None,
    }
}

/// Spawn a blocking task that polls crossterm events and sends them
/// through an mpsc channel. Returns the receiving end.
///
/// The task polls every 50ms. Key presses, resizes, pastes and consumable mouse
/// events are forwarded; high-rate mouse movement/drag/release records and
/// everything else crossterm reports (focus gained/lost, key releases) are
/// dropped by [`map_event`]. Mouse events also use a non-blocking queue send so
/// a burst cannot stall keyboard or paste delivery.
///
/// The task runs until the receiver is dropped (send returns Err).
pub fn spawn_event_collector() -> mpsc::Receiver<TermEvent> {
    let (tx, rx) = mpsc::channel(64);

    tokio::task::spawn_blocking(move || {
        let poll_timeout = Duration::from_millis(50);
        loop {
            // Poll with timeout so we can detect channel closure
            match event::poll(poll_timeout) {
                Ok(true) => {
                    if let Ok(ev) = event::read() {
                        if let Some(te) = map_event(ev) {
                            match te {
                                TermEvent::Mouse(_) => {
                                    // map_event filters non-consumable mouse input; this
                                    // non-blocking send is the second guard. Mouse events
                                    // are disposable transients, so a full queue must not
                                    // stall the collector and delay keyboard input.
                                    match tx.try_send(te) {
                                        Ok(()) => {}
                                        Err(mpsc::error::TrySendError::Full(_)) => {}
                                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                                    }
                                }
                                te => {
                                    // Keyboard, paste and resize events are not disposable:
                                    // preserve their ordering and reliability with a
                                    // blocking send. A closed receiver still ends collection.
                                    if tx.blocking_send(te).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(false) => {
                    // No event within timeout, continue polling
                }
                Err(e) => {
                    // crossterm poll error — unlikely, but log and exit gracefully
                    tracing::warn!("Terminal event poll error: {e}; stopping event collector");
                    break;
                }
            }
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn press_key_event_is_forwarded() {
        // KeyEvent::new defaults kind to Press.
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert!(matches!(
            map_event(Event::Key(key)),
            Some(TermEvent::Key(_))
        ));
    }

    #[test]
    fn release_and_repeat_key_events_are_dropped() {
        // Windows/enhanced terminals emit Release (and Repeat) alongside Press;
        // forwarding them would double-fire every non-textarea binding.
        let mut release = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        release.kind = KeyEventKind::Release;
        assert!(map_event(Event::Key(release)).is_none());

        let mut repeat = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        repeat.kind = KeyEventKind::Repeat;
        assert!(map_event(Event::Key(repeat)).is_none());
    }

    #[test]
    fn consumable_mouse_events_are_forwarded() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        for kind in [
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollDown,
            MouseEventKind::Down(MouseButton::Left),
        ] {
            let ev = MouseEvent {
                kind,
                column: 4,
                row: 9,
                modifiers: KeyModifiers::empty(),
            };
            assert!(matches!(
                map_event(Event::Mouse(ev)),
                Some(TermEvent::Mouse(m)) if m.column == 4 && m.row == 9
            ));
        }
    }

    #[test]
    fn unconsumed_mouse_events_are_dropped() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Right),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Right),
        ] {
            let ev = MouseEvent {
                kind,
                column: 4,
                row: 9,
                modifiers: KeyModifiers::empty(),
            };
            assert!(map_event(Event::Mouse(ev)).is_none());
        }
    }

    #[test]
    fn resize_and_paste_events_are_mapped() {
        assert!(matches!(
            map_event(Event::Resize(80, 24)),
            Some(TermEvent::Resize)
        ));
        assert!(matches!(
            map_event(Event::Paste("hi\nthere".to_string())),
            Some(TermEvent::Paste(s)) if s == "hi\nthere"
        ));
    }

    #[test]
    fn paste_is_sanitized_of_terminal_escape_sequences() {
        let ansi = "hello\x1b[31m world\x1b[0m";
        assert_eq!(
            sanitize_pasted_text(ansi),
            "hello world",
            "CSI color codes must be stripped"
        );

        let osc52 = "\x1b]52;c;Zm9vYmFy\x07plain";
        assert_eq!(
            sanitize_pasted_text(osc52),
            "plain",
            "OSC 52 must be stripped"
        );

        // (The expected literal here once dropped the 'b' by typo, shipping a
        // red test: the sanitizer drops NUL and CR, and keeps every printable.)
        let nul = "a\x00b\rc\td\ne";
        assert_eq!(
            sanitize_pasted_text(nul),
            "abc\td\ne",
            "NUL/CR dropped, tab/newline kept"
        );
    }
}
