//
//  OutputCoordinator.swift
//  Aether
//
//  Coordinator for managing AI response output to target applications.
//  Extracted from AppDelegate to improve separation of concerns.
//

import AppKit
import SwiftUI

// MARK: - Output Types

/// Source of text input - determines cursor positioning for output
enum TextSource {
    case selectedText      // User had text selected, Cmd+X/C captured it
    case accessibilityAPI  // No selection, read full window text via Accessibility API (text NOT deleted)
    case selectAll         // Accessibility failed, used Cmd+A then Cmd+X/C
}

/// Output session type - determines post-output behavior
enum OutputSessionType {
    case singleTurn   // Single-turn: restore clipboard after output, show success state
    case multiTurn    // Multi-turn: don't restore clipboard, post continuation notification
}

/// Unified output context for both single-turn and multi-turn modes
struct OutputContext {
    /// Whether to use replace mode (true) or append mode (false)
    let useReplaceMode: Bool

    /// Text source for cursor positioning (only used in single-turn)
    let textSource: TextSource?

    /// Session type determines post-output behavior
    let sessionType: OutputSessionType

    /// Original clipboard content for restoration (single-turn only)
    let originalClipboard: String?

    /// Turn ID for multi-turn conversations (nil for single-turn)
    let turnId: UInt32?

    /// Session ID for multi-turn conversations (nil for single-turn)
    let conversationSessionId: String?
}

// MARK: - Output Coordinator

/// Coordinator for managing AI response output
///
/// Responsibilities:
/// - Execute typewriter and instant output modes
/// - Handle cursor positioning before output
/// - Manage ESC key monitoring for typewriter cancellation
/// - Handle post-output actions (clipboard restore, success state)
final class OutputCoordinator {

    // MARK: - Dependencies

    /// Reference to core for config loading
    private weak var core: AetherCore?

    /// Reference to Halo window for state updates
    private weak var haloWindow: HaloWindow?

    /// Clipboard manager for paste operations
    private let clipboardManager: any ClipboardManagerProtocol

    // MARK: - State

    /// Typewriter cancellation token
    private var typewriterCancellation: CancellationToken?

    /// ESC key monitor for cancelling typewriter
    private var escapeKeyMonitor: Any?

    /// Reference to previous frontmost app (set by input coordinator)
    var previousFrontmostApp: NSRunningApplication?

    // MARK: - Initialization

    /// Initialize the output coordinator
    ///
    /// - Parameters:
    ///   - clipboardManager: Clipboard manager for paste operations
    init(
        clipboardManager: any ClipboardManagerProtocol = DependencyContainer.shared.clipboardManager
    ) {
        self.clipboardManager = clipboardManager
    }

    /// Configure dependencies after initialization
    ///
    /// - Parameters:
    ///   - core: AetherCore instance
    ///   - haloWindow: HaloWindow for state updates
    func configure(core: AetherCore?, haloWindow: HaloWindow?) {
        self.core = core
        self.haloWindow = haloWindow
    }

    // MARK: - Lifecycle

    /// Start ESC key monitoring
    func start() {
        setupEscapeKeyMonitor()
    }

    /// Stop ESC key monitoring and cleanup
    func stop() {
        removeEscapeKeyMonitor()
        typewriterCancellation?.cancel()
        typewriterCancellation = nil
    }

    // MARK: - Unified Output Pipeline

