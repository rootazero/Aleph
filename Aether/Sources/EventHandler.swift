//
//  EventHandler.swift
//  Aether
//
//  Implements AetherEventHandler protocol to receive callbacks from Rust core.
//

import Foundation
import AppKit
import SwiftUI
import UserNotifications

class EventHandler: AetherEventHandler {
    // Weak reference to Halo window to avoid retain cycle
    private weak var haloWindow: HaloWindow?

    // Weak reference to AetherCore for retry functionality
    private weak var core: AetherCore?

    // Accumulated text for streaming responses
    private var accumulatedText: String = ""

    // Last update time for debouncing
    private var lastUpdateTime: Date = Date()

    // Escape key monitor for cancelling typewriter
    private var escapeKeyMonitor: Any?

    // Auto-dismiss timer for toast notifications
    private var toastDismissTimer: Timer?

    init(haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
        setupEscapeKeyMonitor()
        setupInternalConfigSaveObserver()
    }

    deinit {
        removeEscapeKeyMonitor()
        toastDismissTimer?.invalidate()
        NotificationCenter.default.removeObserver(self)
    }

    // Set AetherCore reference after initialization
    func setCore(_ core: AetherCore) {
        self.core = core
    }

    // MARK: - AetherEventHandler Protocol

    func onStateChanged(state: ProcessingState) {
        print("[EventHandler] State changed: \(state)")

        // All UI updates must happen on main thread
        DispatchQueue.main.async { [weak self] in
            self?.handleStateChange(state)
        }
    }

    // REMOVED: onHotkeyDetected() - hotkey handling now in Swift layer (AppDelegate.handleHotkeyPressed)
    // The new architecture uses GlobalHotkeyMonitor → AppDelegate → Core.processInput()
    // instead of Rust rdev → EventHandler callback

    func onError(message: String, suggestion: String?) {
        print("[EventHandler] Error: \(message)")
        if let sug = suggestion {
            print("[EventHandler] Suggestion: \(sug)")
        }

        DispatchQueue.main.async { [weak self] in
            // Show error toast notification (do NOT call hide() first - it has a delayed
            // orderOut that would hide the toast after it appears)
            self?.showErrorNotification(message: message, suggestion: suggestion)
        }
    }

    // NEW: Handle streaming response chunks
    func onResponseChunk(text: String) {
        print("[EventHandler] Response chunk: \(text.prefix(50))...")

        DispatchQueue.main.async { [weak self] in
            self?.handleResponseChunk(text: text)
        }
    }

    // NEW: Handle typed errors
    func onErrorTyped(errorType: ErrorType, message: String) {
        print("[EventHandler] Typed error (\(errorType)): \(message)")

        DispatchQueue.main.async { [weak self] in
            self?.handleTypedError(errorType: errorType, message: message)
        }
    }

    // NEW: Handle progress updates
    func onProgress(percent: Float) {
        print("[EventHandler] Progress: \(percent)%")

        // Progress updates are not yet visually displayed
        // This can be implemented in future phases
    }

    // AI Processing Callbacks (Phase 9)
    func onAiProcessingStarted(providerName: String, providerColor: String) {
        print("[EventHandler] AI processing started: provider=\(providerName), color=\(providerColor)")

        DispatchQueue.main.async { [weak self] in
            self?.handleAiProcessingStarted(providerName: providerName, providerColor: providerColor)
        }
    }

    func onAiResponseReceived(responsePreview: String) {
        print("[EventHandler] AI response received: \(responsePreview.prefix(100))...")

        DispatchQueue.main.async { [weak self] in
            self?.handleAiResponseReceived(responsePreview: responsePreview)
        }
    }

    // MARK: - Internal State for Config Change Debouncing

    private var lastInternalSaveTime: Date?
    private let internalSaveDebounceInterval: TimeInterval = 2.0  // 2 seconds

    // Config Hot-Reload Callback (Phase 6 - Section 6.2)
    func onConfigChanged() {
        // Check if this change was triggered by an internal save (within debounce window)
        if let lastSave = lastInternalSaveTime,
           Date().timeIntervalSince(lastSave) < internalSaveDebounceInterval {
            print("[EventHandler] Config changed by internal save, skipping external change notification")
            return
        }

        print("[EventHandler] Config file changed externally")

        DispatchQueue.main.async {
            // Post notification to notify all observers
            NotificationCenter.default.post(
                name: NSNotification.Name("AetherConfigDidChange"),
                object: nil
            )

            // Optional: Show toast notification to user
            self.showConfigReloadedToast()
        }
    }

    // Called when configuration is saved internally from UI
    func recordInternalConfigSave() {
        lastInternalSaveTime = Date()
        print("[EventHandler] Recorded internal config save at \(Date())")
    }

    // Provider Fallback Callback
    func onProviderFallback(fromProvider: String, toProvider: String) {
        print("[EventHandler] Provider fallback: \(fromProvider) -> \(toProvider)")

        DispatchQueue.main.async {
            // Show subtle notification about fallback
            let content = UNMutableNotificationContent()
            content.title = "Aether"
            content.body = "Switched from \(fromProvider) to \(toProvider)"
            let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
            UNUserNotificationCenter.current().add(request)
        }
    }

