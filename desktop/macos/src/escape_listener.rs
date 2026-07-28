//! Global Escape key listener for aborting AI desktop control.
//!
//! Uses `NSEvent::addGlobalMonitorForEventsMatchingMask_handler` (unfocused)
//! and `NSEvent::addLocalMonitorForEventsMatchingMask_handler` (focused) to
//! detect Escape key presses system-wide.
//!
//! Requires Accessibility permission (`AXIsProcessTrusted`); without it the
//! global monitor silently receives no events — the listener degrades
//! gracefully by logging a warning and returning `Ok(())`.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
use tracing::{debug, warn};

use aleph_desktop::platform::EscapeAbort;

// ---------------------------------------------------------------------------
// C FFI — Accessibility check
// ---------------------------------------------------------------------------

extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Virtual key code for the Escape key on macOS.
const ESCAPE_KEY_CODE: u16 = 53;

// ---------------------------------------------------------------------------
// Send+Sync wrapper for monitor handles
// ---------------------------------------------------------------------------

/// Serialises every `NSEvent` monitor install/remove in this process.
///
/// AppKit's event-monitor registry is *process-global*: both
/// `+[NSEvent addGlobalMonitorForEventsMatchingMask:handler:]` and
/// `+[NSEvent removeMonitor:]` end up in HIToolbox's `SyncEventMonitorMask()`,
/// which rebuilds one shared CF table and is not thread-safe. Each listener's
/// own `monitors` mutex guards only *its* handles, so two listeners dropped on
/// two threads raced inside `SyncEventMonitorMask()` and over-released a CF
/// object — `EXC_BREAKPOINT` in `CFRelease`, reproducible at roughly 1 run in 6
/// of the desktop test module. The lock has to span the whole process because
/// that is the scope of the table being mutated.
static MONITOR_REGISTRY: Mutex<()> = Mutex::new(());

/// Wrapper around `Retained<AnyObject>` that implements `Send + Sync`.
///
/// # Safety
///
/// `NSEvent` monitor handles are reference-counted `ObjC` objects, and
/// `Retained`'s retain/release is itself thread-safe. Handing one to
/// `NSEvent::removeMonitor` is *not* thread-safe, so every such call is
/// serialised through [`MONITOR_REGISTRY`]; the handles themselves are only
/// ever reached through our `Mutex`-guarded `Vec`.
struct MonitorHandle(Retained<AnyObject>);

// SAFETY: NSEvent monitor handles are reference-counted ObjC objects whose
// retain/release is thread-safe. We only access them behind a Mutex.
unsafe impl Send for MonitorHandle {}
unsafe impl Sync for MonitorHandle {}

// ---------------------------------------------------------------------------
// EscapeListener
// ---------------------------------------------------------------------------

/// macOS implementation of [`EscapeAbort`] using `NSEvent` monitors.
///
/// Listens for Escape key presses (keycode 53) and sets an atomic flag that
/// can be polled by the computer-use loop.
pub struct EscapeListener {
    abort_flag: Arc<AtomicBool>,
    /// Monitor handles returned by `NSEvent`; kept alive for cleanup.
    /// Wrapped in `Mutex` so `start`/`stop` can be called through `&self`
    /// (required by the `EscapeAbort` trait).
    monitors: Mutex<Vec<MonitorHandle>>,
}

impl EscapeListener {
    /// Create a new `EscapeListener` (not yet active).
    pub fn new() -> Self {
        Self {
            abort_flag: Arc::new(AtomicBool::new(false)),
            monitors: Mutex::new(Vec::new()),
        }
    }

    /// Build the global monitor block that sets `abort_flag` on Escape.
    fn build_global_block(flag: &Arc<AtomicBool>) -> RcBlock<dyn Fn(NonNull<NSEvent>)> {
        let flag = Arc::clone(flag);
        RcBlock::new(move |event_ptr: NonNull<NSEvent>| {
            // SAFETY: NSEvent monitor handlers are guaranteed to receive a valid
            // NSEvent pointer from the system for the lifetime of the closure invocation.
            let event = unsafe { event_ptr.as_ref() };
            if event.r#type() == NSEventType::KeyDown && event.keyCode() == ESCAPE_KEY_CODE {
                flag.store(true, Ordering::Release);
                debug!("Escape key detected (global) — abort flag set");
            }
        })
    }

    /// Build the local monitor block that sets `abort_flag` on Escape.
    fn build_local_block(
        flag: &Arc<AtomicBool>,
    ) -> RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> {
        let flag = Arc::clone(flag);
        RcBlock::new(move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: NSEvent monitor handlers are guaranteed to receive a valid
            // NSEvent pointer from the system for the lifetime of the closure invocation.
            let event = unsafe { event_ptr.as_ref() };
            if event.r#type() == NSEventType::KeyDown && event.keyCode() == ESCAPE_KEY_CODE {
                flag.store(true, Ordering::Release);
                debug!("Escape key detected (local) — abort flag set");
            }
            // Pass the event through unchanged.
            event_ptr.as_ptr()
        })
    }
}

