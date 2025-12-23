//
//  EventHandler.swift
//  Aether
//
//  Implements AetherEventHandler protocol to receive callbacks from Rust core.
//

import Foundation
import AppKit
import SwiftUI

class EventHandler: AetherEventHandler {
    // Weak reference to Halo window to avoid retain cycle
    private weak var haloWindow: HaloWindow?

    init(haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
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

        DispatchQueue.main.async { [weak self] in
            self?.handleHotkeyDetected(clipboardContent: clipboardContent)
        }
    }

    func onError(message: String) {
        print("[EventHandler] Error: \(message)")

        DispatchQueue.main.async {
            self.showErrorNotification(message: message)
        }
    }

    // MARK: - State Change Handling

    private func handleStateChange(_ state: ProcessingState) {
        switch state {
        case .idle:
            haloWindow?.hide()

        case .listening:
            haloWindow?.updateState(.listening)

        case .processing:
            haloWindow?.updateState(.processing(providerColor: .green))

        case .success:
            haloWindow?.updateState(.success)
            // Auto-hide after 2 seconds
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.haloWindow?.hide()
            }

        case .error:
            haloWindow?.updateState(.error)
            // Auto-hide after 2 seconds
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.haloWindow?.hide()
            }
        }
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
            self?.haloWindow?.updateState(.success)
        }
    }

    // MARK: - Error Notification

    private func showErrorNotification(message: String) {
        let alert = NSAlert()
        alert.messageText = "Aether Error"
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}
