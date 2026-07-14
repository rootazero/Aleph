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
        Event::Paste(text) => Some(TermEvent::Paste(text)),
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
}
