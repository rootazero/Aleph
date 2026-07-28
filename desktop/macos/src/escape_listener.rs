//! Global Escape key listener for aborting AI desktop control.
//!
//! # Why this is a `CGEventTap` on its own thread, and not an `NSEvent` monitor
//!
//! This used to install `NSEvent::addGlobalMonitorForEventsMatchingMask_handler`
//! (plus the local variant). Both installed fine and both **never fired** in
//! `aleph-server`, so the user's emergency stop was dead while every layer above
//! reported it armed — the worst shape a safety control can take.
//!
//! `NSEvent` monitors are an AppKit facility: the handler is dispatched from the
//! **main** run loop of a real `NSApplication`. `aleph-server` is a headless
//! daemon whose main thread is owned by the tokio runtime; it never builds an
//! `NSApp` and never services a main `CFRunLoop`. Measured on macOS 27 in an
//! AX-trusted process, synthesizing a key on the HID tap:
//!
//! | listener | fired |
//! |---|---|
//! | `NSEvent` global monitor, installed off-main, no main run loop | **no** |
//! | `NSEvent` global monitor, `NSApplication.shared` + main run loop pumped | **no** |
//! | `CGEventTap` (listen-only) on a dedicated thread running its own `CFRunLoop` | **yes** |
//!
//! So the tap owns its thread and its run loop, and depends on nothing about how
//! the host process schedules its main thread.
//!
//! # The tap never swallows a key
//!
//! It is created with `kCGEventTapOptionListenOnly` and the callback returns the
//! event unchanged. A tap that can consume events would let one wedged listener
//! make the Escape key stop working machine-wide — the exact regression the
//! Windows keyboard hook shipped once (see `WINDOWS_RUNTIME.md` ⑦). Listen-only
//! makes that unrepresentable rather than merely avoided.
//!
//! # Failure is reported, not swallowed
//!
//! `CGEventTapCreate` returns NULL for a process without Accessibility (or Input
//! Monitoring) rights. That is the whole feature being unavailable, so `start()`
//! returns an error the tool layer logs, instead of the previous "warn and claim
//! success".

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopSource};
use tracing::{debug, warn};

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::{DesktopError, Result};

// ---------------------------------------------------------------------------
// C FFI — CoreGraphics event taps + Accessibility trust
// ---------------------------------------------------------------------------

type CFMachPortRef = *const c_void;
type CGEventRef = *const c_void;
type CGEventTapProxy = *const c_void;
type CGEventTapCallBack =
    extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> core_foundation::runloop::CFRunLoopSourceRef;
    fn CFRelease(cf: *const c_void);
}

/// `kCGHIDEventTap` — the earliest point in the stream, so a key is seen even
/// when the app that would consume it is unresponsive.
const TAP_HID: u32 = 0;
/// `kCGHeadInsertEventTap`.
const PLACE_HEAD_INSERT: u32 = 0;
/// `kCGEventTapOptionListenOnly` — cannot modify or drop events. Load-bearing.
const OPTION_LISTEN_ONLY: u32 = 1;
/// `kCGEventKeyDown`.
const EVENT_KEY_DOWN: u32 = 10;
/// `kCGEventTapDisabledByTimeout` — the system may disable a tap; re-arm it.
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
/// `kCGEventTapDisabledByUserInput`.
const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
/// `kCGKeyboardEventKeycode`.
const FIELD_KEYCODE: u32 = 9;
/// Virtual key code for the Escape key on macOS.
const ESCAPE_KEY_CODE: i64 = 53;

/// Only key-down is of interest. The two `tapDisabled*` notifications are
/// delivered regardless of the mask, so they are deliberately not in it.
const KEY_DOWN_MASK: u64 = 1 << EVENT_KEY_DOWN;

// ---------------------------------------------------------------------------
// Tap callback context
// ---------------------------------------------------------------------------

/// What the C callback is handed. Lives on the tap thread's heap and is freed
/// only after that thread's run loop has returned, so the pointer the tap holds
/// is valid for the tap's whole life.
struct TapContext {
    abort_flag: Arc<AtomicBool>,
    /// The tap's own mach port, needed to re-enable it after the system disables
    /// it. Written just after creation; the callback tolerates the null it may
    /// briefly observe (a tap is enabled from birth, so it can fire first).
    port: AtomicPtr<c_void>,
}

