//! Global hotkey listener using NSEvent monitoring.
//!
//! Uses `NSEvent::addGlobalMonitorForEventsMatchingMask_handler` (unfocused)
//! and `NSEvent::addLocalMonitorForEventsMatchingMask_handler` (focused) to
//! detect key presses system-wide.
//!
//! Requires Accessibility **and** InputMonitoring TCC permissions; without them
//! the global monitor silently receives no events.
//!
//! # Preflight (Stage 4)
//!
//! `start()` spawns a background Tokio task (Option B — keeps `start()` sync)
//! that calls `perm.check` for InputMonitoring and Accessibility via the Swift
//! bridge.  If either is missing it emits:
//! - `tracing::warn!` with structured `kind`, `deep_link`, `rationale` fields
//! - `HotkeyEvent::PermissionMissing(Box<PermissionGuide>)` on the existing
//!   mpsc channel (best-effort send)
//!
//! Session-side surfacing requires a hotkey-event consumer in `src/` which does
//! not yet exist — that wiring is out of scope for Stage 4.  The tracing + event
//! channel emission is the contract for now.

use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::Arc;

use aleph_desktop::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::perm::{
    CheckParams, GuideParams, PermissionGuide, PermissionKind, METHOD_CHECK, METHOD_GUIDE,
};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use tracing::{debug, warn};

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
    /// A required TCC permission is missing; contains a guide for the user.
    PermissionMissing(Box<PermissionGuide>),
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
/// let bridge = Arc::new(SwiftBridge::new(path));
/// let (listener, rx) = HotkeyListener::new(bridge);
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
    /// Bridge used for TCC permission preflight in `start()`.
    bridge: Arc<SwiftBridge>,
}

