//
//  HaloWindowController.swift
//  Aether
//
//  Controller for managing the Halo overlay window.
//  Extracted from AppDelegate to improve separation of concerns.
//

import AppKit
import SwiftUI

/// Controller for managing the Halo overlay window
///
/// Provides a simplified interface for:
/// - Creating and configuring the Halo window
/// - Showing/hiding the window at various positions
/// - Updating window state
/// - Managing event handler connections
final class HaloWindowController {

    // MARK: - Properties

    /// The managed Halo window instance
    private(set) var window: HaloWindow?

    /// Theme engine for styling
    private let themeEngine: ThemeEngine

    /// Whether the window has been initialized
    var isInitialized: Bool {
        window != nil
    }

    /// Direct access to the view model (convenience)
    var viewModel: HaloViewModel? {
        window?.viewModel
    }

    /// Direct access to the command manager (convenience)
    var commandManager: CommandCompletionManager? {
        window?.viewModel.commandManager
    }

    // MARK: - Initialization

    /// Initialize the controller with a theme engine
    ///
    /// - Parameter themeEngine: The theme engine for styling the Halo
    init(themeEngine: ThemeEngine) {
        self.themeEngine = themeEngine
    }

    /// Create and configure the Halo window
    ///
    /// Call this after initialization to create the actual window.
    /// This is separate from init to allow for lazy creation.
    func createWindow() {
        guard window == nil else {
            print("[HaloWindowController] Window already created")
            return
        }

        window = HaloWindow(themeEngine: themeEngine)
        print("[HaloWindowController] Halo window created")
    }

    // MARK: - Event Handler Connection

    /// Connect an event handler to the Halo window
    ///
    /// - Parameter handler: The event handler for Rust callbacks
    func setEventHandler(_ handler: EventHandler) {
        window?.setEventHandler(handler)
    }

    // MARK: - Core Configuration

    /// Configure the command manager with the Rust core
    ///
    /// - Parameter core: The AetherCore instance
    func configureCore(_ core: AetherCore) {
        window?.viewModel.commandManager.configure(core: core)
    }

    // MARK: - Display Methods

    /// Show the Halo at a specific position
    ///
    /// - Parameter position: The screen position to show at
    func show(at position: NSPoint) {
        window?.show(at: position)
    }

    /// Show the Halo below a specific position (for command mode)
    ///
    /// - Parameter position: The screen position to show below
    func showBelow(at position: NSPoint) {
        window?.showBelow(at: position)
    }

    /// Show the Halo at the current cursor position
    func showAtCurrentPosition() {
        window?.showAtCurrentPosition()
    }

    /// Show the Halo centered on screen
    func showCentered() {
        window?.showCentered()
    }

    /// Show a toast notification centered on screen
    func showToastCentered() {
        window?.showToastCentered()
    }

    /// Hide the Halo window with animation
    func hide() {
        window?.hide()
    }

    /// Force hide the Halo window immediately
    func forceHide() {
        window?.forceHide()
    }

    // MARK: - State Management

    /// Update the Halo state
    ///
    /// - Parameter state: The new state to display
    func updateState(_ state: HaloState) {
        window?.updateState(state)
    }

    /// Update typewriter progress
    ///
    /// - Parameter progress: Progress value (0.0 to 1.0)
    func updateTypewriterProgress(_ progress: Float) {
        window?.updateTypewriterProgress(progress)
    }

    // MARK: - Conversation Input

    /// Show conversation input UI
    ///
    /// - Parameter sessionId: The conversation session ID
    func showConversationInput(sessionId: String) {
        window?.showConversationInput(sessionId: sessionId)
    }

    // MARK: - Command Mode

    /// Enable mouse events for command mode
    func enableMouseEvents() {
        window?.ignoresMouseEvents = false
    }

    /// Disable mouse events (default transparent mode)
    func disableMouseEvents() {
        window?.ignoresMouseEvents = true
    }

    // MARK: - Command Mode (DEPRECATED)
    // These methods are deprecated and will be removed in Phase 8.
    // Use UnifiedInputCoordinator instead for command completion.

    /// Check if currently in command mode
    /// - Important: Deprecated. Use unified input mode instead.
    @available(*, deprecated, message: "Use unified input mode instead. Will be removed in Phase 8.")
    var isInCommandMode: Bool {
        guard let viewModel = window?.viewModel else { return false }
        if case .commandMode = viewModel.state {
            return true
        }
        return false
    }

    /// Activate command mode
    /// - Important: Deprecated. Use UnifiedInputCoordinator instead.
    ///
    /// - Parameters:
    ///   - position: Position to show the window
    ///   - onCommandSelected: Callback when a command is selected
    @available(*, deprecated, message: "Use UnifiedInputCoordinator instead. Will be removed in Phase 8.")
    func activateCommandMode(at position: NSPoint, onCommandSelected: @escaping (CommandNode) -> Void) {
        guard let window = window else { return }

        // Check if already in command mode
        if case .commandMode = window.viewModel.state {
            return
        }

        // Activate command mode
        window.viewModel.commandManager.activateCommandMode(onSelect: onCommandSelected)

        // Update state and show
        window.viewModel.state = .commandMode
        window.ignoresMouseEvents = false
        window.showBelow(at: position)
    }

    /// Deactivate command mode
    /// - Important: Deprecated. Use UnifiedInputCoordinator instead.
    @available(*, deprecated, message: "Use UnifiedInputCoordinator instead. Will be removed in Phase 8.")
    func deactivateCommandMode() {
        window?.viewModel.commandManager.deactivateCommandMode()
        window?.updateState(.idle)
        window?.hide()
    }

    /// Get the current input prefix from command mode
    var inputPrefix: String {
        window?.viewModel.commandManager.inputPrefix ?? ""
    }

    // MARK: - Convenience Methods

    /// Show processing state at a position
    ///
    /// - Parameters:
    ///   - position: Position to show at
    ///   - color: Provider color
    func showProcessing(at position: NSPoint, color: Color = .purple) {
        window?.show(at: position)
        window?.updateState(.processing(providerColor: color, streamingText: nil))
    }

    /// Show processing state centered
    ///
    /// - Parameter color: Provider color
    func showProcessingCentered(color: Color = .purple) {
        window?.showCentered()
        window?.updateState(.processing(providerColor: color, streamingText: nil))
    }

    /// Show error state
    ///
    /// - Parameters:
    ///   - type: Error type
    ///   - message: Error message
    ///   - suggestion: Optional suggestion
    ///   - position: Optional position (uses current if nil)
    func showError(type: ErrorType, message: String, suggestion: String? = nil, at position: NSPoint? = nil) {
        if let position = position {
            window?.show(at: position)
        }
        window?.updateState(.error(type: type, message: message, suggestion: suggestion))
    }

    /// Show success state
    ///
    /// - Parameter message: Optional success message
    func showSuccess(message: String? = nil) {
        window?.showAtCurrentPosition()
        window?.updateState(.success(finalText: message))
    }

    /// Show success then hide after delay
    ///
    /// - Parameters:
    ///   - message: Optional success message
    ///   - delay: Delay before hiding (default 1.5 seconds)
    func showSuccessThenHide(message: String? = nil, delay: TimeInterval = 1.5) {
        showSuccess(message: message)
        DispatchQueue.mainAsyncAfter(delay: delay, weakRef: self) { slf in
            slf.hide()
        }
    }
}
