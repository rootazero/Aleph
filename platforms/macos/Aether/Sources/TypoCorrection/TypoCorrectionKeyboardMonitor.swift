import AppKit
import Carbon.HIToolbox

/// Monitors keyboard events to detect double-space trigger for typo correction
@MainActor
final class TypoCorrectionKeyboardMonitor {

    // MARK: - Properties

    /// Time interval for double-tap detection (200ms)
    private let triggerInterval: TimeInterval = 0.2

    /// Timestamp of the last space key press
    private var lastSpaceTime: Date?

    /// Callback when double-space is triggered
    var onTrigger: (() -> Void)?

    /// Global event monitor
    private var eventMonitor: Any?

    /// Whether the monitor is active
    private(set) var isActive = false

    // MARK: - Singleton

    static let shared = TypoCorrectionKeyboardMonitor()

    private init() {}

    // MARK: - Public Methods

    /// Start monitoring keyboard events
    func start() {
        guard !isActive else { return }

        // Use global event monitor for key down events
        eventMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.handleKeyEvent(event)
        }

        isActive = true
        print("[TypoCorrectionKeyboardMonitor] Started monitoring")
    }

    /// Stop monitoring keyboard events
    func stop() {
        guard isActive else { return }

        if let monitor = eventMonitor {
            NSEvent.removeMonitor(monitor)
            eventMonitor = nil
        }

        isActive = false
        lastSpaceTime = nil
        print("[TypoCorrectionKeyboardMonitor] Stopped monitoring")
    }

    // MARK: - Private Methods

    /// Handle a key event and check for double-space trigger
    private func handleKeyEvent(_ event: NSEvent) {
        // Space key code is 49
        guard event.keyCode == 49 else {
            // Reset on non-space key
            lastSpaceTime = nil
            return
        }

        let now = Date()

        // Check for double-space
        if let last = lastSpaceTime, now.timeIntervalSince(last) <= triggerInterval {
            // Double-space detected
            lastSpaceTime = nil
            print("[TypoCorrectionKeyboardMonitor] Double-space detected, triggering correction")

            // Trigger callback on main thread
            DispatchQueue.main.async { [weak self] in
                self?.onTrigger?()
            }
        } else {
            // First space press
            lastSpaceTime = now
        }
    }
}
