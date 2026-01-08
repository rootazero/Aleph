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
/// - Process input and route to AI providers
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

    /// Reference to output coordinator for response output
    private weak var outputCoordinator: OutputCoordinator?

    /// Reference to conversation coordinator for multi-turn conversations
    private weak var conversationCoordinator: ConversationCoordinator?

    /// Clipboard manager for clipboard operations
    private let clipboardManager: any ClipboardManagerProtocol

    /// Clipboard monitor for context tracking
    private let clipboardMonitor: any ClipboardMonitorProtocol

    // MARK: - State

    /// Store the frontmost app when hotkey is pressed
    private(set) var previousFrontmostApp: NSRunningApplication?

    /// Whether permission gate is active (blocks input)
    var isPermissionGateActive: Bool = false

    // MARK: - Initialization

    /// Initialize the input coordinator
    ///
    /// - Parameters:
    ///   - clipboardManager: Clipboard manager for operations
    ///   - clipboardMonitor: Clipboard monitor for context tracking
    init(
        clipboardManager: any ClipboardManagerProtocol = DependencyContainer.shared.clipboardManager,
        clipboardMonitor: any ClipboardMonitorProtocol = DependencyContainer.shared.clipboardMonitor
    ) {
        self.clipboardManager = clipboardManager
        self.clipboardMonitor = clipboardMonitor
    }

    /// Configure dependencies after initialization
    ///
    /// - Parameters:
    ///   - core: AetherCore instance
    ///   - haloWindowController: HaloWindowController for state updates
    ///   - eventHandler: EventHandler for error callbacks
    ///   - outputCoordinator: OutputCoordinator for response output
    ///   - conversationCoordinator: ConversationCoordinator for multi-turn conversations
    func configure(
        core: AetherCore,
        haloWindowController: HaloWindowController?,
        eventHandler: EventHandler?,
        outputCoordinator: OutputCoordinator? = nil,
        conversationCoordinator: ConversationCoordinator? = nil
    ) {
        self.core = core
        self.haloWindowController = haloWindowController
        self.eventHandler = eventHandler
        self.outputCoordinator = outputCoordinator
        self.conversationCoordinator = conversationCoordinator
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
        processWithInputMode(useCutMode: true)
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
        processWithInputMode(useCutMode: false)
    }

    // MARK: - Input Processing

    /// Process input with specified mode (cut = replace original, copy = append to original)
    ///
    /// - Parameter useCutMode: If true, AI response replaces original text. If false, appends after.
    private func processWithInputMode(useCutMode: Bool) {
        print("[InputCoordinator] Processing with cut mode: \(useCutMode)")

        guard core != nil else {
            print("[InputCoordinator] ⚠️ Core not initialized")
            // Show error in Halo
            DispatchQueue.main.async { [weak self] in
                self?.haloWindowController?.updateState(.error(
                    type: .unknown,
                    message: L("error.core_not_initialized"),
                    suggestion: L("error.core_not_initialized.suggestion")
                ))
            }
            return
        }

        // CRITICAL: Reactivate the previous frontmost app for keyboard events
        // This is essential when coming from Halo input mode selection
        if let previousApp = previousFrontmostApp,
           previousApp.bundleIdentifier != Bundle.main.bundleIdentifier {
            print("[InputCoordinator] 🔄 Reactivating previous app: \(previousApp.localizedName ?? "Unknown")")
            previousApp.activate(options: [])
            Thread.sleep(forTimeInterval: 0.15)  // Give time for activation
        }

        print("[InputCoordinator] 📋 Using cut mode: \(useCutMode)")

        // Track where the text came from - this determines output strategy
        var textSource: TextSource = .selectedText

        // CRITICAL: Save original clipboard content to restore later
        // This protects user's pre-existing clipboard data
        let originalClipboardText = clipboardManager.getText()
        let originalChangeCount = clipboardManager.changeCount()
        print("[InputCoordinator] 💾 Saved original clipboard state (changeCount: \(originalChangeCount))")

        // CRITICAL: Save original clipboard media attachments BEFORE Cut/Copy
        // This preserves images/files that user manually copied to clipboard
        // Without this, simulateCut()/simulateCopy() would overwrite the clipboard
        // and lose the media attachments that user intended to send to AI
        let (_, originalMediaAttachments, _) = clipboardManager.getMixedContent()
        if !originalMediaAttachments.isEmpty {
            print("[InputCoordinator] 📎 Saved \(originalMediaAttachments.count) original media attachment(s) from clipboard")
            for (index, attachment) in originalMediaAttachments.enumerated() {
                print("[InputCoordinator]   [\(index + 1)] \(attachment.mediaType)/\(attachment.mimeType) - \(attachment.sizeBytes) bytes")
            }
        }

        // Step 1: Try to cut/copy selected text based on input_mode
        if useCutMode {
            print("[InputCoordinator] Simulating Cmd+X to cut selected text...")
            KeyboardSimulator.shared.simulateCut()
        } else {
            print("[InputCoordinator] Simulating Cmd+C to copy selected text...")
            KeyboardSimulator.shared.simulateCopy()
        }

        // Wait for clipboard to update (macOS needs a small delay)
        Thread.sleep(forTimeInterval: 0.1)  // 100ms delay

        // Check if clipboard changed (means there was selected text)
        let afterCopyChangeCount = clipboardManager.changeCount()
        let hasSelectedText = (afterCopyChangeCount != originalChangeCount)

        if !hasSelectedText {
            // Step 2: No selected text detected
            // Try elegant Accessibility API first (silent, no visible selection)
            print("[InputCoordinator] ⚠️ No selected text detected, trying Accessibility API to read window text...")

            let accessibilityResult = AccessibilityTextReader.shared.readFocusedText()

            switch accessibilityResult {
            case .success(let text):
                // Successfully read text via Accessibility API!
                // IMPORTANT: Text is NOT deleted from window, just read
                print("[InputCoordinator] ✅ Read text via Accessibility API (\(text.count) chars) - completely silent!")
                textSource = .accessibilityAPI  // Mark source as Accessibility API
                // Temporarily set the text to clipboard for processing
                clipboardManager.setText(text)

            case .noTextContent, .noFocusedElement, .unsupported:
                // Accessibility API couldn't get text, fallback to Cmd+A
                print("[InputCoordinator] ⚠️ Accessibility API failed, falling back to Cmd+A method...")
                textSource = .selectAll  // Mark source as select all
                KeyboardSimulator.shared.simulateSelectAll()
                Thread.sleep(forTimeInterval: 0.05)  // 50ms delay

                // Cut/Copy again after selecting all (based on input_mode)
                if useCutMode {
                    KeyboardSimulator.shared.simulateCut()
                } else {
                    KeyboardSimulator.shared.simulateCopy()
                }
                Thread.sleep(forTimeInterval: 0.1)  // 100ms delay

                let afterSelectAllChangeCount = clipboardManager.changeCount()
                if afterSelectAllChangeCount == afterCopyChangeCount {
                    print("[InputCoordinator] ❌ No text content found even after Cmd+A")
                    // Restore original clipboard
                    if let original = originalClipboardText {
                        clipboardManager.setText(original)
                    }

                    // Show error
                    let errorPosition = CaretPositionHelper.getBestPosition()
                    DispatchQueue.main.async { [weak self] in
                        self?.haloWindowController?.show(at: errorPosition)
                        self?.haloWindowController?.updateState(.error(
                            type: .unknown,
                            message: L("error.no_text_in_window"),
                            suggestion: L("error.no_text_in_window.suggestion")
                        ))
                        // Auto-hide after 2 seconds
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                            self?.haloWindowController?.hide()
                        }
                    }
                    return
                } else {
                    print("[InputCoordinator] ✓ Selected all text in current window (via Cmd+A)")
                }

            case .accessibilityDenied:
                // This shouldn't happen as we check permissions at startup
                print("[InputCoordinator] ❌ Accessibility permission denied, using Cmd+A fallback")
                textSource = .selectAll
                KeyboardSimulator.shared.simulateSelectAll()
                Thread.sleep(forTimeInterval: 0.05)
                if useCutMode {
                    KeyboardSimulator.shared.simulateCut()
                } else {
                    KeyboardSimulator.shared.simulateCopy()
                }
                Thread.sleep(forTimeInterval: 0.1)

            case .error(let message):
                print("[InputCoordinator] ❌ Accessibility error: \(message), using Cmd+A fallback")
                textSource = .selectAll
                KeyboardSimulator.shared.simulateSelectAll()
                Thread.sleep(forTimeInterval: 0.05)
                if useCutMode {
                    KeyboardSimulator.shared.simulateCut()
                } else {
                    KeyboardSimulator.shared.simulateCopy()
                }
                Thread.sleep(forTimeInterval: 0.1)
            }
        } else {
            print("[InputCoordinator] ✓ Detected selected text")
            textSource = .selectedText
        }

        print("[InputCoordinator] 📍 Text source: \(textSource), Input mode: \(useCutMode ? "replace" : "append")")

        // Get the captured clipboard content (text + media attachments)
        let (extractedText, mediaAttachments, extractionError) = clipboardManager.getMixedContent()

        // Check for extraction errors (e.g., file too large)
        if let error = extractionError {
            print("[InputCoordinator] ❌ Content extraction error: \(error)")
            // Restore original clipboard
            if let original = originalClipboardText {
                clipboardManager.setText(original)
            }
            // Hide Halo and show error toast to user
            DispatchQueue.main.async { [weak self] in
                self?.haloWindowController?.hide()
                self?.eventHandler?.showToast(
                    type: .warning,
                    title: L("error.file_size"),
                    message: error,
                    autoDismiss: false
                )
            }
            return
        }

        guard let clipboardText = extractedText else {
            print("[InputCoordinator] ❌ Clipboard is empty after copy operation")
            // Restore original clipboard
            if let original = originalClipboardText {
                clipboardManager.setText(original)
            }
            return
        }

        print("[InputCoordinator] Clipboard text: \(clipboardText.prefix(50))...")

        // Log media attachments if present
        if !mediaAttachments.isEmpty {
            print("[InputCoordinator] 📎 Extracted \(mediaAttachments.count) media attachment(s) from current clipboard:")
            for (index, attachment) in mediaAttachments.enumerated() {
                print("[InputCoordinator]   [\(index + 1)] \(attachment.mediaType)/\(attachment.mimeType) - \(attachment.sizeBytes) bytes")
            }
        }

        // CRITICAL FIX: Merge attachments in correct order
        // Data order rule: Window text + Clipboard text/attachment + Window attachment
        var finalMediaAttachments: [MediaAttachment] = []

        // 1. Add clipboard attachments first (user's copied context)
        if !originalMediaAttachments.isEmpty {
            finalMediaAttachments.append(contentsOf: originalMediaAttachments)
            print("[InputCoordinator] 📎 Added \(originalMediaAttachments.count) clipboard attachment(s)")
        }

        // 2. Add window attachments (from Cut/Copy of window content)
        if !mediaAttachments.isEmpty {
            finalMediaAttachments.append(contentsOf: mediaAttachments)
            print("[InputCoordinator] 📎 Added \(mediaAttachments.count) window attachment(s)")
        }

        if !finalMediaAttachments.isEmpty {
            print("[InputCoordinator] 📎 Total: \(finalMediaAttachments.count) attachment(s)")
        }

        // IMPORTANT: Check for recent clipboard content (within 10 seconds)
        // This allows us to use previous clipboard as additional context
        let recentClipboardContent = clipboardMonitor.getRecentClipboardContent()
        let clipboardContext: String? = {
            guard let recentContent = recentClipboardContent,
                  !recentContent.isEmpty,
                  recentContent != clipboardText else {
                return nil
            }
            return recentContent
        }()

        if let context = clipboardContext {
            print("[InputCoordinator] 📋 Found clipboard context (\(context.count) chars, within 10s)")
        } else {
            print("[InputCoordinator] No clipboard context to use")
        }

        // Capture current window context
        let windowContext = ContextCapture.captureContext()
        print("[InputCoordinator] Context: app=\(windowContext.bundleId ?? "unknown"), window=\(windowContext.windowTitle ?? "nil")")

        // Update Halo to processing state
        DispatchQueue.main.async { [weak self] in
            self?.haloWindowController?.updateState(.processing(providerColor: .purple, streamingText: nil))
        }

        // Process input asynchronously to avoid blocking UI
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }

            do {
                // Create captured context for Rust
                let capturedContext = CapturedContext(
                    appBundleId: windowContext.bundleId ?? "unknown",
                    windowTitle: windowContext.windowTitle,
                    attachments: finalMediaAttachments.isEmpty ? nil : finalMediaAttachments
                )

                // Construct user input - clipboard content appended after window content
                let userInput: String
                if let clipContext = clipboardContext {
                    userInput = "\(clipboardText)\n\n\(clipContext)"
                    print("[InputCoordinator] 🤖 Sending to AI: window (\(clipboardText.count) chars) + clipboard (\(clipContext.count) chars)")
                } else {
                    userInput = clipboardText
                    print("[InputCoordinator] 🤖 Sending to AI: window text only (\(clipboardText.count) chars)")
                }

                // Check if should use multi-turn conversation mode
                let trimmedInput = userInput.trimmingCharacters(in: .whitespacesAndNewlines)
                let hasChatCommand = trimmedInput.hasPrefix("/chat")

                let shouldUseMultiTurn: Bool
                if hasChatCommand {
                    shouldUseMultiTurn = true
                } else {
                    // Check config for default multi-turn setting
                    if let core = self.core {
                        do {
                            let config = try core.loadConfig()
                            shouldUseMultiTurn = config.behavior?.multiTurnEnabled ?? false
                        } catch {
                            shouldUseMultiTurn = false
                        }
                    } else {
                        shouldUseMultiTurn = false
                    }
                }

                if shouldUseMultiTurn {
                    // Extract the actual message (remove /chat prefix if present)
                    let conversationInput: String
                    if hasChatCommand {
                        let chatMessage = String(trimmedInput.dropFirst(5)).trimmingCharacters(in: .whitespacesAndNewlines)
                        conversationInput = chatMessage.isEmpty ? "Hello" : chatMessage
                        print("[InputCoordinator] 🎭 /chat command detected - starting multi-turn conversation")
                    } else {
                        conversationInput = userInput
                        print("[InputCoordinator] 🎭 Multi-turn enabled by default - starting conversation")
                    }
                    print("[InputCoordinator] 🎭 Conversation input: \(conversationInput.prefix(50))...")

                    // Store conversation context for output handling via ConversationCoordinator
                    self.conversationCoordinator?.storeConversationContext(
                        textSource: textSource,
                        useCutMode: useCutMode,
                        originalClipboard: originalClipboardText
                    )
                    self.conversationCoordinator?.previousFrontmostApp = self.previousFrontmostApp
                    print("[InputCoordinator] 🎭 Stored context in ConversationCoordinator")

                    // Start conversation (callbacks handle output and continuation UI)
                    self.conversationCoordinator?.startConversation(userInput: conversationInput, context: capturedContext)

                    // Return early - conversation flow is handled via callbacks
                    return
                }

                // Call Rust core's process_input()
                guard let core = self.core else {
                    print("[InputCoordinator] ERROR: Core became nil during processing")
                    return
                }
                let response = try core.processInput(
                    userInput: userInput,
                    context: capturedContext
                )

                print("[InputCoordinator] Received AI response (\(response.count) chars)")

                // Use unified output pipeline via OutputCoordinator
                let outputContext = OutputContext(
                    useReplaceMode: useCutMode,
                    textSource: textSource,
                    sessionType: .singleTurn,
                    originalClipboard: originalClipboardText,
                    turnId: nil,
                    conversationSessionId: nil
                )
                self.outputCoordinator?.previousFrontmostApp = self.previousFrontmostApp
                self.outputCoordinator?.performOutput(response: response, context: outputContext)
            } catch {
                print("[InputCoordinator] ❌ Error processing input: \(error)")

                // CRITICAL: Clear clipboard monitor history to prevent error messages from being used as context
                self.clipboardMonitor.clearHistory()
                print("[InputCoordinator] 🗑️ Cleared clipboard monitor history after error")

                // CRITICAL: Restore original clipboard on error
                DispatchQueue.main.async {
                    if let original = originalClipboardText {
                        self.clipboardManager.setText(original)
                        print("[InputCoordinator] ♻️ Restored original clipboard content (after AI error)")
                    }
                }

                // For AetherException, the error details have already been sent via callback
                if error is AetherException {
                    print("[InputCoordinator] AetherException caught - error details already sent via callback")
                } else {
                    // For non-Rust errors
                    let errorMessage = error.localizedDescription
                    let nsError = error as NSError
                    let suggestion = nsError.userInfo["suggestion"] as? String

                    DispatchQueue.main.async { [weak self] in
                        self?.eventHandler?.onError(
                            message: errorMessage,
                            suggestion: suggestion ?? L("error.check_connection")
                        )
                    }
                }
            }
        }
    }

    // MARK: - Utility

    /// Clear the previous frontmost app reference
    func clearPreviousFrontmostApp() {
        previousFrontmostApp = nil
    }
}
