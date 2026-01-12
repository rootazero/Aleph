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

    // MARK: - Multi-Turn Mode Detection

    /// Unified check for multi-turn conversation mode
    ///
    /// Returns true if either:
    /// - ConversationManager has an active session (sessionId != nil)
    /// - MultiTurnCoordinator is in active state
    ///
    /// Use this property instead of inline checks to ensure consistency
    /// and simplify future logic changes.
    private var isInMultiTurnMode: Bool {
        conversationManager.sessionId != nil || MultiTurnCoordinator.shared.isMultiTurnActive
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
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.handleStateChange(state)
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode - SubPanel handles UI feedback
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping halo error animation (multi-turn mode)")
                slf.showErrorNotification(message: message, suggestion: suggestion)
                return
            }

            // Calculate delay to ensure Halo displays for at least minHaloDisplayTime
            // showTime is tracked by HaloWindow when show() is called
            let delay: TimeInterval
            if let showTime = slf.haloWindow?.showTime {
                let elapsed = Date().timeIntervalSince(showTime)
                delay = max(0, slf.minHaloDisplayTime - elapsed)
                print("[EventHandler] Halo displayed for \(String(format: "%.2f", elapsed))s, delaying toast by \(String(format: "%.2f", delay))s")
            } else {
                // Halo not showing - use minimum display time as delay
                // This ensures user sees the Halo animation before error
                delay = slf.minHaloDisplayTime
                print("[EventHandler] No Halo show time recorded, using minimum delay: \(slf.minHaloDisplayTime)s")

                // CRITICAL: Show Halo animation first if not already visible
                // This handles the race condition where error fires before Halo shows
                // Use .processing state with purple to show the theme's processing animation
                // (purple + 3 arcs for Zen theme)
                slf.haloWindow?.updateState(.processing(streamingText: nil))
                slf.haloWindow?.showCentered()
                print("[EventHandler] Showing Halo animation before error toast")
            }

            // Show error toast after delay
            DispatchQueue.mainAsyncAfter(delay: delay, weakRef: slf) { innerSlf in
                innerSlf.showErrorNotification(message: message, suggestion: suggestion)
            }
        }
    }

    // NEW: Handle streaming response chunks
    func onResponseChunk(text: String) {
        print("[EventHandler] Response chunk: \(text.prefix(50))...")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.handleResponseChunk(text: text)
        }
    }

    // NEW: Handle typed errors
    func onErrorTyped(errorType: ErrorType, message: String) {
        print("[EventHandler] Typed error (\(errorType)): \(message)")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.handleTypedError(errorType: errorType, message: message)
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.handleAiProcessingStarted(providerName: providerName, providerColor: providerColor)
        }
    }

    func onAiResponseReceived(responsePreview: String) {
        print("[EventHandler] AI response received: \(responsePreview.prefix(100))...")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.handleAiResponseReceived(responsePreview: responsePreview)
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Update Halo with typewriter progress
            slf.haloWindow?.updateTypewriterProgress(percent)

            // Announce progress milestones to VoiceOver (every 25%)
            let progress = Int(percent * 100)
            if progress % 25 == 0 && progress > 0 {
                slf.announceToVoiceOver("Typewriter \(progress) percent complete")
            }
        }
    }

    // Typewriter Cancelled Callback
    func onTypewriterCancelled() {
        print("[EventHandler] Typewriter cancelled by user")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Show brief notification or just hide progress
            // Success state removed - just hide
            slf.haloWindow?.hide()
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.showToolConfirmation(confirmation)
        }
    }

    /// Called when a pending confirmation expires
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onConfirmationExpired(confirmationId: String) {
        print("[EventHandler] Confirmation expired: \(confirmationId)")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.handleConfirmationExpired(confirmationId)
        }
    }

    // MARK: - Tool Confirmation UI

    /// Show tool confirmation dialog in Halo
    private func showToolConfirmation(_ confirmation: PendingConfirmationInfo) {
        // Skip halo in multi-turn mode - auto-execute for smoother conversation flow
        guard !isInMultiTurnMode else {
            print("[EventHandler] Skipping tool confirmation halo (multi-turn mode)")
            handleUserConfirmation(confirmationId: confirmation.id, decision: .execute)
            return
        }

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

        // Skip halo operations in multi-turn mode
        guard !isInMultiTurnMode else {
            print("[EventHandler] Skipping confirmation expired handling (multi-turn mode)")
            return
        }

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

    // MARK: - Tool Registry Callbacks (unify-tool-registry)

    /// Called when tool registry is refreshed
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onToolsChanged(toolCount: UInt32) {
        print("[EventHandler] Tools changed: \(toolCount) tools available")

        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .toolsDidChange,
                object: nil,
                userInfo: ["toolCount": toolCount]
            )
        }
    }

    // MARK: - Agent Loop Callbacks (enhance-l3-agent-planning)

    /// Called when agent loop starts executing a multi-step plan
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onAgentStarted(planId: String, totalSteps: UInt32, description: String) {
        print("[EventHandler] Agent started: planId=\(planId), steps=\(totalSteps)")
        print("[EventHandler]   Description: \(description)")

        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .agentStarted,
                object: nil,
                userInfo: [
                    "planId": planId,
                    "totalSteps": totalSteps,
                    "description": description
                ]
            )
        }
    }

    /// Called when agent starts executing a tool
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onAgentToolStarted(planId: String, stepIndex: UInt32, toolName: String, toolDescription: String) {
        print("[EventHandler] Agent tool started: planId=\(planId), step=\(stepIndex), tool=\(toolName)")

        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .agentToolStarted,
                object: nil,
                userInfo: [
                    "planId": planId,
                    "stepIndex": stepIndex,
                    "toolName": toolName,
                    "toolDescription": toolDescription
                ]
            )
        }
    }

    /// Called when agent tool execution completes
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onAgentToolCompleted(planId: String, stepIndex: UInt32, toolName: String, success: Bool, resultPreview: String) {
        print("[EventHandler] Agent tool completed: planId=\(planId), step=\(stepIndex), tool=\(toolName), success=\(success)")

        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .agentToolCompleted,
                object: nil,
                userInfo: [
                    "planId": planId,
                    "stepIndex": stepIndex,
                    "toolName": toolName,
                    "success": success,
                    "resultPreview": resultPreview
                ]
            )
        }
    }

    /// Called when agent loop completes (success or failure)
    ///
    /// NOTE: This method is called from a background thread by Rust/UniFFI.
    func onAgentCompleted(planId: String, success: Bool, totalDurationMs: UInt64, finalResponse: String) {
        print("[EventHandler] Agent completed: planId=\(planId), success=\(success), duration=\(totalDurationMs)ms")

        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .agentCompleted,
                object: nil,
                userInfo: [
                    "planId": planId,
                    "success": success,
                    "totalDurationMs": totalDurationMs,
                    "finalResponse": finalResponse
                ]
            )
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
            // Skip halo in multi-turn mode - MultiTurnInputWindow handles UI
            guard !isInMultiTurnMode else {
                print("[EventHandler] Skipping listening state (multi-turn mode)")
                return
            }
            haloWindow?.updateState(.listening)
            haloWindow?.show(at: NSEvent.mouseLocation)
            accumulatedText = ""
            announceToVoiceOver("Listening for input")

        case .retrievingMemory:
            // Skip halo in multi-turn mode
            guard !isInMultiTurnMode else {
                print("[EventHandler] Skipping retrievingMemory state (multi-turn mode)")
                return
            }
            haloWindow?.updateState(.retrievingMemory)
            haloWindow?.showAtCurrentPosition()
            announceToVoiceOver("Retrieving memories")

        case .processingWithAi:
            // Skip halo in multi-turn mode
            guard !isInMultiTurnMode else {
                print("[EventHandler] Skipping processing state (multi-turn mode)")
                return
            }
            haloWindow?.updateState(.processingWithAI(providerName: nil))
            haloWindow?.showAtCurrentPosition()
            announceToVoiceOver("Processing with AI")

        case .processing:
            // Skip halo in multi-turn mode
            guard !isInMultiTurnMode else {
                print("[EventHandler] Skipping processing state (multi-turn mode)")
                return
            }
            haloWindow?.updateState(.processing(streamingText: nil))
            haloWindow?.showAtCurrentPosition()
            announceToVoiceOver("Processing request")

        case .success:
            // Success state removed from HaloState - just hide and announce
            // Skip in conversation mode - the conversation input UI should remain visible
            if case .conversationInput = haloWindow?.viewModel.state {
                print("[EventHandler] Skipping success state - conversation input mode active")
                return
            }
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
        accumulatedText = text

        // Skip halo update in multi-turn mode
        guard !isInMultiTurnMode else { return }

        haloWindow?.updateState(.processing(streamingText: text))
        lastUpdateTime = Date()
    }

    // MARK: - AI Processing Handling

    private func handleAiProcessingStarted(providerName: String, providerColor: String) {
        // Skip halo in multi-turn mode - SubPanel handles UI
        guard !isInMultiTurnMode else {
            print("[EventHandler] AI processing started (multi-turn, no halo): \(providerName)")
            return
        }

        _ = providerColor  // Unused (unified purple theme)
        haloWindow?.updateState(.processingWithAI(providerName: providerName))
        haloWindow?.showAtCurrentPosition()
        print("[EventHandler] AI processing started: \(providerName)")
    }

    private func handleAiResponseReceived(responsePreview: String) {
        accumulatedText = responsePreview

        // Skip halo update in multi-turn mode
        guard !isInMultiTurnMode else { return }

        haloWindow?.updateState(.processing(streamingText: responsePreview))
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

        guard core != nil else {
            print("[EventHandler] Error: No AetherCore reference to retry")
            return
        }

        // TODO: Implement retry - retryLastRequest not yet exported to UniFFI
        print("[EventHandler] Retry not yet implemented")
        handleTypedError(errorType: .unknown, message: "Retry feature not yet available")
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Update Halo state to toast
            let shouldAutoDismiss = autoDismiss && type == .info
            slf.haloWindow?.updateState(.toast(
                type: type,
                title: title,
                message: message,
                autoDismiss: shouldAutoDismiss
            ))

            // Set dismiss callback separately (closures stored outside HaloState for Equatable)
            slf.haloWindow?.viewModel.callbacks.toastOnDismiss = { [weak slf] in
                slf?.dismissToast()
            }

            // Show at screen center
            slf.haloWindow?.showToastCentered()

            // Setup auto-dismiss timer for info toasts
            if shouldAutoDismiss {
                slf.toastDismissTimer = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: false) { [weak slf] _ in
                    slf?.dismissToast()
                }
            }
        }
    }

    /// Dismiss current toast notification
    func dismissToast() {
        print("[EventHandler] Dismissing toast")

        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
        }
    }

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
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
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