impl EscapeAbort for EscapeListener {
    fn start(&self) -> aleph_desktop::Result<()> {
        // Hold the lock across the entire check-and-install sequence to prevent
        // a race where another thread calls stop() between the emptiness check
        // and monitor installation.
        let mut monitors = self
            .monitors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Re-entrant: if already active, return immediately.
        if !monitors.is_empty() {
            debug!("EscapeListener already active, skipping start");
            return Ok(());
        }

        // Accessibility check — degrade gracefully.
        // SAFETY: ApplicationServices C function taking no arguments; it only
        // reads whether this process is currently AX-trusted and never prompts.
        let trusted = unsafe { AXIsProcessTrusted() };
        if !trusted {
            warn!(
                "Accessibility permission not granted (AXIsProcessTrusted = false). \
                 Escape listener global monitor will not receive events."
            );
            // Don't block — the local monitor still works when the app is focused.
        }

        let mask = NSEventMask::KeyDown;

        // Installing a monitor rewrites the process-global HIToolbox table;
        // see [`MONITOR_REGISTRY`]. Always taken *after* `self.monitors` so the
        // two locks keep one order everywhere and cannot deadlock.
        let _registry = MONITOR_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Global monitor (app not focused).
        let global_block = Self::build_global_block(&self.abort_flag);
        if let Some(monitor) =
            NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block)
        {
            debug!("Escape global monitor installed");
            monitors.push(MonitorHandle(monitor));
        } else {
            warn!("Failed to install escape global monitor");
        }

        // Local monitor (app focused).
        let local_block = Self::build_local_block(&self.abort_flag);
        // SAFETY: Our block always returns the event pointer unchanged (pass-through).
        if let Some(monitor) =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) }
        {
            debug!("Escape local monitor installed");
            monitors.push(MonitorHandle(monitor));
        } else {
            warn!("Failed to install escape local monitor");
        }

        Ok(())
    }

    fn stop(&self) {
        let mut monitors = self
            .monitors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Same process-global table as `start`, same lock order.
        let _registry = MONITOR_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for handle in monitors.drain(..) {
            // SAFETY: monitors were returned by addGlobal/LocalMonitor.
            unsafe { NSEvent::removeMonitor(&handle.0) };
        }
        debug!("Escape monitors removed");
    }

    fn is_aborted(&self) -> bool {
        self.abort_flag.load(Ordering::Acquire)
    }

    fn reset(&self) {
        self.abort_flag.store(false, Ordering::Release);
    }
}

impl Drop for EscapeListener {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_listener_is_not_aborted() {
        let listener = EscapeListener::new();
        assert!(!listener.is_aborted());
    }

    #[test]
    fn reset_clears_abort_flag() {
        let listener = EscapeListener::new();
        listener.abort_flag.store(true, Ordering::Release);
        assert!(listener.is_aborted());
        listener.reset();
        assert!(!listener.is_aborted());
    }

    /// Two listeners installing/removing monitors on two threads used to corrupt
    /// AppKit's process-global monitor table — an over-release inside
    /// `SyncEventMonitorMask()`, surfacing as `EXC_BREAKPOINT` in `CFRelease`
    /// and killing the whole test binary. [`MONITOR_REGISTRY`] is what makes
    /// this safe: with it removed this aborted 10 runs out of 30 (SIGABRT /
    /// SIGTRAP / SIGSEGV), and with it 0 out of 40.
    ///
    /// Without a window server (headless CI) no monitor ever installs, so the
    /// loop is a cheap no-op there and proves nothing — it only bites on a real
    /// desktop session, which is exactly where the crash lived.
    #[test]
    fn concurrent_start_stop_across_threads_does_not_corrupt_the_monitor_table() {
        let workers: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..25 {
                        let listener = EscapeListener::new();
                        let _ = listener.start();
                        listener.stop();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker
                .join()
                .expect("no thread may die installing or removing event monitors");
        }
    }

    #[test]
    fn start_is_reentrant() {
        let listener = EscapeListener::new();
        // First start installs monitors (may fail without accessibility,
        // but should not panic).
        let _ = listener.start();
        // Second start should return Ok immediately.
        let result = listener.start();
        assert!(result.is_ok());
        listener.stop();
    }
}
