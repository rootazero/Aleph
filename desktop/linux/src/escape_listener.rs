//! Linux Escape-abort listener backed by a filesystem sentinel.
//!
//! Linux has no portable global-hotkey API: X11 needs a live display
//! connection plus a dedicated event-loop thread, and Wayland deliberately
//! denies global key grabs to ordinary clients. Rather than ship a fragile,
//! compositor-specific keyboard hook (and rather than the previous silent
//! no-op stub, which made `start()` claim success while providing no abort
//! path at all), the Linux listener uses a **sentinel file** the user can
//! create from any terminal to abort a running desktop-automation session:
//!
//! ```sh
//! touch ~/.aleph/desktop-abort
//! ```
//!
//! [`is_aborted`](EscapeAbort::is_aborted) reports `true` once the sentinel
//! exists; [`reset`](EscapeAbort::reset) and [`stop`](EscapeAbort::stop)
//! remove it. This works identically under X11, Wayland and headless setups,
//! and needs no extra dependency.

use std::path::PathBuf;

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::Result;
use tracing::{debug, warn};

/// Sentinel file, relative to `$HOME`, that signals an abort request.
const SENTINEL_REL_PATH: &str = ".aleph/desktop-abort";

/// Linux implementation of [`EscapeAbort`] using a filesystem sentinel file.
pub struct LinuxEscapeListener {
    /// Absolute path of the abort sentinel; `None` when `$HOME` is unset, in
    /// which case the listener degrades to a no-op (never aborts).
    sentinel: Option<PathBuf>,
}

impl LinuxEscapeListener {
    /// Create a listener watching `$HOME/.aleph/desktop-abort`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sentinel: resolve_sentinel_path(),
        }
    }

    /// Test constructor: watch an explicit (or absent) sentinel path instead
    /// of resolving `$HOME`.
    #[cfg(test)]
    fn for_test(sentinel: Option<PathBuf>) -> Self {
        Self { sentinel }
    }

    /// Remove the sentinel file if present; a missing file is not an error.
    fn remove_sentinel(&self) {
        if let Some(path) = &self.sentinel {
            match std::fs::remove_file(path) {
                Ok(()) => debug!(path = %path.display(), "desktop-abort sentinel cleared"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warn!(
                    error = %e,
                    path = %path.display(),
                    "Failed to remove desktop-abort sentinel"
                ),
            }
        }
    }
}

/// Resolve `$HOME/.aleph/desktop-abort`, or `None` when `$HOME` is unset.
fn resolve_sentinel_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(SENTINEL_REL_PATH))
}

impl Default for LinuxEscapeListener {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeAbort for LinuxEscapeListener {
    fn start(&self) -> Result<()> {
        if let Some(path) = &self.sentinel {
            // Clear any stale sentinel left over from a previous run so a
            // fresh session does not start out already "aborted".
            self.remove_sentinel();
            debug!(
                path = %path.display(),
                "Linux escape listener armed — `touch` this file to abort desktop control"
            );
        } else { warn!("HOME is unset — Linux desktop-abort sentinel is unavailable") }
        Ok(())
    }

    fn stop(&self) {
        self.remove_sentinel();
    }

    fn is_aborted(&self) -> bool {
        self.sentinel.as_ref().is_some_and(|p| p.exists())
    }

    fn reset(&self) {
        self.remove_sentinel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp path per test so parallel runs never collide.
    fn temp_sentinel(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aleph-escape-{}-{}", std::process::id(), tag));
        p
    }

    #[test]
    fn not_aborted_when_no_sentinel() {
        let path = temp_sentinel("absent");
        let _ = std::fs::remove_file(&path);
        let listener = LinuxEscapeListener::for_test(Some(path));
        listener.start().unwrap();
        assert!(!listener.is_aborted());
    }

    #[test]
    fn aborted_after_sentinel_created() {
        let path = temp_sentinel("created");
        let listener = LinuxEscapeListener::for_test(Some(path.clone()));
        listener.start().unwrap();
        assert!(!listener.is_aborted());

        std::fs::write(&path, b"abort").unwrap();
        assert!(listener.is_aborted());

        listener.stop(); // cleanup
        assert!(!path.exists());
    }

    #[test]
    fn reset_clears_the_sentinel() {
        let path = temp_sentinel("reset");
        let listener = LinuxEscapeListener::for_test(Some(path.clone()));
        listener.start().unwrap();

        std::fs::write(&path, b"abort").unwrap();
        assert!(listener.is_aborted());

        listener.reset();
        assert!(!listener.is_aborted());
        assert!(!path.exists());
    }

    #[test]
    fn start_clears_stale_sentinel() {
        let path = temp_sentinel("stale");
        // A leftover sentinel from a previous run must not abort a fresh session.
        std::fs::write(&path, b"stale").unwrap();
        let listener = LinuxEscapeListener::for_test(Some(path.clone()));
        listener.start().unwrap();
        assert!(!listener.is_aborted());
        assert!(!path.exists());
    }

    #[test]
    fn missing_home_degrades_to_no_op() {
        let listener = LinuxEscapeListener::for_test(None);
        listener.start().unwrap();
        assert!(!listener.is_aborted());
        listener.reset();
        listener.stop();
    }
}