/// The tap callback. Runs on the tap thread; does nothing but set an atomic and
/// hand the event straight back.
extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if user_info.is_null() {
        return event;
    }
    // SAFETY: `user_info` is the `TapContext` pointer passed to `CGEventTapCreate`
    // on the tap thread; it is freed only after that thread's run loop returns,
    // which happens strictly after the tap is disabled and its source removed.
    let ctx = unsafe { &*user_info.cast::<TapContext>() };

    match event_type {
        EVENT_KEY_DOWN => {
            // SAFETY: `event` is the live CGEvent the tap was handed for the
            // duration of this callback; the field id is the documented constant.
            let code = unsafe { CGEventGetIntegerValueField(event, FIELD_KEYCODE) };
            if code == ESCAPE_KEY_CODE {
                ctx.abort_flag.store(true, Ordering::Release);
            }
        }
        EVENT_TAP_DISABLED_BY_TIMEOUT | EVENT_TAP_DISABLED_BY_USER_INPUT => {
            // A disabled tap is a silently dead abort key. Re-arm it.
            let port = ctx.port.load(Ordering::Acquire);
            if !port.is_null() {
                // SAFETY: `port` is the CFMachPort this tap was created with,
                // alive until the thread tears it down after the run loop exits.
                unsafe { CGEventTapEnable(port.cast_const(), true) };
            }
        }
        _ => {}
    }

    // Pass the event through unchanged. Listen-only taps ignore the return value,
    // but returning the event is the contract and keeps this honest if the option
    // ever changes.
    event
}

// ---------------------------------------------------------------------------
// EscapeListener
// ---------------------------------------------------------------------------

/// The tap thread, and the handle needed to ask it to stop.
///
/// `CFRunLoopStop` is the one `CFRunLoop` entry point documented as safe to call
/// from another thread, and `CFRunLoop` is `Send + Sync` for exactly that reason.
struct TapThread {
    run_loop: CFRunLoop,
    join: JoinHandle<()>,
}

/// macOS implementation of [`EscapeAbort`] backed by a listen-only `CGEventTap`.
pub struct EscapeListener {
    abort_flag: Arc<AtomicBool>,
    /// `Mutex` so `start`/`stop` can run through `&self` (the trait's shape).
    thread: Mutex<Option<TapThread>>,
}

impl EscapeListener {
    /// Create a new `EscapeListener` (not yet active).
    pub fn new() -> Self {
        Self {
            abort_flag: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        }
    }

    /// Body of the tap thread: build the tap, publish the run loop to the
    /// caller, then service events until [`EscapeListener::stop`] stops us.
    fn run_tap(
        abort_flag: Arc<AtomicBool>,
        ready: &mpsc::Sender<std::result::Result<CFRunLoop, String>>,
    ) {
        let ctx = Box::into_raw(Box::new(TapContext {
            abort_flag,
            port: AtomicPtr::new(ptr::null_mut()),
        }));

        // SAFETY: all arguments are the documented constants above; `ctx` is a
        // live `TapContext` owned by this thread for longer than the tap.
        let port = unsafe {
            CGEventTapCreate(
                TAP_HID,
                PLACE_HEAD_INSERT,
                OPTION_LISTEN_ONLY,
                KEY_DOWN_MASK,
                tap_callback,
                ctx.cast(),
            )
        };
        if port.is_null() {
            let _ = ready.send(Err(
                "CGEventTapCreate returned NULL — grant Accessibility (or Input Monitoring) \
                 rights to this binary in System Settings › Privacy & Security"
                    .to_string(),
            ));
            // SAFETY: `ctx` came from `Box::into_raw` above and was never shared:
            // the tap that would have held it does not exist.
            drop(unsafe { Box::from_raw(ctx) });
            return;
        }
        // SAFETY: `ctx` is still solely owned here; the tap may already be
        // calling back, which is why the field is atomic.
        unsafe { (*ctx).port.store(port.cast_mut(), Ordering::Release) };

        // SAFETY: `port` is a live CFMachPort; a NULL allocator means the default.
        let source_ref = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), port, 0) };
        if source_ref.is_null() {
            let _ = ready.send(Err(
                "CFMachPortCreateRunLoopSource failed for the event tap".to_string(),
            ));
            // SAFETY: `port` is a +1 reference from `CGEventTapCreate`.
            unsafe { CFRelease(port) };
            // SAFETY: `ctx` came from `Box::into_raw`; the tap is being dropped
            // and was never added to a run loop, so no callback can be in flight.
            drop(unsafe { Box::from_raw(ctx) });
            return;
        }
        // SAFETY: `source_ref` is a +1 reference from a Create-rule call.
        let source = unsafe { CFRunLoopSource::wrap_under_create_rule(source_ref) };

        let run_loop = CFRunLoop::get_current();
        // SAFETY: reading the framework's mode constant.
        let mode = unsafe { kCFRunLoopCommonModes };
        run_loop.add_source(&source, mode);
        // SAFETY: `port` is the live tap.
        unsafe { CGEventTapEnable(port, true) };

        debug!("Escape event tap armed on its own run loop");
        if ready.send(Ok(run_loop.clone())).is_err() {
            // The caller went away before we finished arming; unwind rather than
            // leave a thread parked in a run loop nobody can stop.
            run_loop.remove_source(&source, mode);
            // SAFETY: the tap is live and owned here.
            unsafe {
                CGEventTapEnable(port, false);
                CFRelease(port);
            }
            // SAFETY: `ctx` came from `Box::into_raw`; the tap is disabled and
            // its source removed, so no callback can still be running.
            drop(unsafe { Box::from_raw(ctx) });
            return;
        }

        CFRunLoop::run_current();

        // Torn down only after the run loop has returned, so the ordering is:
        // disable the tap → drop its source → release the port → free the
        // context the callback reads.
        // SAFETY: `port` is still the live tap created above.
        unsafe { CGEventTapEnable(port, false) };
        run_loop.remove_source(&source, mode);
        drop(source);
        // SAFETY: `port` is the +1 reference from `CGEventTapCreate`.
        unsafe { CFRelease(port) };
        // SAFETY: `ctx` came from `Box::into_raw`; the tap is disabled and
        // unregistered, so the callback can no longer be entered.
        drop(unsafe { Box::from_raw(ctx) });
        debug!("Escape event tap torn down");
    }
}

