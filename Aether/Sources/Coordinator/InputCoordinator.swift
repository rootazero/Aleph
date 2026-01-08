//
//  InputCoordinator.swift
//  Aether
//
//  Coordinator for managing input capture from target applications.
//  Extracted from AppDelegate to improve separation of concerns.
//

import AppKit
import SwiftUI

// MARK: - Input Coordinator

/// Coordinator for managing input capture operations
///
/// Responsibilities:
/// - Handle replace and append triggers
/// - Capture text from clipboard and accessibility API
/// - Manage frontmost app tracking
/// - Coordinate with Halo for visual feedback
final class InputCoordinator {

    // MARK: - Dependencies

    /// Reference to core for processing
    private weak var core: AetherCore?

    /// Reference to Halo window controller for state updates
    private weak var haloWindowController: HaloWindowController?

    /// Reference to event handler for error callbacks
    private weak var eventHandler: EventHandler?

    /// Clipboard manager for clipboard operations
    private let clipboardManager: any ClipboardManagerProtocol

    // MARK: - State

    /// Store the frontmost app when hotkey is pressed
    private(set) var previousFrontmostApp: NSRunningApplication?

    /// Whether permission gate is active (blocks input)
    var isPermissionGateActive: Bool = false

    /// Callback for processing input with mode
    /// This allows AppDelegate to handle the complex processWithInputMode logic
    /// until it can be fully migrated to InputCoordinator
    var onProcessInput: ((Bool) -> Void)?

    // MARK: - Initialization

    /// Initialize the input coordinator
    ///
    /// - Parameter clipboardManager: Clipboard manager for operations
    init(clipboardManager: any ClipboardManagerProtocol = ClipboardManager.shared) {
        self.clipboardManager = clipboardManager
    }

    /// Configure dependencies after initialization
    ///
    /// - Parameters:
    ///   - core: AetherCore instance
    ///   - haloWindowController: HaloWindowController for state updates
    ///   - eventHandler: EventHandler for error callbacks
    func configure(
        core: AetherCore,
        haloWindowController: HaloWindowController?,
        eventHandler: EventHandler?
    ) {
        self.core = core
        self.haloWindowController = haloWindowController
        self.eventHandler = eventHandler
    }

    // MARK: - Trigger Handlers

    /// Handle Replace trigger (double-tap replace hotkey, default: left Shift)
    ///
    /// AI response replaces the original selected text.
    func handleReplaceTriggered() {
        print("[InputCoordinator] 🔄 Replace triggered")

        // Block if permission gate is active or core not initialized
        guard !isPermissionGateActive, core != nil else {
            print("[InputCoordinator] ⚠️ Replace blocked - permission gate or core not ready")
            NSSound.beep()
            return
        }

        // Store frontmost app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[InputCoordinator] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Get best position for Halo
        let haloPosition = CaretPositionHelper.getBestPosition()

        // Show Halo immediately with processing state
        if Thread.isMainThread {
            haloWindowController?.show(at: haloPosition)
            haloWindowController?.updateState(.processing(providerColor: .purple, streamingText: nil))
        } else {
            DispatchQueue.main.sync { [weak self] in
                self?.haloWindowController?.show(at: haloPosition)
                self?.haloWindowController?.updateState(.processing(providerColor: .purple, streamingText: nil))
            }
        }

        // Process with replace mode (AI response replaces original text)
        onProcessInput?(true)
    }

    /// Handle Append trigger (double-tap append hotkey, default: right Shift)
    ///
    /// AI response appends after the original selected text.
    func handleAppendTriggered() {
        print("[InputCoordinator] ➕ Append triggered")

        // Block if permission gate is active or core not initialized
        guard !isPermissionGateActive, core != nil else {
            print("[InputCoordinator] ⚠️ Append blocked - permission gate or core not ready")
            NSSound.beep()
            return
        }

        // Store frontmost app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[InputCoordinator] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Get best position for Halo
        let haloPosition = CaretPositionHelper.getBestPosition()

        // Show Halo immediately with processing state
        if Thread.isMainThread {
            haloWindowController?.show(at: haloPosition)
            haloWindowController?.updateState(.processing(providerColor: .purple, streamingText: nil))
        } else {
            DispatchQueue.main.sync { [weak self] in
                self?.haloWindowController?.show(at: haloPosition)
                self?.haloWindowController?.updateState(.processing(providerColor: .purple, streamingText: nil))
            }
        }

        // Process with append mode (AI response appends after original text)
        onProcessInput?(false)
    }

    // MARK: - Utility

    /// Clear the previous frontmost app reference
    func clearPreviousFrontmostApp() {
        previousFrontmostApp = nil
    }
}
