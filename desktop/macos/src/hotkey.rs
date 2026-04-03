//! Global hotkey listener using NSEvent monitoring.
//!
//! Uses `NSEvent::addGlobalMonitorForEventsMatchingMask_handler` (unfocused)
//! and `NSEvent::addLocalMonitorForEventsMatchingMask_handler` (focused) to
//! detect key presses system-wide.
//!
//! Requires Accessibility permission (`AXIsProcessTrusted`); without it the
//! global monitor silently receives no events.

use std::ptr::NonNull;
use std::sync::mpsc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// C FFI — Accessibility check
// ---------------------------------------------------------------------------

extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub type HotkeyId = u32;

/// Events emitted by the listener.
#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    /// Single key press (key down + up).
    Pressed(HotkeyId),
    /// Key held down (for PTT-style hold-to-talk).
    KeyDown(HotkeyId),
    /// Key released (for PTT-style hold-to-talk).
    KeyUp(HotkeyId),
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Registered hotkey definition.
struct RegisteredHotkey {
    id: HotkeyId,
    key_code: u16,
    /// Modifier flags mask (only device-independent bits are compared).
    modifiers: NSEventModifierFlags,
    /// Whether to track hold (KeyDown/KeyUp) or just Pressed.
    track_hold: bool,
}

/// Mask that keeps only Shift/Control/Option/Command bits.
const MODIFIER_MASK: NSEventModifierFlags = NSEventModifierFlags(
    NSEventModifierFlags::Shift.0
        | NSEventModifierFlags::Control.0
        | NSEventModifierFlags::Option.0
        | NSEventModifierFlags::Command.0,
);

// ---------------------------------------------------------------------------
// HotkeyListener
// ---------------------------------------------------------------------------

/// A global hotkey listener backed by NSEvent monitors.
///
/// # Usage
///
/// ```ignore
/// let (listener, rx) = HotkeyListener::new();
/// let listener = listener
///     .register(1, 49, NSEventModifierFlags::Command.0, false)  // Cmd+Space
///     .register(2, 0x38, NSEventModifierFlags::Shift.0, true);  // Shift hold
/// listener.start();
///
/// // The calling thread must run an NSRunLoop / CFRunLoop for events to fire.
/// for event in rx.iter() {
///     println!("{event:?}");
/// }
/// ```
pub struct HotkeyListener {
    tx: mpsc::Sender<HotkeyEvent>,
    hotkeys: Vec<RegisteredHotkey>,
    /// Monitor handles returned by NSEvent; kept alive for cleanup.
    monitors: Vec<Retained<AnyObject>>,
}

impl HotkeyListener {
    /// Create a new listener and its corresponding event receiver.
    pub fn new() -> (Self, mpsc::Receiver<HotkeyEvent>) {
        let (tx, rx) = mpsc::channel();
        let listener = Self {
            tx,
            hotkeys: Vec::new(),
            monitors: Vec::new(),
        };
        (listener, rx)
    }

    /// Register a hotkey. Returns self for chaining.
    ///
    /// * `id` — unique identifier echoed back in events.
    /// * `key_code` — virtual key code (e.g. `49` for Space, `0` for A).
    /// * `modifiers` — raw `NSEventModifierFlags` value (bitwise OR of
    ///   Shift/Control/Option/Command constants).
    /// * `track_hold` — if `true`, emit `KeyDown`/`KeyUp`; otherwise emit
    ///   `Pressed` on key-down.
    pub fn register(
        mut self,
        id: HotkeyId,
        key_code: u16,
        modifiers: u64,
        track_hold: bool,
    ) -> Self {
        // Truncate to NSUInteger (usize on 64-bit) — always fits.
        let modifiers = NSEventModifierFlags(modifiers as usize);
        self.hotkeys.push(RegisteredHotkey {
            id,
            key_code,
            modifiers,
            track_hold,
        });
        self
    }

