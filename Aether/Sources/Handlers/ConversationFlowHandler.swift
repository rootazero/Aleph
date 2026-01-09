//
//  ConversationFlowHandler.swift
//  Aether
//
//  Handles keyboard-driven multi-turn conversation UI.
//  Extracted from HaloWindow for better separation of concerns.
//

import Foundation
import AppKit

/// Handles multi-turn conversation UI and keyboard events
///
/// This class manages:
/// - Conversation continuation notifications
/// - Keyboard handling (Enter to submit, Escape to cancel)
/// - Local and global event monitors
/// - Communication with ConversationManager
@MainActor
final class ConversationFlowHandler: KeyboardFlowHandler {

    // MARK: - Properties

    /// Whether this handler is currently active
    private(set) var isActive: Bool = false

    /// Delegate for communicating with HaloWindow
    weak var delegate: KeyboardFlowDelegate?

    /// Conversation manager for state management
    private var conversationManager: any ConversationManagerProtocol {
        DependencyContainer.shared.conversationManager
    }

    /// Current session ID
    private var currentSessionId: String?

    /// Local keyboard event monitor
    private var keyMonitor: Any?

    /// Global keyboard event monitor (fallback for ESC)
    private var globalKeyMonitor: Any?

    /// Observer for conversation notifications
    private var notificationObserver: NSObjectProtocol?

    /// Reference to the window (for activation)
    private weak var window: HaloWindow?

    // MARK: - Initialization

    init() {}

    deinit {
        // Cleanup notification observer and key monitors directly (not calling MainActor methods)
        if let observer = notificationObserver {
            NotificationCenter.default.removeObserver(observer)
        }
        if let monitor = keyMonitor {
            NSEvent.removeMonitor(monitor)
        }
        if let monitor = globalKeyMonitor {
            NSEvent.removeMonitor(monitor)
        }
    }

    // MARK: - KeyboardFlowHandler Protocol

    func activate(window: HaloWindow) {
        self.window = window
        setupNotificationObserver()
        isActive = true
        NSLog("[ConversationFlowHandler] Activated")
    }

    func deactivate() {
        removeKeyMonitors()
        removeNotificationObserver()
        currentSessionId = nil
        isActive = false
        NSLog("[ConversationFlowHandler] Deactivated")
    }

    func handleKeyEvent(_ event: NSEvent) -> Bool {
        return handleConversationKeyEvent(event)
    }

    // MARK: - Notification Observer

    private func setupNotificationObserver() {
        removeNotificationObserver()

        notificationObserver = NotificationCenter.default.addObserver(
            forName: .conversationContinuationReady,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let sessionId = notification.object as? String else { return }
            MainActor.assumeIsolated {
                self?.showConversationInput(sessionId: sessionId)
            }
        }
    }

    private func removeNotificationObserver() {
        if let observer = notificationObserver {
            NotificationCenter.default.removeObserver(observer)
            notificationObserver = nil
        }
    }

    // MARK: - Conversation Display

    /// Show conversation input UI at screen center
    func showConversationInput(sessionId: String) {
        guard let window = window else { return }

        let turnCount = conversationManager.turnCount
        NSLog("[ConversationFlowHandler] Showing conversation input: sessionId=%@, turn=%d", sessionId, turnCount)

        currentSessionId = sessionId

        // Fixed size for conversation input
        let windowSize = NSSize(width: 480, height: 118)

        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            NSLog("[ConversationFlowHandler] Warning: No screen found")
            return
        }

        let screenFrame = screen.visibleFrame
        let windowOrigin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )

        // Set frame without animation first
        window.setFrame(NSRect(origin: windowOrigin, size: windowSize), display: false)

        // Update state to conversation input
        delegate?.updateState(.conversationInput(sessionId: sessionId, turnCount: turnCount))

        // Enable mouse events for input interaction
        delegate?.setIgnoresMouseEvents(false)

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            window.animator().alphaValue = 1.0
        })

        // Activate window after a short delay
        DispatchQueue.mainAsyncAfter(delay: 0.1, weakRef: self) { slf in
            guard let window = slf.window else { return }

            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)

            NSLog("[ConversationFlowHandler] Window activated: isKey=%@, canBecomeKey=%@",
                  window.isKeyWindow ? "YES" : "NO",
                  window.canBecomeKey ? "YES" : "NO")

            slf.setupKeyMonitors()

            // Retry activation if needed
            DispatchQueue.mainAsyncAfter(delay: 0.2, weakRef: slf) { innerSlf in
                guard let window = innerSlf.window, !window.isKeyWindow else { return }
                NSLog("[ConversationFlowHandler] Retrying window activation...")
                NSApp.activate(ignoringOtherApps: true)
                window.makeKeyAndOrderFront(nil)
            }
        }
    }

    // MARK: - Keyboard Monitoring

    private func setupKeyMonitors() {
        removeKeyMonitors()

        // Local monitor when window is key
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self, self.isActive else { return event }

            if self.handleKeyEvent(event) {
                return nil  // Consume the event
            }
            return event
        }

        // Global monitor as fallback (for ESC when window fails to become key)
        globalKeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self, self.isActive else { return }

            // Only handle ESC key globally
            if event.keyCode == 53 {  // Escape
                DispatchQueue.mainAsync(weakRef: self) { slf in
                    slf.cancelConversation()
                    NSLog("[ConversationFlowHandler] Conversation cancelled (global monitor)")
                }
            }
        }
    }

    private func removeKeyMonitors() {
        if let monitor = keyMonitor {
            NSEvent.removeMonitor(monitor)
            keyMonitor = nil
        }

        if let monitor = globalKeyMonitor {
            NSEvent.removeMonitor(monitor)
            globalKeyMonitor = nil
        }
    }

    // MARK: - Key Event Handling

    private func handleConversationKeyEvent(_ event: NSEvent) -> Bool {
        let manager = conversationManager

        switch event.keyCode {
        case 36:  // Return/Enter - submit input
            let text = manager.textInput.trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                submitConversation(text: text)
                NSLog("[ConversationFlowHandler] Input submitted: %@", String(text.prefix(50)))
            }
            return true

        case 53:  // Escape - cancel conversation
            cancelConversation()
            NSLog("[ConversationFlowHandler] Conversation cancelled")
            return true

        default:
            return false
        }
    }

    // MARK: - Flow Completion

    private func submitConversation(text: String) {
        removeKeyMonitors()
        currentSessionId = nil

        // Reset state before hiding
        delegate?.updateState(.idle)
        conversationManager.submitContinuationInput(text)
        delegate?.setIgnoresMouseEvents(true)
        delegate?.flowDidRequestForceHide()
    }

    private func cancelConversation() {
        removeKeyMonitors()
        currentSessionId = nil

        // Reset state before hiding
        delegate?.updateState(.idle)
        conversationManager.cancelConversation()
        delegate?.setIgnoresMouseEvents(true)
        delegate?.flowDidRequestForceHide()
        delegate?.flowDidCancel()
    }
}
