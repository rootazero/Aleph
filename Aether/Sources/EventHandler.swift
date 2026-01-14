//
//  EventHandler.swift
//  Aether
//
//  Implements AetherEventHandler protocol to receive callbacks from Rust core.
//  This handler works with the rig-core based AetherCore.
//

import Foundation
import AppKit
import SwiftUI

/// Event Handler implementing the simplified rig-core callback protocol
///
/// This handler provides callbacks for:
/// - AI thinking/processing states
/// - Tool execution lifecycle
/// - Streaming response chunks
/// - Completion and error states
/// - Memory storage confirmation
class EventHandler: AetherEventHandler {

    // MARK: - Dependencies

    // Weak reference to Halo window to avoid retain cycle
    private weak var haloWindow: HaloWindow?

    // Weak reference to AetherCore for cancellation functionality
    private weak var core: AetherCore?

    // Weak reference to InputCoordinator for output handling
    private weak var inputCoordinator: InputCoordinator?

    // Managers accessed through DependencyContainer
    private var conversationManager: any ConversationManagerProtocol {
        DependencyContainer.shared.conversationManager
    }

    // MARK: - State

    // Accumulated text for streaming responses
    private var accumulatedText: String = ""

    // Current tool being executed (for UI feedback)
    private var currentToolName: String?

    // Check for multi-turn conversation mode
    private var isInMultiTurnMode: Bool {
        conversationManager.sessionId != nil || MultiTurnCoordinator.shared.isMultiTurnActive
    }

    // MARK: - Initialization

    init(haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
    }

    // Set AetherCore reference after initialization
    func setCore(_ core: AetherCore) {
        self.core = core
    }

    // Set HaloWindow reference (for DependencyContainer use)
    func setHaloWindow(_ window: HaloWindow?) {
        self.haloWindow = window
    }

    // Set InputCoordinator reference for output handling
    func setInputCoordinator(_ coordinator: InputCoordinator?) {
        self.inputCoordinator = coordinator
    }

    // MARK: - AetherEventHandler Protocol

    /// Called when AI is processing/thinking
    func onThinking() {
        print("[EventHandler] AI thinking...")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping thinking state (multi-turn mode)")
                return
            }

            slf.haloWindow?.updateState(.processingWithAI(providerName: nil))
            slf.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a tool execution starts
    /// - Parameter toolName: Name of the tool being executed
    func onToolStart(toolName: String) {
        print("[EventHandler] Tool started: \(toolName)")
        currentToolName = toolName

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping tool start state (multi-turn mode)")
                return
            }

            // Show processing state with tool name
            slf.haloWindow?.updateState(.processing(streamingText: "Using \(toolName)..."))
            slf.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a tool execution completes
    /// - Parameters:
    ///   - toolName: Name of the tool that completed
    ///   - result: Result from the tool (may be truncated for display)
    func onToolResult(toolName: String, result: String) {
        print("[EventHandler] Tool result: \(toolName) - \(result.prefix(100))...")
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping tool result state (multi-turn mode)")
                return
            }

            // Update state to show tool completed
            slf.haloWindow?.updateState(.processing(streamingText: "Completed: \(toolName)"))
        }
    }

    /// Called for each streaming response chunk
    /// - Parameter text: The accumulated response text so far
    func onStreamChunk(text: String) {
        // Only log first call and on significant changes to avoid log spam
        if accumulatedText.isEmpty || text.count - accumulatedText.count > 50 {
            print("[EventHandler] Stream chunk: \(text.prefix(50))... (total: \(text.count) chars)")
        }

        accumulatedText = text

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            slf.haloWindow?.updateState(.processing(streamingText: text))
        }
    }

    /// Called when processing completes successfully
    /// - Parameter response: The final response text
    func onComplete(response: String) {
        print("[EventHandler] Processing complete: \(response.prefix(100))...")

        // Reset state
        accumulatedText = ""
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Notify InputCoordinator if processing is pending
            if slf.inputCoordinator?.isProcessingPending == true {
                slf.inputCoordinator?.handleCompletion(response: response)
                return
            }

            // Notify MultiTurnCoordinator if processing is pending
            if MultiTurnCoordinator.shared.isProcessingPending {
                MultiTurnCoordinator.shared.handleCompletion(response: response)
                return
            }

            // Skip halo in multi-turn mode - conversation UI handles it
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping completion state (multi-turn mode)")
                return
            }

            // Show success state then auto-hide
            slf.haloWindow?.updateState(.success(message: nil))

            // Auto-hide after brief display
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak slf] in
                slf?.haloWindow?.hide()
            }
        }
    }

    /// Called when an error occurs during processing
    /// - Parameter message: Error message describing what went wrong
    func onError(message: String) {
        print("[EventHandler] Error: \(message)")

        // Reset state
        accumulatedText = ""
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Notify InputCoordinator if processing is pending
            if slf.inputCoordinator?.isProcessingPending == true {
                slf.inputCoordinator?.handleError(message: message)
                // Still show error notification
                slf.showErrorNotification(message: message)
                return
            }

            // Notify MultiTurnCoordinator if processing is pending
            if MultiTurnCoordinator.shared.isProcessingPending {
                MultiTurnCoordinator.shared.handleError(message: message)
                // Multi-turn mode shows error in conversation UI, no halo notification
                return
            }

            // Show error notification even in multi-turn mode
            slf.showErrorNotification(message: message)
        }
    }

    /// Called when memory is stored successfully
    func onMemoryStored() {
        print("[EventHandler] Memory stored")

        // Subtle feedback - could show toast or just log
        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Optionally show brief memory stored indicator
            // For now, just log - memory storage is typically transparent to user
        }
    }

    // MARK: - Error Notification

    private func showErrorNotification(message: String) {
        // Skip halo in multi-turn mode - just show notification
        guard !isInMultiTurnMode else {
            print("[EventHandler] Showing error notification (multi-turn mode)")
            // Could show system notification here
            return
        }

        // Use toast notification in Halo
        haloWindow?.updateState(.toast(
            type: .error,
            title: L("error.aether"),
            message: message,
            autoDismiss: false
        ))

        // Set dismiss callback
        haloWindow?.viewModel.callbacks.toastOnDismiss = { [weak self] in
            self?.haloWindow?.hide()
        }

        // Show at screen center
        haloWindow?.showToastCentered()
    }

    // MARK: - Helper Methods

    /// Cancel the current operation
    func cancel() {
        core?.cancel()
        accumulatedText = ""
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
        }
    }

    /// Reset handler state
    func reset() {
        accumulatedText = ""
        currentToolName = nil
    }

    // MARK: - Toast Display

    /// Timer for auto-dismissing toasts
    private var toastDismissTimer: Timer?

    /// Show a toast notification to the user
    /// - Parameters:
    ///   - type: The toast type (info, warning, error)
    ///   - title: Toast title
    ///   - message: Toast message
    ///   - autoDismiss: Whether to auto-dismiss (default: true for info)
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

            // Set dismiss callback
            slf.haloWindow?.viewModel.callbacks.toastOnDismiss = { [weak slf] in
                slf?.dismissToast()
            }

            // Show at screen center
            slf.haloWindow?.showToastCentered()

            // Set auto-dismiss timer for info toasts
            if shouldAutoDismiss {
                slf.toastDismissTimer = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: false) { [weak slf] _ in
                    slf?.dismissToast()
                }
            }
        }
    }

    /// Dismiss the current toast
    private func dismissToast() {
        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
        }
    }
}
