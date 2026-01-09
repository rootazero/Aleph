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

    // Managers accessed through DependencyContainer (eliminates direct .shared usage)
    private var clarificationManager: any ClarificationManagerProtocol {
        DependencyContainer.shared.clarificationManager
    }
    private var conversationManager: any ConversationManagerProtocol {
        DependencyContainer.shared.conversationManager
    }

    // Accumulated text for streaming responses
    private var accumulatedText: String = ""

    // Last update time for debouncing
    private var lastUpdateTime: Date = Date()

    // Escape key monitor for cancelling typewriter
    private var escapeKeyMonitor: Any?

    // Auto-dismiss timer for toast notifications
    private var toastDismissTimer: Timer?

    // Minimum time Halo should display before showing error toast (in seconds)
    private let minHaloDisplayTime: TimeInterval = 1.0

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

    // Set HaloWindow reference after initialization (for DependencyContainer use)
    func setHaloWindow(_ window: HaloWindow?) {
        self.haloWindow = window
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
            guard let self = self else { return }

            // Calculate delay to ensure Halo displays for at least minHaloDisplayTime
            // showTime is tracked by HaloWindow when show() is called
            let delay: TimeInterval
            if let showTime = self.haloWindow?.showTime {
                let elapsed = Date().timeIntervalSince(showTime)
                delay = max(0, self.minHaloDisplayTime - elapsed)
                print("[EventHandler] Halo displayed for \(String(format: "%.2f", elapsed))s, delaying toast by \(String(format: "%.2f", delay))s")
            } else {
                // Halo not showing - use minimum display time as delay
                // This ensures user sees the Halo animation before error
                delay = self.minHaloDisplayTime
                print("[EventHandler] No Halo show time recorded, using minimum delay: \(self.minHaloDisplayTime)s")

                // CRITICAL: Show Halo animation first if not already visible
                // This handles the race condition where error fires before Halo shows
                // Use .processing state with purple to show the theme's processing animation
                // (purple + 3 arcs for Zen theme)
                self.haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
                self.haloWindow?.showCentered()
                print("[EventHandler] Showing Halo animation before error toast")
            }

            // Show error toast after delay
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                self?.showErrorNotification(message: message, suggestion: suggestion)
            }
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
                name: .aetherConfigDidChange,
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

    // MARK: - Clarification (Phantom Flow)

    /// Called when Rust core needs clarification from user
    ///
    /// This is a BLOCKING callback - the Rust core will wait for this to return.
    /// The callback delegates to ClarificationManager which coordinates with the Halo UI.
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    /// ClarificationManager handles the thread synchronization internally.
    func onClarificationNeeded(request: ClarificationRequest) -> ClarificationResult {
        print("[EventHandler] Clarification needed: \(request.id) - \(request.prompt)")

        // Delegate to ClarificationManager which handles UI coordination
        // This blocks the Rust thread until user responds
        return clarificationManager.handleRequest(request)
    }

    // MARK: - Conversation (Multi-turn)

    /// Called when a new conversation session starts
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onConversationStarted(sessionId: String) {
        print("[EventHandler] Conversation started: \(sessionId)")

        // Delegate to ConversationManager
        conversationManager.onConversationStarted(sessionId: sessionId)
    }

    /// Called when a conversation turn is completed
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onConversationTurnCompleted(turn: ConversationTurn) {
        print("[EventHandler] Conversation turn completed: \(turn.turnId)")

        // Delegate to ConversationManager
        conversationManager.onConversationTurnCompleted(turn: turn)
    }

    /// Called when the AI response is ready and continuation input can be shown
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onConversationContinuationReady() {
        print("[EventHandler] Conversation continuation ready")

        // Delegate to ConversationManager which posts notification to HaloWindow
        conversationManager.onConversationContinuationReady()
    }

    /// Called when a conversation session ends
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onConversationEnded(sessionId: String, totalTurns: UInt32) {
        print("[EventHandler] Conversation ended: \(sessionId), total turns: \(totalTurns)")

        // Delegate to ConversationManager
        conversationManager.onConversationEnded(sessionId: sessionId, totalTurns: totalTurns)
    }

    // MARK: - Async Tool Confirmation (Phase 6)

    /// Called when a tool execution needs user confirmation (async flow)
    ///
    /// This is a NON-BLOCKING callback - the Rust core returns immediately.
    /// The UI shows a confirmation dialog and calls confirmAction() when user decides.
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onConfirmationNeeded(confirmation: PendingConfirmationInfo) {
        print("[EventHandler] Confirmation needed: \(confirmation.id) - \(confirmation.toolName)")
        print("[EventHandler]   Tool: \(confirmation.toolDisplayName)")
        print("[EventHandler]   Reason: \(confirmation.reason)")
        print("[EventHandler]   Confidence: \(confirmation.confidence)")

        DispatchQueue.main.async { [weak self] in
            self?.showToolConfirmation(confirmation)
        }
    }

    /// Called when a pending confirmation expires
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onConfirmationExpired(confirmationId: String) {
        print("[EventHandler] Confirmation expired: \(confirmationId)")

        DispatchQueue.main.async { [weak self] in
            self?.handleConfirmationExpired(confirmationId)
        }
    }

    // MARK: - Tool Confirmation UI

    /// Show tool confirmation dialog in Halo
    private func showToolConfirmation(_ confirmation: PendingConfirmationInfo) {
        // Build confirmation message
        let title = L("confirmation.tool_execution")
        let message = """
        \(confirmation.toolDisplayName)

        \(confirmation.reason)

        \(L("confirmation.confidence")): \(Int(confirmation.confidence * 100))%
        """

        // Show confirmation in Halo with action buttons
        haloWindow?.showToolConfirmation(
            confirmationId: confirmation.id,
            toolName: confirmation.toolDisplayName,
            toolDescription: confirmation.toolDescription,
            reason: confirmation.reason,
            confidence: confirmation.confidence,
            onExecute: { [weak self] in
                self?.handleUserConfirmation(confirmationId: confirmation.id, decision: .execute)
            },
            onCancel: { [weak self] in
                self?.handleUserConfirmation(confirmationId: confirmation.id, decision: .cancel)
            }
        )
    }

    /// Handle user's confirmation decision
    private func handleUserConfirmation(confirmationId: String, decision: UserConfirmationDecision) {
        print("[EventHandler] User confirmation decision: \(confirmationId) -> \(decision)")

        guard let core = core else {
            print("[EventHandler] Error: No AetherCore reference for confirmation")
            return
        }

        do {
            let success = try core.confirmAction(confirmationId: confirmationId, decision: decision)
            print("[EventHandler] Confirmation action result: \(success)")

            if decision == .cancel {
                // Hide Halo on cancel
                haloWindow?.hide()
            }
        } catch {
            print("[EventHandler] Confirmation action failed: \(error)")
            showToast(type: .error, title: L("error.aether"), message: error.localizedDescription, autoDismiss: false)
        }
    }

    /// Handle expired confirmation
    private func handleConfirmationExpired(_ confirmationId: String) {
        print("[EventHandler] Handling expired confirmation: \(confirmationId)")

        // Show timeout notification
        showToast(
            type: .warning,
            title: L("confirmation.expired"),
            message: L("confirmation.expired_message"),
            autoDismiss: true
        )

        // Hide Halo
        haloWindow?.hide()
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
            // Skip in conversation mode - the conversation input UI should remain visible
            if case .conversationInput = haloWindow?.viewModel.state {
                print("[EventHandler] Skipping success state - conversation input mode active")
                return
            }

            // Simply hide Halo on success - the AI response is already visible in the target window
            // No need to show a success icon, it just adds visual noise
            haloWindow?.hide()
            announceToVoiceOver("Request completed successfully")

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
            forName: .aetherConfigSavedInternally,
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
