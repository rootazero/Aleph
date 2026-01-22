import AppKit
import UserNotifications

/// Coordinator for the typo correction feature
/// Manages the flow: keyboard trigger -> text retrieval -> AI correction -> text replacement
@MainActor
final class TypoCorrectionCoordinator {

    // MARK: - Singleton

    static let shared = TypoCorrectionCoordinator()

    // MARK: - Properties

    /// Whether a correction is currently in progress
    private var isProcessing = false

    /// Keyboard monitor instance
    private let keyboardMonitor = TypoCorrectionKeyboardMonitor.shared

    /// Accessibility helper instance
    private let accessibilityHelper = AccessibilityHelper.shared

    // MARK: - Initialization

    private init() {
        setupKeyboardMonitor()
    }

    // MARK: - Public Methods

    /// Start the typo correction feature
    func start() {
        guard accessibilityHelper.hasAccessibilityPermission() else {
            print("[TypoCorrectionCoordinator] Accessibility permission not granted")
            // Request permission
            accessibilityHelper.requestAccessibilityPermission()
            return
        }

        keyboardMonitor.start()
        print("[TypoCorrectionCoordinator] Started")
    }

    /// Stop the typo correction feature
    func stop() {
        keyboardMonitor.stop()
        print("[TypoCorrectionCoordinator] Stopped")
    }

    // MARK: - Private Methods

    private func setupKeyboardMonitor() {
        keyboardMonitor.onTrigger = { [weak self] in
            Task { @MainActor in
                await self?.triggerCorrection()
            }
        }
    }

    /// Trigger the correction process
    private func triggerCorrection() async {
        // Prevent concurrent corrections
        guard !isProcessing else {
            print("[TypoCorrectionCoordinator] Already processing, ignoring trigger")
            return
        }

        isProcessing = true
        defer { isProcessing = false }

        print("[TypoCorrectionCoordinator] Correction triggered")

        // 1. Get the focused text
        guard let text = accessibilityHelper.getFocusedText() else {
            print("[TypoCorrectionCoordinator] Failed to get focused text")
            return
        }

        // 2. Check if there's any meaningful content
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            print("[TypoCorrectionCoordinator] No meaningful content to correct")
            return
        }

        // 3. Get the AetherCore instance from AppDelegate
        guard let core = (NSApplication.shared.delegate as? AppDelegate)?.core else {
            print("[TypoCorrectionCoordinator] AetherCore not initialized")
            showNotification(title: "Typo Correction", message: "Service not ready")
            return
        }

        // 4. Call the Rust core for correction
        // Note: This is a synchronous call that blocks briefly during the network request.
        // The Rust side uses its own tokio runtime to handle the async network call.
        print("[TypoCorrectionCoordinator] Sending text to AI for correction (\(text.count) chars)")

        let result = core.correctTypo(text: text)

        // 5. Handle the result - full text replacement (AI handles trailing spaces)
        switch result {
        case .success(let correctedText, _):
            print("[TypoCorrectionCoordinator] Text corrected, applying changes")
            accessibilityHelper.setFocusedText(correctedText)

        case .error(let message):
            print("[TypoCorrectionCoordinator] Correction failed: \(message)")
            showNotification(title: "Typo Correction Failed", message: message)
        }
    }

    /// Show a system notification
    private func showNotification(title: String, message: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = message
        content.sound = .default

        let request = UNNotificationRequest(
            identifier: UUID().uuidString,
            content: content,
            trigger: nil
        )

        UNUserNotificationCenter.current().add(request) { error in
            if let error = error {
                print("[TypoCorrectionCoordinator] Failed to show notification: \(error)")
            }
        }
    }
}