    /// Start listening for registered hotkeys.
    ///
    /// The caller's thread **must** be running an `NSRunLoop` (or `CFRunLoop`)
    /// for the monitors to fire.  In a Tauri app this is typically the main
    /// thread.
    pub fn start(&mut self) {
        // Accessibility check
        let trusted = unsafe { AXIsProcessTrusted() };
        if !trusted {
            warn!(
                "Accessibility permission not granted (AXIsProcessTrusted = false). \
                 Global hotkey monitor will not receive events."
            );
        }

        let mask = NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged;

        // --- Global monitor (app not focused) --------------------------------
        let global_block = self.build_handler_block();
        if let Some(monitor) =
            NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block)
        {
            debug!("Global hotkey monitor installed");
            self.monitors.push(monitor);
        } else {
            warn!("Failed to install global hotkey monitor");
        }

        // --- Local monitor (app focused) -------------------------------------
        let local_block = self.build_local_handler_block();
        // SAFETY: Our block always returns the event pointer unchanged (pass-through).
        if let Some(monitor) =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) }
        {
            debug!("Local hotkey monitor installed");
            self.monitors.push(monitor);
        } else {
            warn!("Failed to install local hotkey monitor");
        }
    }

    /// Remove all monitors.  Called automatically on drop.
    pub fn stop(&mut self) {
        for monitor in self.monitors.drain(..) {
            // SAFETY: monitors were returned by addGlobal/LocalMonitor.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        debug!("Hotkey monitors removed");
    }

    // -----------------------------------------------------------------------
    // Block builders
    // -----------------------------------------------------------------------

    /// Build the `DynBlock<dyn Fn(NonNull<NSEvent>)>` used by the global monitor.
    fn build_handler_block(&self) -> RcBlock<dyn Fn(NonNull<NSEvent>)> {
        let tx = self.tx.clone();
        let registrations = self.snapshot_registrations();

        RcBlock::new(move |event_ptr: NonNull<NSEvent>| {
            let event = unsafe { event_ptr.as_ref() };
            Self::dispatch_event(event, &registrations, &tx);
        })
    }

    /// Build the `DynBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent>` for the local monitor.
    fn build_local_handler_block(&self) -> RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> {
        let tx = self.tx.clone();
        let registrations = self.snapshot_registrations();

        RcBlock::new(move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event_ptr.as_ref() };
            Self::dispatch_event(event, &registrations, &tx);
            // Pass the event through unchanged.
            event_ptr.as_ptr()
        })
    }

    /// Snapshot registration data so it can be moved into a `'static` block.
    fn snapshot_registrations(&self) -> Vec<(HotkeyId, u16, NSEventModifierFlags, bool)> {
        self.hotkeys
            .iter()
            .map(|h| (h.id, h.key_code, h.modifiers, h.track_hold))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Event matching
    // -----------------------------------------------------------------------

    fn dispatch_event(
        event: &NSEvent,
        registrations: &[(HotkeyId, u16, NSEventModifierFlags, bool)],
        tx: &mpsc::Sender<HotkeyEvent>,
    ) {
        let event_type = event.r#type();
        let key_code = event.keyCode();
        let flags = event.modifierFlags();
        let active_modifiers = NSEventModifierFlags(flags.0 & MODIFIER_MASK.0);

        for &(id, reg_code, reg_mods, track_hold) in registrations {
            if event_type == NSEventType::FlagsChanged {
                // Modifier-only hotkey: detect press/release by flag presence.
                if reg_code == key_code {
                    let modifier_active =
                        NSEventModifierFlags(active_modifiers.0 & reg_mods.0) == reg_mods;
                    let evt = if modifier_active {
                        if track_hold {
                            HotkeyEvent::KeyDown(id)
                        } else {
                            HotkeyEvent::Pressed(id)
                        }
                    } else if track_hold {
                        HotkeyEvent::KeyUp(id)
                    } else {
                        continue;
                    };
                    let _ = tx.send(evt);
                }
            } else if key_code == reg_code && active_modifiers == reg_mods {
                // Regular key with optional modifiers.
                let evt = match event_type {
                    NSEventType::KeyDown => {
                        if track_hold {
                            // Skip auto-repeat for hold tracking.
                            if event.isARepeat() {
                                continue;
                            }
                            HotkeyEvent::KeyDown(id)
                        } else {
                            HotkeyEvent::Pressed(id)
                        }
                    }
                    NSEventType::KeyUp if track_hold => HotkeyEvent::KeyUp(id),
                    _ => continue,
                };
                let _ = tx.send(evt);
            }
        }
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        self.stop();
    }
}
