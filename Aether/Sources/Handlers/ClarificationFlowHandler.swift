//
//  ClarificationFlowHandler.swift
//  Aether
//
//  Handles keyboard-driven clarification (Phantom Flow) UI.
//  Extracted from HaloWindow for better separation of concerns.
//

import Foundation
import AppKit

/// Handles Phantom Flow clarification UI and keyboard events
///
/// This class manages:
/// - Clarification request notifications
/// - Keyboard navigation (arrow keys, Enter, Escape, number keys)
/// - Local/global event monitors based on clarification type
/// - Communication with ClarificationManager
@MainActor
final class ClarificationFlowHandler: KeyboardFlowHandler {

    // MARK: - Properties

    /// Whether this handler is currently active
    private(set) var isActive: Bool = false

    /// Delegate for communicating with HaloWindow
    weak var delegate: KeyboardFlowDelegate?

    /// Clarification manager for state management
    private var clarificationManager: any ClarificationManagerProtocol {
        DependencyContainer.shared.clarificationManager
    }

    /// Current clarification request
    private var currentRequest: ClarificationRequest?

    /// Keyboard event monitor
    private var keyMonitor: Any?

    /// Observer for clarification notifications
    private var notificationObserver: NSObjectProtocol?

    /// Reference to the window (for activation)
    private weak var window: HaloWindow?

    // MARK: - Initialization

    init() {}

    deinit {
        // Cleanup notification observer directly (not calling MainActor methods)
        if let observer = notificationObserver {
            NotificationCenter.default.removeObserver(observer)
        }
        if let monitor = keyMonitor {
            NSEvent.removeMonitor(monitor)
        }
    }

    // MARK: - KeyboardFlowHandler Protocol

    func activate(window: HaloWindow) {
        self.window = window
        setupNotificationObserver()
        isActive = true
        NSLog("[ClarificationFlowHandler] Activated")
    }

    func deactivate() {
        removeKeyMonitor()
        removeNotificationObserver()
        currentRequest = nil
        isActive = false
        NSLog("[ClarificationFlowHandler] Deactivated")
    }

    func handleKeyEvent(_ event: NSEvent) -> Bool {
        guard let request = currentRequest else { return false }
        return handleClarificationKeyEvent(event, request: request)
    }

    // MARK: - Notification Observer

    private func setupNotificationObserver() {
        removeNotificationObserver()

        notificationObserver = NotificationCenter.default.addObserver(
            forName: .clarificationRequested,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let request = notification.object as? ClarificationRequest else { return }
            self?.showClarification(request)
        }
    }

    private func removeNotificationObserver() {
        if let observer = notificationObserver {
            NotificationCenter.default.removeObserver(observer)
            notificationObserver = nil
        }
    }

    // MARK: - Clarification Display

    /// Show clarification UI
    private func showClarification(_ request: ClarificationRequest) {
        guard let window = window else { return }

        NSLog("[ClarificationFlowHandler] Showing clarification: %@", request.id)

        currentRequest = request

        // Calculate size based on request type
        let width: CGFloat = 320
        let height: CGFloat
        if let options = request.options {
            height = CGFloat(80 + options.count * 48)
        } else {
            height = 140
        }

        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            NSLog("[ClarificationFlowHandler] Warning: No screen found")
            return
        }

        let windowSize = NSSize(width: width, height: height)
        let screenFrame = screen.visibleFrame
        let windowOrigin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )

        // Set frame without animation first
        window.setFrame(NSRect(origin: windowOrigin, size: windowSize), display: false)

        // Update state to clarification
        delegate?.updateState(.clarification(request: request))

        // Enable mouse events for interaction
        delegate?.setIgnoresMouseEvents(false)

        // For text-type: activate window for TextField focus
        // For select-type: use orderFrontRegardless to preserve focus
        if request.clarificationType == .text {
            window.makeKeyAndOrderFront(nil)
            NSLog("[ClarificationFlowHandler] Text clarification - activating window")
        } else {
            window.orderFrontRegardless()
        }

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            window.animator().alphaValue = 1.0
        })

        // Setup keyboard monitor
        setupKeyMonitor(for: request)
    }

    // MARK: - Keyboard Monitoring

    private func setupKeyMonitor(for request: ClarificationRequest) {
        removeKeyMonitor()

        if request.clarificationType == .text {
            // Local monitor for key window (text input mode)
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self = self, self.isActive else { return event }

                if self.handleKeyEvent(event) {
                    return nil  // Consume the event
                }
                return event
            }
        } else {
            // Global monitor for non-key window (select mode)
            keyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self = self, self.isActive else { return }

                DispatchQueue.main.async {
                    _ = self.handleKeyEvent(event)
                }
            }
        }
    }

    private func removeKeyMonitor() {
        if let monitor = keyMonitor {
            NSEvent.removeMonitor(monitor)
            keyMonitor = nil
        }
    }

    // MARK: - Key Event Handling

    private func handleClarificationKeyEvent(_ event: NSEvent, request: ClarificationRequest) -> Bool {
        let manager = clarificationManager

        // For text mode, handle Enter and Escape
        if request.clarificationType == .text {
            if event.keyCode == 36 { // Return/Enter - submit text
                let text = manager.textInput
                if !text.isEmpty {
                    completeClarification()
                    manager.completeWithText(text)
                    NSLog("[ClarificationFlowHandler] Text submitted: %@", text)
                }
                return true
            } else if event.keyCode == 53 { // Escape - cancel
                cancelClarification()
                manager.cancel()
                return true
            }
            return false
        }

        // For select mode
        guard let options = request.options, !options.isEmpty else { return false }

        switch event.keyCode {
        case 125: // Down arrow
            let newIndex = min(manager.selectedIndex + 1, options.count - 1)
            manager.selectedIndex = newIndex
            return true

        case 126: // Up arrow
            let newIndex = max(manager.selectedIndex - 1, 0)
            manager.selectedIndex = newIndex
            return true

        case 36: // Return/Enter
            let index = manager.selectedIndex
            if index < options.count {
                completeClarification()
                manager.completeWithSelection(index: index, value: options[index].value)
            }
            return true

        case 53: // Escape
            cancelClarification()
            manager.cancel()
            return true

        case 18...26: // Number keys 1-9
            let numberIndex = Int(event.keyCode) - 18
            if numberIndex < options.count {
                manager.selectedIndex = numberIndex
                completeClarification()
                manager.completeWithSelection(index: numberIndex, value: options[numberIndex].value)
            }
            return true

        default:
            return false
        }
    }

    // MARK: - Flow Completion

    private func completeClarification() {
        removeKeyMonitor()
        currentRequest = nil
        delegate?.setIgnoresMouseEvents(true)
        delegate?.flowDidRequestHide()
    }

    private func cancelClarification() {
        removeKeyMonitor()
        currentRequest = nil
        delegate?.setIgnoresMouseEvents(true)
        delegate?.flowDidRequestHide()
        delegate?.flowDidCancel()
    }
}
