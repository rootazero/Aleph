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

        // 2. Remove trailing spaces (the two spaces from the trigger)
        var cleanText = text
        // Remove up to 2 trailing spaces (the trigger spaces)
        var spacesRemoved = 0
        while cleanText.hasSuffix(" ") && spacesRemoved < 2 {
            cleanText.removeLast()
            spacesRemoved += 1
        }

        // 3. Check if there's any meaningful content
        if cleanText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            print("[TypoCorrectionCoordinator] No meaningful content to correct")
            // Just remove the trigger spaces
            accessibilityHelper.setFocusedText(cleanText)
            return
        }

        // 4. Get the AetherCore instance
        guard let core = DependencyContainer.shared.core else {
            print("[TypoCorrectionCoordinator] AetherCore not initialized")
            showNotification(title: "Typo Correction", message: "Service not ready")
            // Restore text without trigger spaces
            accessibilityHelper.setFocusedText(cleanText)
            return
        }

        // 5. Call the Rust core for correction
        print("[TypoCorrectionCoordinator] Sending text to AI for correction (\(cleanText.count) chars)")

        let result = await core.correctTypo(text: cleanText)

        // 6. Handle the result
        switch result {
        case .success(let correctedText, let hasChanges):
            if hasChanges {
                print("[TypoCorrectionCoordinator] Text corrected, applying changes")
                accessibilityHelper.setFocusedText(correctedText)
            } else {
                print("[TypoCorrectionCoordinator] No corrections needed")
                // Remove trigger spaces
                accessibilityHelper.setFocusedText(cleanText)
            }

        case .error(let message):
            print("[TypoCorrectionCoordinator] Correction failed: \(message)")
            showNotification(title: "Typo Correction Failed", message: message)
            // Restore text without trigger spaces
            accessibilityHelper.setFocusedText(cleanText)
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
