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

    // Weak reference to AetherCore for retry functionality
    private weak var core: AetherCore?

    // Accumulated text for streaming responses
    private var accumulatedText: String = ""

    // Last update time for debouncing
    private var lastUpdateTime: Date = Date()

    init(haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
    }

    // Set AetherCore reference after initialization
    func setCore(_ core: AetherCore) {
        self.core = core
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

        // Capture context when hotkey is detected
        let context = ContextCapture.captureContext()
        print("[EventHandler] Captured context: app=\(context.bundleId ?? "nil"), window=\(context.windowTitle ?? "nil")")

        // Send context to Rust core
        if let bundleId = context.bundleId {
            let capturedContext = CapturedContext(
                appBundleId: bundleId,
                windowTitle: context.windowTitle
            )
            core?.setCurrentContext(context: capturedContext)
        }

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

    // MARK: - State Change Handling

    private func handleStateChange(_ state: ProcessingState) {
        switch state {
        case .idle:
            haloWindow?.hide()
            // Reset accumulated text when going idle
            accumulatedText = ""

        case .listening:
            haloWindow?.updateState(.listening)
            // Reset accumulated text when starting new interaction
            accumulatedText = ""

        case .retrievingMemory:
            haloWindow?.updateState(.retrievingMemory)

        case .processingWithAi:
            // This state will be updated with provider details via onAiProcessingStarted callback
            haloWindow?.updateState(.processing(providerColor: .blue, streamingText: nil))

        case .processing:
            haloWindow?.updateState(.processing(providerColor: .green, streamingText: nil))

        case .success:
            // Show final accumulated text
            if !accumulatedText.isEmpty {
                haloWindow?.updateState(.success(finalText: accumulatedText))
            } else {
                haloWindow?.updateState(.success(finalText: nil))
            }
            // Auto-hide after 2 seconds
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.haloWindow?.hide()
            }

        case .error:
            // Use typed error if available, otherwise show generic error
            haloWindow?.updateState(.error(type: .unknown, message: "An error occurred"))
            // Auto-hide after 2 seconds
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.haloWindow?.hide()
            }
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
        let color = parseHexColor(providerColor) ?? .green

        // Update Halo to show AI processing with provider info
        haloWindow?.updateState(.processingWithAI(providerColor: color, providerName: providerName))
    }

    private func handleAiResponseReceived(responsePreview: String) {
        // Store the response preview
        accumulatedText = responsePreview

        // Update Halo with the response preview
        haloWindow?.updateState(.processing(providerColor: .green, streamingText: responsePreview))
    }

    /// Parse hex color string to NSColor
    private func parseHexColor(_ hex: String) -> NSColor? {
        var hexSanitized = hex.trimmingCharacters(in: .whitespacesAndNewlines)
        hexSanitized = hexSanitized.replacingOccurrences(of: "#", with: "")

        var rgb: UInt64 = 0
        guard Scanner(string: hexSanitized).scanHexInt64(&rgb) else {
            return nil
        }

        let r = CGFloat((rgb & 0xFF0000) >> 16) / 255.0
        let g = CGFloat((rgb & 0x00FF00) >> 8) / 255.0
        let b = CGFloat(rgb & 0x0000FF) / 255.0

        return NSColor(red: r, green: g, blue: b, alpha: 1.0)
    }

    // MARK: - Typed Error Handling

    private func handleTypedError(errorType: ErrorType, message: String) {
        // Update Halo with typed error (do NOT auto-hide, let user interact with error actions)
        haloWindow?.updateState(.error(type: errorType, message: message))

        // NOTE: We don't auto-hide the Halo for errors anymore because
        // ErrorActionView provides actionable buttons that the user might want to interact with.
        // The Halo will hide when the user clicks "Dismiss" or after a successful retry.
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