    /// Unified output function for both single-turn and multi-turn modes
    ///
    /// This function consolidates all output logic:
    /// 1. Load output config (outputMode, typingSpeed)
    /// 2. Reactivate target app
    /// 3. Prepare cursor position (single-turn only, based on textSource)
    /// 4. Add newline for append mode
    /// 5. Execute output (typewriter or instant)
    /// 6. Post-output actions (based on sessionType)
    ///
    /// - Parameters:
    ///   - response: The AI response text to output
    ///   - context: Output context containing mode and session configuration
    func performOutput(response: String, context: OutputContext) {
        guard let core = core else {
            print("[OutputCoordinator] ⚠️ Core not available for output")
            return
        }

        // Truncate response if needed
        let maxResponseLength = 5000
        let truncatedResponse: String
        if response.count > maxResponseLength {
            print("[OutputCoordinator] ⚠️ Response too long (\(response.count) chars), truncating to \(maxResponseLength)")
            truncatedResponse = String(response.prefix(maxResponseLength)) + "\n\n[... response truncated due to length limit ...]"
        } else {
            truncatedResponse = response
        }

        // Load output config
        // Load output config from behavior settings
        var outputMode = "instant"
        var typingSpeed: Int = 50
        do {
            let config = try core.loadConfig()
            if let behavior = config.behavior {
                outputMode = behavior.outputMode
                typingSpeed = Int(behavior.typingSpeed)
            }
            print("[OutputCoordinator] 📋 Output config: mode=\(outputMode), speed=\(typingSpeed) chars/sec")
        } catch {
            print("[OutputCoordinator] ⚠️ Failed to load config, using defaults: \(error)")
        }

        // Determine append mode based on context
        let useAppendMode: Bool
        if let turnId = context.turnId {
            // Multi-turn: first turn uses trigger mode, subsequent turns always append
            if turnId == 0 {
                useAppendMode = !context.useReplaceMode
                print("[OutputCoordinator] 🎯 Multi-turn first turn: useReplaceMode=\(context.useReplaceMode), useAppendMode=\(useAppendMode)")
            } else {
                useAppendMode = true
                print("[OutputCoordinator] 🎯 Multi-turn subsequent turn: always append mode")
            }
        } else {
            // Single-turn: directly use the mode from context
            useAppendMode = !context.useReplaceMode
            print("[OutputCoordinator] 🎯 Single-turn: useReplaceMode=\(context.useReplaceMode), useAppendMode=\(useAppendMode)")
        }

        DispatchQueue.mainAsync(weakRef: self) { slf in
            print("[OutputCoordinator] 🎯 Starting unified output phase...")

            // Reactivate target app
            if let previousApp = slf.previousFrontmostApp,
               previousApp.bundleIdentifier != Bundle.main.bundleIdentifier {
                print("[OutputCoordinator] 🔄 Reactivating target app: \(previousApp.localizedName ?? "Unknown")")
                previousApp.activate(options: [])
                Thread.sleep(forTimeInterval: 0.15)
            }

            // Prepare cursor position
            // - Single-turn: always prepare
            // - Multi-turn first turn (turnId == 0): prepare for initial output (to handle replace mode correctly)
            // - Multi-turn subsequent turns: skip (cursor already at correct position after previous output)
            let shouldPreparePosition = context.sessionType == .singleTurn ||
                                        (context.sessionType == .multiTurn && context.turnId == 0)

            if shouldPreparePosition, let textSource = context.textSource {
                slf.prepareOutputPosition(textSource: textSource, useCutMode: context.useReplaceMode)
                Thread.sleep(forTimeInterval: 0.05)
            }

            // Add double newline for append mode (visual separation between original and response)
            if useAppendMode {
                print("[OutputCoordinator] ⏎ Adding double newline before response (append mode)")
                // Use typeTextInstant which calls typeSpecialKey with privateState
                // This properly isolates modifier key state and clears flags
                // More reliable than simulateKeyPress across different apps (e.g., Notes)
                KeyboardSimulator.shared.typeTextInstant("\n\n")
                Thread.sleep(forTimeInterval: 0.05)
            } else {
                print("[OutputCoordinator] ✂️ No newline - replacing original text")
            }

            // Execute output
            if outputMode == "typewriter" {
                slf.executeTypewriterOutput(
                    text: truncatedResponse,
                    speed: typingSpeed,
                    context: context
                )
            } else {
                slf.executeInstantOutput(
                    text: truncatedResponse,
                    context: context
                )
            }
        }
    }

    // MARK: - Output Execution

    /// Execute typewriter output with proper post-output handling
    private func executeTypewriterOutput(text: String, speed: Int, context: OutputContext) {
        print("[OutputCoordinator] ⌨️ Using typewriter mode at \(speed) chars/sec")

        typewriterCancellation = CancellationToken()
        haloWindow?.hide()

        Task {
            let typedCount = await KeyboardSimulator.shared.typeText(
                text,
                speed: speed,
                cancellationToken: typewriterCancellation
            )
            print("[OutputCoordinator] ⌨️ Typed \(typedCount)/\(text.count) characters")

            typewriterCancellation = nil

            await MainActor.run {
                self.handlePostOutput(context: context, responsePreview: String(text.prefix(100)))
            }
        }
    }

    /// Execute instant (paste) output with proper post-output handling
    private func executeInstantOutput(text: String, context: OutputContext) {
        print("[OutputCoordinator] 📋 Using instant mode (paste)")

        clipboardManager.setText(text)
        Thread.sleep(forTimeInterval: 0.05)

        print("[OutputCoordinator] 📋 Simulating Cmd+V to paste response")
        let pasteSuccess = KeyboardSimulator.shared.simulatePaste()
        print("[OutputCoordinator] 📋 Paste result: \(pasteSuccess ? "success" : "failed")")

        // Small delay for paste completion
        Thread.sleep(forTimeInterval: 0.3)

        handlePostOutput(context: context, responsePreview: String(text.prefix(100)))
    }

