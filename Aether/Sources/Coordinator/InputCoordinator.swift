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

    /// Reference to V2 core for processing (rig-core based)
    private weak var coreV2: AetherV2Core?

    /// Reference to Halo window for state updates
    private weak var haloWindow: HaloWindow?

    /// Reference to V2 event handler for callbacks
    private weak var eventHandlerV2: EventHandlerV2?

    /// Reference to output coordinator for response output
    private weak var outputCoordinator: OutputCoordinator?

    /// Clipboard manager for clipboard operations
    private let clipboardManager: any ClipboardManagerProtocol

    /// Clipboard monitor for context tracking
    private let clipboardMonitor: any ClipboardMonitorProtocol

    // MARK: - State

    /// Store the frontmost app when hotkey is pressed
    private(set) var previousFrontmostApp: NSRunningApplication?

    /// Whether permission gate is active (blocks input)
    var isPermissionGateActive: Bool = false

    /// Pending output context for V2 async callbacks
    private var pendingV2OutputContext: OutputContext?

    /// Original clipboard text for v2 restoration on error
    private var pendingV2OriginalClipboard: String?

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
    ///   - coreV2: AetherV2Core instance
    ///   - haloWindow: HaloWindow for state updates
    ///   - eventHandlerV2: EventHandlerV2 for callbacks
    ///   - outputCoordinator: OutputCoordinator for response output
    func configure(
        coreV2: AetherV2Core?,
        haloWindow: HaloWindow?,
        eventHandlerV2: EventHandlerV2?,
        outputCoordinator: OutputCoordinator? = nil
    ) {
        self.coreV2 = coreV2
        self.haloWindow = haloWindow
        self.eventHandlerV2 = eventHandlerV2
        self.outputCoordinator = outputCoordinator

        if coreV2 != nil && eventHandlerV2 != nil {
            print("[InputCoordinator] V2 interface configured and enabled")
        }
    }

    // MARK: - Trigger Handlers

    /// Handle Replace trigger (double-tap replace hotkey, default: left Shift)
    ///
    /// AI response replaces the original selected text.
    func handleReplaceTriggered() {
        print("[InputCoordinator] 🔄 Replace triggered")

        // Block if permission gate is active or core not initialized
        guard !isPermissionGateActive, coreV2 != nil else {
            print("[InputCoordinator] ⚠️ Replace blocked - permission gate or coreV2 not ready")
            NSSound.beep()
            return
        }

        // Store frontmost app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[InputCoordinator] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Show processing indicator at cursor/mouse position
        showProcessingIndicator()

        // Process with replace mode (AI response replaces original text)
        processWithInputMode(useCutMode: true)
    }

    /// Handle Append trigger (double-tap append hotkey, default: right Shift)
    ///
    /// AI response appends after the original selected text.
    func handleAppendTriggered() {
        print("[InputCoordinator] ➕ Append triggered")

        // Block if permission gate is active or core not initialized
        guard !isPermissionGateActive, coreV2 != nil else {
            print("[InputCoordinator] ⚠️ Append blocked - permission gate or coreV2 not ready")
            NSSound.beep()
            return
        }

        // Store frontmost app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[InputCoordinator] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Show processing indicator at cursor/mouse position
        showProcessingIndicator()

        // Process with append mode (AI response appends after original text)
        processWithInputMode(useCutMode: false)
    }

    // MARK: - Input Processing

    /// Process input with specified mode (cut = replace original, copy = append to original)
    ///
    /// - Parameter useCutMode: If true, AI response replaces original text. If false, appends after.
    private func processWithInputMode(useCutMode: Bool) {
        print("[InputCoordinator] Processing with cut mode: \(useCutMode)")

        guard coreV2 != nil else {
            print("[InputCoordinator] ⚠️ CoreV2 not initialized")
            // Show error in Halo
            DispatchQueue.mainAsync(weakRef: self) { slf in
                slf.haloWindow?.updateState(.error(
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

        // CRITICAL: Get recent clipboard content BEFORE Cut/Copy operation
        // This must happen before Cut/Copy because:
        // 1. Cut/Copy will change the clipboard
        // 2. ClipboardMonitor's timer might update timestamp after Cut/Copy
        // 3. We need to capture the "recent" content before it gets overwritten
        let recentClipboardContentBeforeCut = clipboardMonitor.getRecentClipboardContent()
        if let recent = recentClipboardContentBeforeCut {
            print("[InputCoordinator] 📋 Captured recent clipboard content BEFORE cut (\(recent.count) chars)")
        }

        // CRITICAL: Save original clipboard media attachments BEFORE Cut/Copy
        // This preserves images/files that user manually copied to clipboard
        // Without this, simulateCut()/simulateCopy() would overwrite the clipboard
        // and lose the media attachments that user intended to send to AI
        //
        // Also check if clipboard is recent (within 10 seconds) - only include
        // attachments if they were copied recently to avoid unintentional inclusion
        let isClipboardRecent = clipboardMonitor.isClipboardRecent()
        let originalMediaAttachments: [MediaAttachment]
        if isClipboardRecent {
            let (_, attachments, _) = clipboardManager.getMixedContent()
            originalMediaAttachments = attachments
            if !originalMediaAttachments.isEmpty {
                print("[InputCoordinator] 📎 Saved \(originalMediaAttachments.count) recent media attachment(s) from clipboard (within 10s)")
                for (index, attachment) in originalMediaAttachments.enumerated() {
                    print("[InputCoordinator]   [\(index + 1)] \(attachment.mediaType)/\(attachment.mimeType) - \(attachment.sizeBytes) bytes")
                }
            }
        } else {
            originalMediaAttachments = []
            print("[InputCoordinator] 📎 Skipping clipboard attachments (clipboard too old)")
        }

        // Step 1: Always COPY selected text first (not cut)
        // This provides better UX: original text stays visible during AI thinking.
        // The actual replacement happens at output time - paste/type will replace the selection.
        // For append mode (useCutMode=false), this is the expected behavior anyway.
        // For replace mode (useCutMode=true), the selection remains active and will be replaced on output.
        print("[InputCoordinator] Simulating Cmd+C to copy selected text...")
        KeyboardSimulator.shared.simulateCopy()

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
                textSource = .selectAll
                performSelectAllCopyFallback()

                let afterSelectAllChangeCount = clipboardManager.changeCount()
                if afterSelectAllChangeCount == afterCopyChangeCount {
                    print("[InputCoordinator] ❌ No text content found even after Cmd+A")
                    // Restore original clipboard
                    if let original = originalClipboardText {
                        clipboardManager.setText(original)
                    }

                    // Show error
                    let errorPosition = CaretPositionHelper.getBestPosition()
                    DispatchQueue.mainAsync(weakRef: self) { slf in
                        slf.haloWindow?.show(at: errorPosition)
                        slf.haloWindow?.updateState(.error(
                            type: .unknown,
                            message: L("error.no_text_in_window"),
                            suggestion: L("error.no_text_in_window.suggestion")
                        ))
                        // Auto-hide after 2 seconds
                        DispatchQueue.mainAsyncAfter(delay: 2.0, weakRef: slf) { innerSlf in
                            innerSlf.haloWindow?.hide()
                        }
                    }
                    return
                } else {
                    print("[InputCoordinator] ✓ Selected all text in current window (via Cmd+A)")
                }

            case .accessibilityDenied:
                print("[InputCoordinator] ❌ Accessibility permission denied, using Cmd+A fallback")
                textSource = .selectAll
                performSelectAllCopyFallback()

            case .error(let message):
                print("[InputCoordinator] ❌ Accessibility error: \(message), using Cmd+A fallback")
                textSource = .selectAll
                performSelectAllCopyFallback()
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
            DispatchQueue.mainAsync(weakRef: self) { slf in
                slf.haloWindow?.hide()
                slf.eventHandlerV2?.showToast(
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

        // IMPORTANT: Use the recent clipboard content captured BEFORE Cut/Copy
        // This ensures we use the correct 10-second threshold
        // (ClipboardMonitor's timer might have updated after Cut/Copy)
        let clipboardContext: String? = {
            guard let recentContent = recentClipboardContentBeforeCut,
                  !recentContent.isEmpty,
                  recentContent != clipboardText else {
                return nil
            }
            return recentContent
        }()

        // Use AttachmentMerger for centralized attachment merging logic
        // Data order rule: Window text + Clipboard text context + Clipboard attachments + Window attachments
        let mergeContext = AttachmentMerger.MergeContext(
            clipboardAttachments: originalMediaAttachments,
            windowAttachments: mediaAttachments,
            clipboardTextContext: clipboardContext,
            windowText: clipboardText
        )
        let mergeResult = AttachmentMerger.merge(mergeContext)

        // Log merge result
        if mergeResult.clipboardAttachmentCount > 0 {
            print("[InputCoordinator] 📎 Added \(mergeResult.clipboardAttachmentCount) clipboard attachment(s)")
        }
        if mergeResult.windowAttachmentCount > 0 {
            print("[InputCoordinator] 📎 Added \(mergeResult.windowAttachmentCount) window attachment(s)")
        }
        if mergeResult.totalAttachmentCount > 0 {
            print("[InputCoordinator] 📎 Total: \(mergeResult.totalAttachmentCount) attachment(s)")
        }

        if let context = clipboardContext {
            print("[InputCoordinator] 📋 Found clipboard context (\(context.count) chars, within 10s)")
        } else {
            print("[InputCoordinator] No clipboard context to use")
        }

        // Capture current window context
        let windowContext = ContextCapture.captureContext()
        print("[InputCoordinator] Context: app=\(windowContext.bundleId ?? "unknown"), window=\(windowContext.windowTitle ?? "nil")")

        // Process input asynchronously to avoid blocking UI
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }

            do {
                // Create captured context for Rust using merged attachments
                let capturedContext = CapturedContext(
                    appBundleId: windowContext.bundleId ?? "unknown",
                    windowTitle: windowContext.windowTitle,
                    attachments: mergeResult.finalAttachments.isEmpty ? nil : mergeResult.finalAttachments,
                    topicId: nil  // Single-turn mode uses default topic
                )

                // Use merged text from AttachmentMerger
                let userInput = mergeResult.finalText
                if mergeResult.hasClipboardContext {
                    print("[InputCoordinator] 🤖 Sending to AI: window (\(clipboardText.count) chars) + clipboard context")
                } else {
                    print("[InputCoordinator] 🤖 Sending to AI: window text only (\(clipboardText.count) chars)")
                }

                // Double-tap Shift always uses single-turn mode
                // Multi-turn conversations are only triggered by Cmd+Opt+/ hotkey

                // Prepare output context
                let outputContext = OutputContext(
                    useReplaceMode: useCutMode,
                    textSource: textSource,
                    sessionType: .singleTurn,
                    originalClipboard: originalClipboardText,
                    turnId: nil,
                    conversationSessionId: nil
                )

                // V2 async processing
                guard let coreV2 = self.coreV2 else {
                    print("[InputCoordinator] ERROR: CoreV2 became nil during processing")
                    return
                }

                print("[InputCoordinator] 🚀 Using V2 interface (rig-core)")

                DispatchQueue.main.async {
                    self.pendingV2OutputContext = outputContext
                    self.pendingV2OriginalClipboard = originalClipboardText
                }

                // Create V2 process options
                let options = ProcessOptionsV2(
                    appContext: capturedContext.appBundleId,
                    windowTitle: capturedContext.windowTitle,
                    stream: true
                )

                // Call V2 async process - response comes via EventHandlerV2 callbacks
                try coreV2.process(input: userInput, options: options)
                print("[InputCoordinator] V2 process initiated, awaiting callbacks")

                // Note: Output will be triggered by handleV2Completion()
                // which is called from EventHandlerV2.onComplete()
            } catch {
                print("[InputCoordinator] ❌ Error processing input: \(error)")

                // Hide processing indicator
                self.hideProcessingIndicator()

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

                    DispatchQueue.mainAsync(weakRef: self) { slf in
                        // Combine error message and suggestion for V2 onError (single message parameter)
                        let fullMessage = suggestion != nil
                            ? "\(errorMessage)\n\(suggestion!)"
                            : "\(errorMessage)\n\(L("error.check_connection"))"
                        slf.eventHandlerV2?.onError(message: fullMessage)
                    }
                }
            }
        }
    }

    // MARK: - Processing Indicator

    /// Show processing indicator at cursor position (falls back to mouse position)
    private func showProcessingIndicator() {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            // Try cursor position first, fall back to mouse position
            let position = CaretPositionHelper.getBestPosition()
            self.haloWindow?.updateState(.processing(streamingText: nil))
            self.haloWindow?.show(at: position)
        }
    }

    /// Hide processing indicator
    private func hideProcessingIndicator() {
        DispatchQueue.main.async { [weak self] in
            self?.haloWindow?.hide()
        }
    }

    // MARK: - Utility

    /// Clear the previous frontmost app reference
    func clearPreviousFrontmostApp() {
        previousFrontmostApp = nil
    }

    // MARK: - Private Helpers

    /// Perform select-all and copy fallback when accessibility API fails
    ///
    /// This method encapsulates the common keyboard simulation pattern used
    /// when we can't read text directly via Accessibility API.
    private func performSelectAllCopyFallback() {
        KeyboardSimulator.shared.simulateSelectAll()
        Thread.sleep(forTimeInterval: 0.05)
        print("[InputCoordinator] Simulating Cmd+C to copy all text...")
        KeyboardSimulator.shared.simulateCopy()
        Thread.sleep(forTimeInterval: 0.1)
    }

    // MARK: - V2 Callback Handlers

    /// Handle V2 processing completion
    /// Called by EventHandlerV2.onComplete() when async processing finishes
    ///
    /// - Parameter response: The AI response text
    func handleV2Completion(response: String) {
        print("[InputCoordinator] V2 completion received (\(response.count) chars)")

        // Hide processing indicator
        hideProcessingIndicator()

        // Get pending context
        guard let outputContext = pendingV2OutputContext else {
            print("[InputCoordinator] Warning: No pending output context for V2 completion")
            return
        }

        // Clear pending context
        pendingV2OutputContext = nil
        pendingV2OriginalClipboard = nil

        // Perform output
        outputCoordinator?.previousFrontmostApp = previousFrontmostApp
        outputCoordinator?.performOutput(response: response, context: outputContext)
    }

    /// Handle V2 processing error
    /// Called by EventHandlerV2.onError() when async processing fails
    ///
    /// - Parameter message: Error message
    func handleV2Error(message: String) {
        print("[InputCoordinator] V2 error received: \(message)")

        // Hide processing indicator
        hideProcessingIndicator()

        // Clear clipboard monitor history
        clipboardMonitor.clearHistory()
        print("[InputCoordinator] 🗑️ Cleared clipboard monitor history after V2 error")

        // Restore original clipboard
        if let original = pendingV2OriginalClipboard {
            DispatchQueue.main.async { [weak self] in
                self?.clipboardManager.setText(original)
                print("[InputCoordinator] ♻️ Restored original clipboard content (after V2 error)")
            }
        }

        // Clear pending context
        pendingV2OutputContext = nil
        pendingV2OriginalClipboard = nil

        // Error is already shown via EventHandlerV2
    }

    /// Check if V2 processing is pending
    var isV2ProcessingPending: Bool {
        pendingV2OutputContext != nil
    }
}
