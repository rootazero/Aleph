// Terminal event collection.
//
// Spawns a blocking task that polls crossterm events every 50ms and
// forwards Key/Resize events to an async mpsc channel. The main loop
// receives from this channel alongside gateway events.

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
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
}

/// Strip ANSI/OSC escape sequences and non-printing control characters from
/// text delivered by the terminal's bracketed-paste mechanism.
///
/// Crossterm forwards the raw bytes the terminal reports, so a paste that
/// contains terminal control sequences can otherwise drive the cursor, clear
/// the screen, or inject OSC 52 clipboard requests. We keep only printable
/// text, horizontal tab, and newline; everything else is dropped.
fn sanitize_pasted_text(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes().peekable();

    while let Some(b) = bytes.next() {
        match b {
            // ESC: consume a full ANSI/OSC sequence.
            0x1b => {
                match bytes.peek() {
                    Some(&b'[') => {
                        bytes.next(); // '['
                        while let Some(&n) = bytes.peek() {
                            if (0x20..=0x3F).contains(&n) {
                                bytes.next();
                            } else {
                                break;
                            }
                        }
                        if let Some(&n) = bytes.peek() {
                            if (0x40..=0x7E).contains(&n) {
                                bytes.next();
                            }
                        }
                    }
                    Some(&b']') => {
                        bytes.next(); // ']'
                        loop {
                            match bytes.next() {
                                Some(0x07) | None => break,
                                Some(0x1b) => {
                                    if bytes.peek() == Some(&b'\\') {
                                        bytes.next();
                                    }
                                    break;
                                }
                                Some(_) => continue,
                            }
                        }
                    }
                    _ => {
                        if let Some(&n) = bytes.peek() {
                            if (0x40..=0x5F).contains(&n) {
                                bytes.next();
                            }
                        }
                    }
                }
            }
            // Drop NUL and CR.
            0x00 | 0x0d => {}
            // Drop DEL and other C0 controls except newline and tab.
            b if (b < 0x20 && b != b'\n' && b != b'\t') || b == 0x7f => {}
            // Keep everything else (ASCII printable and UTF-8 continuation bytes).
            b => out.push(b),
        }
    }

    String::from_utf8(out).unwrap_or_default()
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
        _ => None,
    }
}

/// Spawn a blocking task that polls crossterm events and sends them
/// through an mpsc channel. Returns the receiving end.
///
/// The task polls every 50ms. Only Key and Resize events are forwarded;
/// mouse events and other crossterm events are silently discarded.
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
                            // If send fails, receiver was dropped — exit the loop
                            if tx.blocking_send(te).is_err() {
                                break;
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