    /// Handle post-output actions based on session type
    private func handlePostOutput(context: OutputContext, responsePreview: String) {
        switch context.sessionType {
        case .singleTurn:
            // Restore clipboard after delay
            DispatchQueue.mainAsyncAfter(delay: 0.5, weakRef: self) { slf in
                if let original = context.originalClipboard {
                    slf.clipboardManager.setText(original)
                    print("[OutputCoordinator] ♻️ Restored original clipboard content")
                } else {
                    slf.clipboardManager.clear()
                    print("[OutputCoordinator] ♻️ Cleared clipboard (original was empty)")
                }
            }

            // Output complete - hide Halo immediately (success state removed)
            print("[OutputCoordinator] ✅ Output complete, hiding Halo")
            haloWindow?.hide()

        case .multiTurn:
            // Post continuation notification
            if let sessionId = context.conversationSessionId {
                print("[OutputCoordinator] 🎯 Triggering conversation input display")
                NotificationCenter.default.post(
                    name: .conversationContinuationReady,
                    object: sessionId
                )
            }
        }
    }

    // MARK: - Output Preparation

    /// Prepare cursor position before outputting AI response
    ///
    /// This method ensures the cursor is in the correct position based on:
    /// - Text source: Where the input text came from
    /// - Input mode: Whether user wants to replace or append
    ///
    /// | Text Source      | Replace Mode      | Append Mode             |
    /// |------------------|-------------------|-------------------------|
    /// | selectedText     | No action needed  | Move to selection end   |
    /// | accessibilityAPI | Cmd+A to select   | Cmd+Down to move to end |
    /// | selectAll        | No action needed  | Cmd+Down to move to end |
    private func prepareOutputPosition(textSource: TextSource, useCutMode: Bool) {
        print("[OutputCoordinator] 🎯 Preparing output position: source=\(textSource), replace=\(useCutMode)")

        switch (textSource, useCutMode) {
        case (.selectedText, true):
            // Replace selected text: Selection is still active after input phase (Cmd+C only)
            // We don't need to do anything - paste/type will automatically replace the selection
            // This provides better UX: text stays visible during AI thinking, then gets replaced on output
            print("[OutputCoordinator] ➡️ selectedText + replace: Selection active, will be replaced by paste/type")

        case (.selectedText, false):
            // Append after selected text: Move cursor to end of selection
            // After Cmd+C, the selection is still active, pressing Right arrow moves to end
            print("[OutputCoordinator] ➡️ selectedText + append: Moving to end of selection")
            KeyboardSimulator.shared.simulateKeyPress(.rightArrow)
            Thread.sleep(forTimeInterval: 0.05)

        case (.accessibilityAPI, true):
            // Replace full window text: Need to select all first, then typing will replace
            // Because Accessibility API only read the text, didn't delete it
            print("[OutputCoordinator] ➡️ accessibilityAPI + replace: Selecting all to replace")
            KeyboardSimulator.shared.simulateSelectAll()
            Thread.sleep(forTimeInterval: 0.05)

        case (.accessibilityAPI, false):
            // Append to full window text: Move cursor to end of document
            print("[OutputCoordinator] ➡️ accessibilityAPI + append: Moving to end of document")
            KeyboardSimulator.shared.simulateMoveToEnd()
            Thread.sleep(forTimeInterval: 0.05)

        case (.selectAll, true):
            // Replace full window text: Need to select all first, then typing will replace
            // Selection from input phase may have been lost during AI processing
            // So we re-select all content before output
            print("[OutputCoordinator] ➡️ selectAll + replace: Selecting all to replace")
            KeyboardSimulator.shared.simulateSelectAll()
            Thread.sleep(forTimeInterval: 0.05)

        case (.selectAll, false):
            // Append after Cmd+A + Cmd+C: Move cursor to end of document
            print("[OutputCoordinator] ➡️ selectAll + append: Moving to end of document")
            KeyboardSimulator.shared.simulateMoveToEnd()
            Thread.sleep(forTimeInterval: 0.05)
        }
    }

    // MARK: - ESC Key Monitoring

    /// Setup global ESC key monitor to cancel typewriter animation
    private func setupEscapeKeyMonitor() {
        escapeKeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            // Check if ESC key was pressed (keyCode 53)
            if event.keyCode == 53 {
                self?.handleEscapeKey()
            }
        }
        print("[OutputCoordinator] ESC key monitor installed for typewriter cancellation")
    }

    /// Remove ESC key monitor
    private func removeEscapeKeyMonitor() {
        if let monitor = escapeKeyMonitor {
            NSEvent.removeMonitor(monitor)
            escapeKeyMonitor = nil
            print("[OutputCoordinator] ESC key monitor removed")
        }
    }

    /// Handle ESC key press - cancel typewriter animation
    private func handleEscapeKey() {
        guard let cancellation = typewriterCancellation else {
            // Note: Multi-turn input ESC handling is managed by MultiTurnCoordinator
            print("[OutputCoordinator] ESC pressed but no typewriter is running")
            return
        }

        print("[OutputCoordinator] ESC pressed - cancelling typewriter animation")
        cancellation.cancel()

        // Clear the cancellation token immediately
        typewriterCancellation = nil

        // Hide Halo immediately (success state removed)
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
        }
    }
}
