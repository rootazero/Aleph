//
//  HaloWindowController.swift
//  Aether
//
//  Simplified controller for HaloWindow (no theme support).
//

import Cocoa
import SwiftUI

/// Controller for managing HaloWindow lifecycle
final class HaloWindowController {

    // MARK: - Properties

    /// The managed HaloWindow instance
    private(set) var window: HaloWindow?

    /// Event handler reference for callbacks
    private weak var eventHandler: EventHandler?

    /// Core reference for command completion
    private weak var core: AetherCore?

    // MARK: - Initialization

    /// Initialize controller (no theme engine required)
    init() {
        // Window is created lazily
    }

    /// Initialize with theme engine (deprecated, ignored)
    convenience init(themeEngine: Any?) {
        self.init()
    }

    // MARK: - Window Management

    /// Create the HaloWindow
    func createWindow() {
        window = HaloWindow()
    }

    /// Set the event handler for callbacks
    func setEventHandler(_ handler: EventHandler) {
        self.eventHandler = handler
    }

    /// Configure with AetherCore reference
    func configureCore(_ core: AetherCore) {
        self.core = core
    }

    // MARK: - State Forwarding

    /// Update Halo state
    func updateState(_ state: HaloState) {
        window?.updateState(state)
    }

    /// Show Halo at position
    func show(at position: NSPoint) {
        window?.show(at: position)
    }

    /// Show Halo centered
    func showCentered() {
        window?.showCentered()
    }

    /// Show at current tracked position
    func showAtCurrentPosition() {
        window?.showAtCurrentPosition()
    }

    /// Hide Halo
    func hide() {
        window?.hide()
    }

    /// Show below a position
    func showBelow(at position: NSPoint) {
        window?.showBelow(at: position)
    }

    /// Force hide immediately
    func forceHide() {
        window?.forceHide()
    }

    /// Show toast notification
    func showToast(type: ToastType, title: String, message: String, autoDismiss: Bool) {
        window?.updateState(.toast(type: type, title: title, message: message, autoDismiss: autoDismiss))
        window?.showToastCentered()
    }
}
