//! Async spinner with Drop-guard cleanup.
//!
//! Use case: wrap a long-running RPC call (WebSocket connect, big query)
//! so users see live progress instead of a frozen terminal.
//!
//! ```ignore
//! let _spin = Spinner::start("Connecting to gateway");
//! let client = AlephClient::connect(url).await?;
//! // _spin clears the line when it drops.
//! ```
//!
//! Spinner is a **no-op** when:
//! - stdout is not a TTY (piped)
//! - colour is disabled (`NO_COLOR` / `ALEPH_COLOR=never`) → still ticks but
//!   uses ASCII frames
//! - `--json` mode (caller should not construct one in that path)

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::icon::use_unicode;
use super::theme::{paint, Style};

const BRAILLE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ASCII_FRAMES: &[&str] = &["|", "/", "-", "\\"];
const FRAME_INTERVAL: Duration = Duration::from_millis(80);

/// RAII handle for an active spinner. Drop or call [`Spinner::stop`] to
/// clear the line.
pub struct Spinner {
    stop_signal: Arc<Notify>,
    join: Option<JoinHandle<()>>,
    active: bool,
}

impl Spinner {
    /// Start a spinner with `message`. Returns a no-op handle when stderr
    /// is not a TTY — safe to drop with no visible side effect.
    pub fn start(message: impl Into<String>) -> Self {
        let message = message.into();
        // We render to stderr so spinner doesn't pollute stdout (which may
        // be consumed by --json mode or shell pipelines).
        let active = std::io::stderr().is_terminal();
        if !active {
            return Self {
                stop_signal: Arc::new(Notify::new()),
                join: None,
                active: false,
            };
        }
        let stop_signal = Arc::new(Notify::new());
        let signal = stop_signal.clone();
        let frames: &'static [&'static str] = if use_unicode() {
            BRAILLE_FRAMES
        } else {
            ASCII_FRAMES
        };

        let join = tokio::spawn(async move {
            let mut idx = 0usize;
            loop {
                {
                    let mut err = std::io::stderr().lock();
                    // \r returns cursor; \x1b[K clears to end-of-line.
                    let frame = frames[idx % frames.len()];
                    let painted = paint(Style::Info, frame);
                    let _ = write!(err, "\r{painted} {message}\x1b[K");
                    let _ = err.flush();
                }
                idx = idx.wrapping_add(1);
                tokio::select! {
                    () = signal.notified() => break,
                    () = tokio::time::sleep(FRAME_INTERVAL) => {}
                }
            }
            // Clear the line on exit.
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[K");
            let _ = err.flush();
        });

        Self {
            stop_signal,
            join: Some(join),
            active: true,
        }
    }

    /// Stop the spinner explicitly and await its cleanup. Safe to call
    /// twice; subsequent calls are no-ops.
    pub async fn stop(mut self) {
        if !self.active {
            return;
        }
        self.stop_signal.notify_one();
        if let Some(handle) = self.join.take() {
            let _ = handle.await;
        }
        self.active = false;
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.stop_signal.notify_one();
        // Don't abort: the spawned task owns its own terminal cleanup
        // (clear-line on stderr) and will exit naturally when the signal
        // fires. Aborting here kills the task before it can clean up,
        // leaving the spinner glyph and message on stderr.
        let _ = self.join.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spinner_can_be_dropped_without_tty() {
        // In CI / `cargo test` stderr is rarely a TTY so the spinner is
        // inert. Just confirm we can spawn + drop without panic / deadlock.
        let s = Spinner::start("test");
        assert!(!s.active);
        drop(s);
    }

    #[tokio::test]
    async fn spinner_stop_is_idempotent_when_inactive() {
        let s = Spinner::start("test");
        s.stop().await; // should not deadlock when inactive
    }
}