impl HotkeyListener {
    /// Create a new listener and its corresponding event receiver.
    pub fn new(bridge: Arc<SwiftBridge>) -> (Self, mpsc::Receiver<HotkeyEvent>) {
        let (tx, rx) = mpsc::channel();
        let listener = Self {
            tx,
            hotkeys: Vec::new(),
            monitors: Vec::new(),
            bridge,
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
    ///
    /// # Preflight (Stage 4, Option B)
    ///
    /// A background Tokio task is spawned to check InputMonitoring and
    /// Accessibility permissions via the Swift bridge.  Any missing permission
    /// results in a `tracing::warn!` and a `HotkeyEvent::PermissionMissing`
    /// sent on the mpsc channel (best-effort).
    pub fn start(&mut self) {
        // --- Permission preflight (Option B — background task, start() stays sync) ---
        // Only spawn the background check when we are inside a Tokio runtime.
        // Falls through silently in sync test/CLI contexts.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let bridge = Arc::clone(&self.bridge);
            let tx = self.tx.clone();
            handle.spawn(async move {
                for kind in [
                    PermissionKind::InputMonitoring,
                    PermissionKind::Accessibility,
                ] {
                    let params = CheckParams { kind };
                    let result: Result<
                        aleph_protocol::desktop_bridge::methods::perm::PermissionStatus,
                        _,
                    > = bridge.call(METHOD_CHECK, &params).await;

                    match result {
                        Ok(status) if !status.granted => {
                            // Fetch the guide for actionable user-facing info.
                            let guide: Option<PermissionGuide> =
                                bridge.call(METHOD_GUIDE, &GuideParams { kind }).await.ok();
                            let deep_link = guide
                                .as_ref()
                                .map(|g| g.deep_link.as_str())
                                .unwrap_or("<unknown>");
                            let rationale = guide
                                .as_ref()
                                .map(|g| g.rationale.as_str())
                                .unwrap_or("<unknown>");
                            warn!(
                                kind = ?kind,
                                deep_link = deep_link,
                                rationale = rationale,
                                "Hotkey preflight: TCC permission not granted; \
                                 global monitor will receive no events for this kind"
                            );
                            if let Some(g) = guide {
                                let _ = tx.send(HotkeyEvent::PermissionMissing(Box::new(g)));
                            }
                        }
                        Err(e) => {
                            warn!(
                                kind = ?kind,
                                error = %e,
                                "Hotkey preflight: permission check failed"
                            );
                        }
                        Ok(_) => {} // granted — nothing to do
                    }
                }
            });
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
            warn!(
                "Failed to install global hotkey monitor. \
                 This usually means Accessibility or Input Monitoring TCC permission is denied."
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_event_debug_clone_send_sync() {
        // HotkeyEvent must be Send + Sync so it can cross task boundaries.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HotkeyEvent>();

        // Debug round-trip
        let event = HotkeyEvent::Pressed(42);
        let debug = format!("{:?}", event);
        assert!(debug.contains("Pressed"));
        assert!(debug.contains("42"));

        // Clone
        let event2 = event.clone();
        assert!(matches!(event2, HotkeyEvent::Pressed(42)));

        // PermissionMissing holds Box
        let guide = PermissionGuide {
            kind: PermissionKind::Accessibility,
            status: aleph_protocol::desktop_bridge::methods::perm::PermissionStatus {
                kind: PermissionKind::Accessibility,
                granted: false,
                can_request_programmatically: false,
                restricted: false,
            },
            deep_link: "x-apple.systempreferences".into(),
            human_readable_steps: vec!["Open System Preferences".into()],
            rationale: "test".into(),
        };
        let perm_event = HotkeyEvent::PermissionMissing(Box::new(guide));
        assert!(matches!(perm_event, HotkeyEvent::PermissionMissing(_)));
    }

    #[test]
    fn test_register_returns_self_for_chaining() {
        let bridge = Arc::new(SwiftBridge::new(std::path::PathBuf::from("/dev/null")));
        let (listener, _rx) = HotkeyListener::new(bridge);
        let listener = listener
            .register(1, 49, NSEventModifierFlags::Command.0 as u64, false)
            .register(2, 0x38, NSEventModifierFlags::Shift.0 as u64, true);
        assert!(listener.hotkeys.len() == 2);
        assert_eq!(listener.hotkeys[0].id, 1);
        assert_eq!(listener.hotkeys[0].key_code, 49);
        assert!(!listener.hotkeys[0].track_hold);
        assert_eq!(listener.hotkeys[1].id, 2);
        assert_eq!(listener.hotkeys[1].key_code, 0x38);
        assert!(listener.hotkeys[1].track_hold);
    }

    #[test]
    fn test_snapshot_registrations() {
        let bridge = Arc::new(SwiftBridge::new(std::path::PathBuf::from("/dev/null")));
        let (listener, _rx) = HotkeyListener::new(bridge);
        let listener = listener
            .register(1, 49, NSEventModifierFlags::Command.0 as u64, false)
            .register(2, 0x38, NSEventModifierFlags::Shift.0 as u64, true);
        let snapshot = listener.snapshot_registrations();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].0, 1); // id
        assert_eq!(snapshot[0].1, 49); // key_code
        assert_eq!(snapshot[1].0, 2);
        assert_eq!(snapshot[1].1, 0x38);
    }

    #[test]
    fn test_modifier_mask_contains_expected_bits() {
        let mask = MODIFIER_MASK;
        assert!(mask.contains(NSEventModifierFlags::Shift));
        assert!(mask.contains(NSEventModifierFlags::Control));
        assert!(mask.contains(NSEventModifierFlags::Option));
        assert!(mask.contains(NSEventModifierFlags::Command));
    }

    #[test]
    fn test_listener_stop_clears_monitors() {
        let bridge = Arc::new(SwiftBridge::new(std::path::PathBuf::from("/dev/null")));
        let (mut listener, _rx) = HotkeyListener::new(bridge);
        // Without start(), monitors is empty; stop() should be safe to call.
        listener.stop();
        assert!(listener.monitors.is_empty());
    }
}
