//
//  KeyboardFlowHandlerProtocol.swift
//  Aether
//
//  Protocol for keyboard-driven UI flow handlers (Clarification, Conversation).
//  Enables separation of keyboard event handling from HaloWindow.
//

import Foundation
import AppKit

// MARK: - KeyboardFlowHandler Protocol

/// Protocol for handlers that manage keyboard-driven UI flows
///
/// This protocol enables the separation of keyboard event handling logic
/// from HaloWindow, improving testability and maintainability.
///
/// Implementations:
/// - ClarificationFlowHandler: Handles Phantom Flow clarification UI
/// - ConversationFlowHandler: Handles multi-turn conversation UI
protocol KeyboardFlowHandler: AnyObject {
    /// Whether this handler is currently active
    var isActive: Bool { get }

    /// Activate the handler and start keyboard monitoring
    /// - Parameter window: The window to attach this handler to
    func activate(window: HaloWindow)

    /// Deactivate the handler and stop keyboard monitoring
    func deactivate()

    /// Handle a keyboard event
    /// - Parameter event: The keyboard event to handle
    /// - Returns: true if the event was consumed, false to pass through
    func handleKeyEvent(_ event: NSEvent) -> Bool
}

// MARK: - KeyboardFlowDelegate

/// Delegate for keyboard flow handlers to communicate with HaloWindow
protocol KeyboardFlowDelegate: AnyObject {
    /// Called when the flow needs to hide the window
    func flowDidRequestHide()

    /// Called when the flow needs to force hide the window
    func flowDidRequestForceHide()

    /// Called when the flow completes successfully
    /// - Parameter result: Optional result data
    func flowDidComplete(with result: Any?)

    /// Called when the flow is cancelled
    func flowDidCancel()

    /// Update the HaloWindow state
    /// - Parameter state: New state to set
    func updateState(_ state: HaloState)

    /// Update window's ignoresMouseEvents property
    /// - Parameter ignores: Whether to ignore mouse events
    func setIgnoresMouseEvents(_ ignores: Bool)
}

// MARK: - Default Extension

extension KeyboardFlowHandler {
    /// Default implementation: not active
    var isActive: Bool { false }
}
