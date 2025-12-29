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

    init(haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
        setupEscapeKeyMonitor()
    }

    deinit {
        removeEscapeKeyMonitor()
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

    func onHotkeyDetected(clipboardContent: String) {
        print("[EventHandler] Hotkey detected, clipboard: \(clipboardContent.prefix(50))...")

        // Capture context when hotkey is detected
        let context = ContextCapture.captureContext()
        print("[EventHandler] Captured context: app=\(context.bundleId ?? "nil"), window=\(context.windowTitle ?? "nil")")

        // Send context to Rust core
        if let bundleId = context.bundleId {
            let capturedContext = CapturedContext(
                appBundleId: bundleId,
                windowTitle: context.windowTitle
            )
            core?.setCurrentContext(context: capturedContext)
        }

        DispatchQueue.main.async { [weak self] in
            self?.handleHotkeyDetected(clipboardContent: clipboardContent)
        }
    }

    func onError(message: String, suggestion: String?) {
        print("[EventHandler] Error: \(message)")
        if let sug = suggestion {
            print("[EventHandler] Suggestion: \(sug)")
        }

        DispatchQueue.main.async { [weak self] in
            // Update Halo window to show error with suggestion
            self?.haloWindow?.updateState(.error(type: .unknown, message: message, suggestion: suggestion))

            // Also show system notification
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

    // Config Hot-Reload Callback (Phase 6 - Section 6.2)
    func onConfigChanged() {
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
            haloWindow?.updateState(.listening)
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
            // Show final accumulated text
            if !accumulatedText.isEmpty {
                haloWindow?.updateState(.success(finalText: accumulatedText))
                announceToVoiceOver("Request completed successfully")
            } else {
                haloWindow?.updateState(.success(finalText: nil))
                announceToVoiceOver("Success")
            }
            // Auto-hide after 2 seconds
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.haloWindow?.hide()
            }

        case .error:
            // Use typed error if available, otherwise show generic error
            haloWindow?.updateState(.error(type: .unknown, message: "An error occurred", suggestion: nil))
            announceToVoiceOver("Error occurred")
            // Auto-hide after 2 seconds
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.haloWindow?.hide()
            }

        case .typewriting:
            // Show typewriter state with progress
            haloWindow?.updateState(.typewriting(progress: 0.0))
            announceToVoiceOver("Typewriter animation started. Press Escape to skip.")
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
        let color = parseHexColor(providerColor) ?? .green

        // Update Halo to show AI processing with provider info
        haloWindow?.updateState(.processingWithAI(providerColor: color, providerName: providerName))
    }

    private func handleAiResponseReceived(responsePreview: String) {
        // Store the response preview
        accumulatedText = responsePreview

        // Update Halo with the response preview
        haloWindow?.updateState(.processing(providerColor: .green, streamingText: responsePreview))
    }

    /// Parse hex color string to Color
    private func parseHexColor(_ hex: String) -> Color? {
        var hexSanitized = hex.trimmingCharacters(in: .whitespacesAndNewlines)
        hexSanitized = hexSanitized.replacingOccurrences(of: "#", with: "")

        var rgb: UInt64 = 0
        guard Scanner(string: hexSanitized).scanHexInt64(&rgb) else {
            return nil
        }

        let r = Double((rgb & 0xFF0000) >> 16) / 255.0
        let g = Double((rgb & 0x00FF00) >> 8) / 255.0
        let b = Double(rgb & 0x0000FF) / 255.0

        return Color(red: r, green: g, blue: b)
    }

    // MARK: - Typed Error Handling

    private func handleTypedError(errorType: ErrorType, message: String) {
        // Update Halo with typed error (do NOT auto-hide, let user interact with error actions)
        // Note: onErrorTyped doesn't include suggestion, only onError callback does
        haloWindow?.updateState(.error(type: errorType, message: message, suggestion: nil))

        // NOTE: We don't auto-hide the Halo for errors anymore because
        // ErrorActionView provides actionable buttons that the user might want to interact with.
        // The Halo will hide when the user clicks "Dismiss" or after a successful retry.
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

    /// Show permission prompt in Halo window
    func showPermissionPrompt(type: PermissionType) {
        print("[EventHandler] Showing permission prompt: \(type)")
        // Show halo window centered on screen
        if let screen = NSScreen.main {
            let screenRect = screen.frame
            let position = CGPoint(
                x: screenRect.midX,
                y: screenRect.midY
            )
            haloWindow?.show(at: position)
        } else {
            // Fallback to center of primary screen
            haloWindow?.show(at: CGPoint(x: 960, y: 540))
        }

        // Set permission required state
        haloWindow?.setState(.permissionRequired(type: type))
    }

    // MARK: - Hotkey Handling

    private func handleHotkeyDetected(clipboardContent: String) {
        // Get current mouse position
        let mouseLocation = NSEvent.mouseLocation

        // Show Halo at cursor
        haloWindow?.show(at: mouseLocation)
        haloWindow?.updateState(.listening)

        // Simulate AI processing (placeholder for Phase 2)
        // In Phase 4, this will trigger actual AI routing
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
            self?.haloWindow?.updateState(.processing(providerColor: .green))
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
            self?.haloWindow?.updateState(.success(finalText: nil))
        }
    }

    // MARK: - Error Notification

    private func showErrorNotification(message: String, suggestion: String?) {
        let alert = NSAlert()
        alert.messageText = "Aether Error"

        // Combine message and suggestion
        var fullMessage = message
        if let sug = suggestion {
            fullMessage += "\n\n💡 Suggestion: \(sug)"
        }

        alert.informativeText = fullMessage
        alert.alertStyle = .warning
        alert.addButton(withTitle: "OK")
        alert.runModal()
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
