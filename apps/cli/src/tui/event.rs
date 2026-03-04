// Terminal event collection.
//
// Spawns a blocking task that polls crossterm events every 50ms and
// forwards Key/Resize events to an async mpsc channel. The main loop
// receives from this channel alongside gateway events.

use crossterm::event::{self, Event, KeyEvent};
use std::time::Duration;
use tokio::sync::mpsc;

/// Terminal events relevant to the TUI.
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// A keyboard event
    Key(KeyEvent),
    /// Terminal was resized to (columns, rows)
    Resize(u16, u16),
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
                        let term_event = match ev {
                            Event::Key(key) => Some(TermEvent::Key(key)),
                            Event::Resize(cols, rows) => Some(TermEvent::Resize(cols, rows)),
                            _ => None,
                        };
                        if let Some(te) = term_event {
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
                Err(_) => {
                    // crossterm poll error — unlikely, but exit gracefully
                    break;
                }
            }
        }
    });

    rx
}