impl Default for EscapeListener {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeAbort for EscapeListener {
    fn start(&self) -> Result<()> {
        // Held across check-and-install so a concurrent stop() cannot slot in
        // between "not running" and "thread stored".
        let mut slot = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            debug!("EscapeListener already active, skipping start");
            return Ok(());
        }

        // SAFETY: reads whether this process is AX-trusted; never prompts.
        if !unsafe { AXIsProcessTrusted() } {
            // `CGEventTapCreate` would return NULL below anyway, but naming the
            // cause is the difference between a fixable message and a mystery.
            return Err(DesktopError::NotAvailable(
                "escape abort listener needs Accessibility permission (AXIsProcessTrusted is \
                 false); until it is granted, pressing Escape will not stop desktop actions"
                    .to_string(),
            ));
        }

        let (tx, rx) = mpsc::channel();
        let flag = Arc::clone(&self.abort_flag);
        let join = std::thread::Builder::new()
            .name("aleph-escape-tap".into())
            .spawn(move || Self::run_tap(flag, &tx))
            .map_err(|e| {
                DesktopError::InputFailed(format!("failed to spawn escape tap thread: {e}"))
            })?;

        match rx.recv() {
            Ok(Ok(run_loop)) => {
                *slot = Some(TapThread { run_loop, join });
                Ok(())
            }
            Ok(Err(reason)) => {
                let _ = join.join();
                Err(DesktopError::NotAvailable(reason))
            }
            // The thread died without reporting — treat as unavailable rather
            // than hang or claim an armed listener.
            Err(_) => {
                let _ = join.join();
                Err(DesktopError::InputFailed(
                    "escape tap thread exited before arming".to_string(),
                ))
            }
        }
    }

    fn stop(&self) {
        let taken = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(TapThread { run_loop, join }) = taken else {
            return;
        };
        run_loop.stop();
        if join.join().is_err() {
            warn!("escape tap thread panicked during shutdown");
        }
        debug!("Escape listener stopped");
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

    /// `start` is re-entrant, and — the part that used to be untrue — when it
    /// cannot arm the tap it says so instead of returning `Ok(())`.
    ///
    /// Both outcomes are legitimate here: CI runners are not AX-trusted, a
    /// developer machine may be. What must hold either way is that a *reported*
    /// success is a real one, so the second call is only required to agree with
    /// the first.
    #[test]
    fn start_is_reentrant_and_honest_about_failure() {
        let listener = EscapeListener::new();
        let first = listener.start();
        let second = listener.start();
        assert_eq!(
            first.is_ok(),
            second.is_ok(),
            "start must not flip between armed and unavailable: {first:?} / {second:?}"
        );
        if let Err(e) = &first {
            // An unavailable listener must name the permission, not go quiet.
            let msg = e.to_string().to_lowercase();
            assert!(
                msg.contains("accessibility") || msg.contains("input monitoring"),
                "failure must point at the missing grant: {msg}"
            );
        }
        listener.stop();
    }

    /// Stopping an armed listener must actually retire the thread, and stopping
    /// a stopped one must be a no-op rather than a panic or a hang.
    #[test]
    fn stop_is_idempotent_and_joins_the_tap_thread() {
        let listener = EscapeListener::new();
        let _ = listener.start();
        listener.stop();
        assert!(
            listener
                .thread
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none(),
            "stop must clear the thread slot"
        );
        listener.stop();
    }
}
