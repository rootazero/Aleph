//! End-to-end proof that the Escape abort listener actually observes a key
//! press — the property that silently did **not** hold while the listener was
//! built on `NSEvent` monitors (they install fine in a daemon and never fire).
//!
//! `#[ignore]` because it synthesizes a real Escape key press on the machine
//! running it, and because it needs Accessibility rights. Run it deliberately:
//!
//! ```text
//! cargo test -p aleph-desktop-macos --test escape_listener_e2e -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::time::{Duration, Instant};

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop_macos::EscapeListener;

type CFTypeRef = *const c_void;

/// `kCGEventSourceStateHIDSystemState`.
const SOURCE_HID_SYSTEM_STATE: i32 = 1;
/// `kCGHIDEventTap`.
const TAP_HID: u32 = 0;
/// Virtual key code for Escape.
const ESCAPE_KEY_CODE: u16 = 53;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn CGEventSourceCreate(state_id: i32) -> CFTypeRef;
    fn CGEventCreateKeyboardEvent(source: CFTypeRef, key: u16, key_down: bool) -> CFTypeRef;
    fn CGEventPost(tap: u32, event: CFTypeRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
}

/// Press and release Escape on the global HID stream, exactly as a user would.
fn press_escape() {
    // SAFETY: the state id is the documented constant; a NULL source is a valid
    // (if less precise) argument to `CGEventCreateKeyboardEvent`, so a failed
    // create is tolerated by the calls below.
    unsafe {
        let source = CGEventSourceCreate(SOURCE_HID_SYSTEM_STATE);
        for down in [true, false] {
            let event = CGEventCreateKeyboardEvent(source, ESCAPE_KEY_CODE, down);
            if !event.is_null() {
                CGEventPost(TAP_HID, event);
                CFRelease(event);
            }
        }
        if !source.is_null() {
            CFRelease(source);
        }
    }
}

/// Poll `is_aborted` for up to `budget`, so the assertion does not race the
/// event's trip through the window server.
fn aborted_within(listener: &EscapeListener, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if listener.is_aborted() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    listener.is_aborted()
}

#[test]
#[ignore = "presses Escape on the real machine and needs Accessibility rights"]
fn an_escape_press_raises_the_abort_flag_and_stop_disarms_it() {
    // SAFETY: reads the process's AX trust; never prompts.
    if !unsafe { AXIsProcessTrusted() } {
        eprintln!("skipped: this binary is not Accessibility-trusted");
        return;
    }

    let listener = EscapeListener::new();
    listener
        .start()
        .expect("escape listener must arm on an AX-trusted process");
    // The tap needs a moment to be wired into its run loop before it can see
    // anything; without this the test would race its own key press.
    std::thread::sleep(Duration::from_millis(300));

    assert!(!listener.is_aborted(), "flag must start clear");
    press_escape();
    assert!(
        aborted_within(&listener, Duration::from_secs(3)),
        "the Escape press was not observed — the abort key is dead, which is \
         exactly the regression this test exists for"
    );

    // A stopped listener must stop listening: the tap is torn down, so a later
    // Escape must not resurrect the flag.
    listener.reset();
    listener.stop();
    press_escape();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !listener.is_aborted(),
        "a stopped listener must not keep observing keys"
    );
}
