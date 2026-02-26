import Cocoa
import Carbon

// MARK: - GlobalShortcuts

/// Registers system-wide keyboard shortcuts using a CGEvent tap.
///
/// This replaces `tauri_plugin_global_shortcut` from the Tauri bridge.
/// Currently registers Cmd+Opt+/ to show the Halo floating window.
///
/// **Accessibility permission is required** — the app must be granted
/// permission in System Settings > Privacy & Security > Accessibility
/// for the event tap to function.
final class GlobalShortcuts {
    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?

    /// Registers the global keyboard shortcut event tap.
    ///
    /// Creates a CGSession event tap that intercepts key-down events
    /// and watches for the Cmd+Opt+/ combination. When detected, it
    /// posts a `.showHalo` notification and consumes the event.
    func register() {
        let mask: CGEventMask = (1 << CGEventType.keyDown.rawValue)

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: mask,
            callback: { _, _, event, _ -> Unmanaged<CGEvent>? in
                let keyCode = event.getIntegerValueField(.keyboardEventKeycode)
                let flags = event.flags

                // Cmd+Opt+/ (keycode 0x2C = forward slash)
                if keyCode == 0x2C,
                   flags.contains(.maskCommand),
                   flags.contains(.maskAlternate) {
                    DispatchQueue.main.async {
                        NotificationCenter.default.post(name: .showHalo, object: nil)
                    }
                    return nil // Consume the event
                }
                return Unmanaged.passRetained(event)
            },
            userInfo: nil
        ) else {
            print("Failed to create event tap — accessibility permission required")
            return
        }

        self.eventTap = tap
        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        self.runLoopSource = source
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
    }

    /// Disables and removes the event tap from the run loop.
    func unregister() {
        if let tap = eventTap {
            CGEvent.tapEnable(tap: tap, enable: false)
        }
        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, .commonModes)
        }
        eventTap = nil
        runLoopSource = nil
    }

    deinit {
        unregister()
    }
}