    // Typewriter Progress Callback
    func onTypewriterProgress(percent: Float) {
        print("[EventHandler] Typewriter progress: \(Int(percent * 100))%")

        DispatchQueue.main.async { [weak self] in
            // Update Halo with typewriter progress
            self?.haloWindow?.updateTypewriterProgress(percent)

            // Announce progress milestones to VoiceOver (every 25%)
            let progress = Int(percent * 100)
            if progress % 25 == 0 && progress > 0 {
                self?.announceToVoiceOver("Typewriter \(progress) percent complete")
            }
        }
    }

    // Typewriter Cancelled Callback
    func onTypewriterCancelled() {
        print("[EventHandler] Typewriter cancelled by user")

        DispatchQueue.main.async { [weak self] in
            // Show brief notification or just hide progress
            self?.haloWindow?.updateState(.success(finalText: nil))

            // Auto-hide after 1 second
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                self?.haloWindow?.hide()
            }
        }
    }

    // MARK: - State Change Handling

    private func handleStateChange(_ state: ProcessingState) {
        switch state {
        case .idle:
            haloWindow?.hide()
            // Reset accumulated text when going idle
            accumulatedText = ""

        case .listening:
            // Use processing animation for listening state (same as processingWithAI)
            // This unifies the visual feedback: processing icon → (hidden) → success
            haloWindow?.updateState(.processing(providerColor: .blue, streamingText: nil))
            // Reset accumulated text when starting new interaction
            accumulatedText = ""
            announceToVoiceOver("Listening for input")

        case .retrievingMemory:
            haloWindow?.updateState(.retrievingMemory)
            announceToVoiceOver("Retrieving memories")

        case .processingWithAi:
            // This state will be updated with provider details via onAiProcessingStarted callback
            haloWindow?.updateState(.processing(providerColor: .blue, streamingText: nil))
            announceToVoiceOver("Processing with AI")

        case .processing:
            haloWindow?.updateState(.processing(providerColor: .green, streamingText: nil))
            announceToVoiceOver("Processing request")

        case .success:
            // Show success checkmark only (no text)
            haloWindow?.updateState(.success(finalText: nil))
            announceToVoiceOver("Request completed successfully")

            // Auto-hide after 2 seconds
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.haloWindow?.hide()
            }

        case .error:
            // Do NOT hide Halo here - errors are now shown via toast notification
            // The toast is displayed by onError() callback which fires before this state change
            // Hiding here would cause the toast to flash and disappear immediately
            announceToVoiceOver("Error occurred")

        case .typewriting:
            // Ignore typewriting state - Halo is hidden during output
            // to reduce visual distractions (keyboard icon removed)
            break
        }
    }

    // MARK: - Streaming Response Handling

    private func handleResponseChunk(text: String) {
        // Accumulate text
        accumulatedText = text

        // Update Halo with streaming text
        haloWindow?.updateState(.processing(providerColor: .green, streamingText: text))

        // Update timestamp
        lastUpdateTime = Date()
    }

    // MARK: - AI Processing Handling

    private func handleAiProcessingStarted(providerName: String, providerColor: String) {
        // Parse provider color from hex string (e.g., "#10a37f")
        let color = Color(hex: providerColor) ?? .green

        // Update Halo to show AI processing with provider info
        haloWindow?.updateState(.processingWithAI(providerColor: color, providerName: providerName))
    }

    private func handleAiResponseReceived(responsePreview: String) {
        // Store the response preview
        accumulatedText = responsePreview

        // Update Halo with the response preview
        haloWindow?.updateState(.processing(providerColor: .green, streamingText: responsePreview))
    }

    // MARK: - Typed Error Handling

    private func handleTypedError(errorType: ErrorType, message: String) {
        // Show error toast (do NOT call hide() first - it conflicts with toast display)
        showErrorNotification(message: message, suggestion: nil)
    }

    // MARK: - Error Action Handlers

    /// Handle retry action from ErrorActionView
    func handleRetry() {
        print("[EventHandler] Retry requested")

        guard let core = core else {
            print("[EventHandler] Error: No AetherCore reference to retry")
            return
        }

        do {
            try core.retryLastRequest()
            print("[EventHandler] Retry initiated successfully")
        } catch {
            print("[EventHandler] Retry failed: \(error)")
            // Show error message
            handleTypedError(errorType: .unknown, message: "Retry failed: \(error.localizedDescription)")
        }
    }

    /// Handle open settings action from ErrorActionView
    func handleOpenSettings() {
        print("[EventHandler] Open settings requested")

        // Open System Settings to Accessibility pane
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }

    /// Handle dismiss action from ErrorActionView
    func handleDismiss() {
        print("[EventHandler] Dismiss requested")
        haloWindow?.hide()
        accumulatedText = ""
    }

    // MARK: - Toast Notifications

    /// Show toast notification in Halo window
    ///
    /// - Parameters:
    ///   - type: Toast type (info, warning, error)
    ///   - title: Toast title text
    ///   - message: Toast message text
    ///   - autoDismiss: Whether to auto-dismiss (3s for info, disabled for warning/error)
    func showToast(type: ToastType, title: String, message: String, autoDismiss: Bool = true) {
        print("[EventHandler] Showing toast: \(type) - \(title)")

        // Cancel any existing dismiss timer
        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // Create dismiss closure
            let dismissAction: () -> Void = { [weak self] in
                self?.dismissToast()
            }

            // Update Halo state to toast
            let shouldAutoDismiss = autoDismiss && type == .info
            self.haloWindow?.updateState(.toast(
                type: type,
                title: title,
                message: message,
                autoDismiss: shouldAutoDismiss,
                onDismiss: dismissAction
            ))

            // Show at screen center
            self.haloWindow?.showToastCentered()

            // Setup auto-dismiss timer for info toasts
            if shouldAutoDismiss {
                self.toastDismissTimer = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: false) { [weak self] _ in
                    self?.dismissToast()
                }
            }
        }
    }

    /// Dismiss current toast notification
    func dismissToast() {
        print("[EventHandler] Dismissing toast")

        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        DispatchQueue.main.async { [weak self] in
            self?.haloWindow?.hide()
        }
    }

    /// Show permission prompt in Halo window
    /// DEPRECATED: Now using PermissionGateView instead of Halo for permission prompts
    /// Kept for backward compatibility but does not show any UI
    func showPermissionPrompt(type: PermissionType) {
        print("[EventHandler] showPermissionPrompt called (DEPRECATED) - Permission gate should be used instead")
        print("[EventHandler] Permission type: \(type)")

        // NOTE: This method is now deprecated in favor of the PermissionGateView
        // which is shown automatically on app launch if permissions are missing.
        // The old implementation that showed permission prompts in the Halo window
        // has been removed because it's incompatible with the mandatory permission gate approach.

        // No-op: Permission prompts are now handled by PermissionGateView in AppDelegate
    }

    // REMOVED: handleHotkeyDetected() - hotkey handling now in Swift layer (AppDelegate.handleHotkeyPressed)
    // The entire hotkey flow is now: GlobalHotkeyMonitor → AppDelegate → Core.processInput() → KeyboardSimulator

    // MARK: - Error Notification

    private func showErrorNotification(message: String, suggestion: String?) {
        // Combine message and suggestion
        var fullMessage = message
        if let sug = suggestion {
            fullMessage += "\n\n\(sug)"
        }

        // Use toast notification instead of NSAlert
        showToast(type: .error, title: L("error.aether"), message: fullMessage, autoDismiss: false)
    }

    // MARK: - Config Reload Notification

    /// Show a subtle toast notification when config is reloaded
    private func showConfigReloadedToast() {
        // Using UserNotifications for a non-intrusive toast
        let content = UNMutableNotificationContent()
        content.title = "Aether"
        content.body = "Settings updated from file"
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    // MARK: - Escape Key Monitoring

    /// Setup global Escape key monitor to cancel typewriter
    private func setupEscapeKeyMonitor() {
        escapeKeyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            // Check if Escape key was pressed (keyCode 53)
            if event.keyCode == 53 {
                self?.handleEscapeKey()
                // Return nil to consume the event (prevent default behavior)
                // Return event to allow it to propagate
                return event
            }
            return event
        }
        print("[EventHandler] Escape key monitor installed")
    }

    /// Remove Escape key monitor
    private func removeEscapeKeyMonitor() {
        if let monitor = escapeKeyMonitor {
            NSEvent.removeMonitor(monitor)
            escapeKeyMonitor = nil
            print("[EventHandler] Escape key monitor removed")
        }
    }

    /// Handle Escape key press
    private func handleEscapeKey() {
        guard core != nil else {
            print("[EventHandler] Cannot cancel typewriter: core not available")
            return
        }

        // TODO: Implement typewriter cancellation support in Rust core
        // These methods need to be added to aether.udl and implemented:
        // - is_typewriting() -> boolean
        // - cancel_typewriter() -> boolean
        print("[EventHandler] Escape pressed (typewriter cancellation not yet implemented)")

        // Temporary workaround: hide halo window
        DispatchQueue.main.async { [weak self] in
            self?.haloWindow?.hide()
        }
    }

    // MARK: - Internal Config Save Observer

    /// Setup observer for internal config save notifications
    private func setupInternalConfigSaveObserver() {
        NotificationCenter.default.addObserver(
            forName: NSNotification.Name("AetherConfigSavedInternally"),
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.recordInternalConfigSave()
        }
        print("[EventHandler] Internal config save observer installed")
    }

    // MARK: - VoiceOver Support

    /// Announce message to VoiceOver users
    /// - Parameter message: The message to announce
    private func announceToVoiceOver(_ message: String) {
        #if os(macOS)
        // Use NSAccessibility to post announcement
        NSAccessibility.post(
            element: (NSApp.mainWindow ?? NSApp) as Any,
            notification: .announcementRequested,
            userInfo: [.announcement: message, .priority: NSAccessibilityPriorityLevel.high.rawValue]
        )
        #endif
        print("[EventHandler] VoiceOver announcement: \(message)")
    }
}
