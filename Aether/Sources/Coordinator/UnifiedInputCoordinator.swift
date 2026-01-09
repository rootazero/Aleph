//
//  UnifiedInputCoordinator.swift
//  Aether
//
//  Coordinator for managing the unified Halo input window.
//  Combines command completion, multi-turn conversation, and AI interaction
//  into a single, cohesive flow with a Raycast-style SubPanel.
//
//  Part of: refactor-unified-halo-window
//

import AppKit
import SwiftUI
import Carbon.HIToolbox

// MARK: - UnifiedInputCoordinator

/// Coordinator for managing unified Halo input flow
///
/// Responsibilities:
/// - Setup and manage unified hotkey (Cmd+Opt+/)
/// - Check cursor focus before showing Halo (via FocusDetector)
/// - Show UnifiedInputView with SubPanel integration
/// - Handle command completion inline (no typing "/" to target app)
/// - Start multi-turn conversation immediately
/// - Route output to target application
final class UnifiedInputCoordinator {

    // MARK: - Dependencies

    /// Reference to core for processing
    private weak var core: AetherCore?

    /// Reference to Halo window controller for UI
    private weak var haloWindowController: HaloWindowController?

    /// Reference to event handler for callbacks
    private weak var eventHandler: EventHandler?

    /// Reference to output coordinator for response output
    private weak var outputCoordinator: OutputCoordinator?

    /// Reference to conversation coordinator for multi-turn conversations
    private weak var conversationCoordinator: ConversationCoordinator?

    /// Clipboard manager for clipboard operations
    private let clipboardManager: any ClipboardManagerProtocol

    /// Clipboard monitor for context tracking
    private let clipboardMonitor: any ClipboardMonitorProtocol

    /// Focus detector for cursor position and input field detection
    private let focusDetector: FocusDetector

    // MARK: - Hotkey Configuration

    /// Unified hotkey monitor
    private var unifiedHotkeyMonitor: Any?

    /// Unified hotkey modifiers (default: Cmd+Opt)
    private var unifiedHotkeyModifiers: NSEvent.ModifierFlags = [.command, .option]

    /// Unified hotkey key code (default: / = 44)
    private var unifiedHotkeyKeyCode: UInt16 = 44

    // MARK: - State

    /// Store the frontmost app when hotkey is pressed
    private(set) var previousFrontmostApp: NSRunningApplication?

    /// Store target app info from focus detection
    private(set) var targetAppInfo: TargetAppInfo?

    /// Whether permission gate is active (blocks input)
    var isPermissionGateActive: Bool = false

    /// Current session ID for multi-turn conversation
    private var currentSessionId: String?

    /// Current turn count
    private var currentTurnCount: UInt32 = 0

    /// Whether to display output in SubPanel CLI mode (vs target app)
    /// Set to true when cursor is not in an input field
    private var useCLIOutputMode: Bool = false

    // MARK: - SubPanel Access

    /// Access SubPanelState for CLI output
    private var subPanelState: SubPanelState? {
        haloWindowController?.window?.viewModel.subPanelState
    }

    // MARK: - Initialization

    /// Initialize the unified input coordinator
    ///
    /// - Parameters:
    ///   - clipboardManager: Clipboard manager for operations
    ///   - clipboardMonitor: Clipboard monitor for context tracking
    ///   - focusDetector: Focus detector for cursor position
    init(
        clipboardManager: any ClipboardManagerProtocol = DependencyContainer.shared.clipboardManager,
        clipboardMonitor: any ClipboardMonitorProtocol = DependencyContainer.shared.clipboardMonitor,
        focusDetector: FocusDetector = FocusDetector()
    ) {
        self.clipboardManager = clipboardManager
        self.clipboardMonitor = clipboardMonitor
        self.focusDetector = focusDetector
    }

    /// Configure dependencies after initialization
    ///
    /// - Parameters:
    ///   - core: AetherCore instance
    ///   - haloWindowController: HaloWindowController for UI
    ///   - eventHandler: EventHandler for callbacks
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

    // MARK: - Hotkey Setup

    /// Setup global hotkey for unified input (configurable, default: Cmd+Opt+/)
    func setupUnifiedHotkey() {
        // Load unified hotkey from config
        loadUnifiedHotkeyConfig()

        unifiedHotkeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return }

            // Check for configured hotkey
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.unifiedHotkeyModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }
            if modifiersMatch && event.keyCode == self.unifiedHotkeyKeyCode {
                self.handleUnifiedHotkey()
            }
        }
        print("[UnifiedInputCoordinator] Unified hotkey monitor installed (keyCode: \(unifiedHotkeyKeyCode), modifiers: \(unifiedHotkeyModifiers))")
    }

    /// Load unified hotkey configuration from config
    private func loadUnifiedHotkeyConfig() {
        guard let core = core else { return }

        do {
            let config = try core.loadConfig()
            if let shortcuts = config.shortcuts {
                parseAndApplyUnifiedHotkey(shortcuts.commandPrompt)
            }
        } catch {
            print("[UnifiedInputCoordinator] Failed to load unified hotkey config: \(error)")
        }
    }

    /// Parse unified hotkey config string (e.g., "Command+Option+/") and apply it
    private func parseAndApplyUnifiedHotkey(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count >= 2 else {
            print("[UnifiedInputCoordinator] Invalid unified hotkey config: \(configString)")
            return
        }

        var modifiers: NSEvent.ModifierFlags = []

        // Parse modifiers (all parts except the last)
        for i in 0..<(parts.count - 1) {
            switch parts[i] {
            case "Command": modifiers.insert(.command)
            case "Option": modifiers.insert(.option)
            case "Control": modifiers.insert(.control)
            case "Shift": modifiers.insert(.shift)
            default: break
            }
        }

        // Parse last part as key code
        let keyCode: UInt16
        switch parts[parts.count - 1] {
        case "/": keyCode = 44
        case "`": keyCode = 50
        case "\\": keyCode = 42
        case ";": keyCode = 41
        case ",": keyCode = 43
        case ".": keyCode = 47
        case "Space": keyCode = 49
        default: keyCode = 44  // Default to /
        }

        unifiedHotkeyModifiers = modifiers
        unifiedHotkeyKeyCode = keyCode
        print("[UnifiedInputCoordinator] Unified hotkey configured: \(configString) (keyCode: \(keyCode), modifiers: \(modifiers))")
    }

    /// Update unified hotkey at runtime (called from ShortcutsView)
    func updateUnifiedHotkey(_ shortcuts: ShortcutsConfig) {
        parseAndApplyUnifiedHotkey(shortcuts.commandPrompt)

        // Reinstall the monitor with new settings
        removeUnifiedHotkey()
        setupUnifiedHotkey()
        print("[UnifiedInputCoordinator] Unified hotkey updated and monitor reinstalled")
    }

    /// Remove unified hotkey monitor
    func removeUnifiedHotkey() {
        if let monitor = unifiedHotkeyMonitor {
            NSEvent.removeMonitor(monitor)
            unifiedHotkeyMonitor = nil
            print("[UnifiedInputCoordinator] Unified hotkey monitor removed")
        }
    }

    // MARK: - Unified Hotkey Handling

    /// Handle unified hotkey (Cmd+Opt+/)
    private func handleUnifiedHotkey() {
        print("[UnifiedInputCoordinator] Unified hotkey pressed")

        // Block if permission gate is active
        guard !isPermissionGateActive else {
            print("[UnifiedInputCoordinator] ⚠️ Blocked - permission gate active")
            NSSound.beep()
            return
        }

        guard let haloWindow = haloWindowController?.window else {
            print("[UnifiedInputCoordinator] ❌ HaloWindow not available")
            return
        }

        // If already in unified input mode, toggle off
        if case .unifiedInput = haloWindow.viewModel.state {
            exitUnifiedInput()
            return
        }

        // Store frontmost app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[UnifiedInputCoordinator] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Check cursor focus using FocusDetector
        let focusResult = focusDetector.checkInputFocus()

        switch focusResult {
        case .focused(let info):
            // Cursor is in an input field - show unified input at caret position
            print("[UnifiedInputCoordinator] ✅ Cursor focused in: \(info.bundleId)")
            targetAppInfo = info
            showUnifiedInput(at: info.caretPosition)

        case .notFocused:
            // Cursor is not in an input field - show warning toast
            print("[UnifiedInputCoordinator] ⚠️ Cursor not in input field")
            showFocusWarningToast()

        case .accessibilityDenied:
            // Accessibility permission denied - show toast and fall back to mouse position
            print("[UnifiedInputCoordinator] ⚠️ Accessibility permission denied, using mouse position")
            showAccessibilityWarningToast()
            let mousePosition = NSEvent.mouseLocation
            showUnifiedInput(at: mousePosition)

        case .unknownError(let error):
            // Error during detection - fall back to mouse position
            print("[UnifiedInputCoordinator] ⚠️ Focus detection error: \(error.localizedDescription), using mouse position")
            let mousePosition = NSEvent.mouseLocation
            showUnifiedInput(at: mousePosition)
        }
    }

    /// Show focus warning toast
    /// Uses `.info` type to enable auto-dismiss behavior
    private func showFocusWarningToast() {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.eventHandler?.showToast(
                type: .info,
                title: L("unified.focus_warning.title"),
                message: L("unified.focus_warning.message"),
                autoDismiss: true
            )
        }
    }

    /// Show accessibility permission warning toast
    private func showAccessibilityWarningToast() {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.eventHandler?.showToast(
                type: .warning,
                title: L("unified.accessibility_warning.title"),
                message: L("unified.accessibility_warning.message"),
                autoDismiss: false  // User should manually dismiss or go to settings
            )
        }
    }

    /// Show unified input at specified position
    ///
    /// - Parameter position: Screen position to show the Halo window
    private func showUnifiedInput(at position: NSPoint) {
        print("[UnifiedInputCoordinator] Showing unified input at: (\(position.x), \(position.y))")

        guard let haloWindow = haloWindowController?.window else {
            print("[UnifiedInputCoordinator] ❌ HaloWindow not available")
            return
        }

        // Generate a new session ID for this conversation
        currentSessionId = UUID().uuidString
        currentTurnCount = 0

        // Show unified input window with callbacks
        haloWindow.showUnifiedInput(
            at: position,
            sessionId: currentSessionId!,
            turnCount: currentTurnCount,
            onSubmit: { [weak self] text in
                self?.handleInputSubmitted(text)
            },
            onCancel: { [weak self] in
                self?.exitUnifiedInput()
            },
            onCommandSelected: { [weak self] command in
                self?.handleCommandSelected(command)
            }
        )

        print("[UnifiedInputCoordinator] Unified input shown, sessionId: \(currentSessionId!)")
    }

    // MARK: - Input Handling

    /// Handle input submitted from UnifiedInputView
    ///
    /// - Parameter text: The submitted text
    private func handleInputSubmitted(_ text: String) {
        let trimmedText = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedText.isEmpty else { return }

        print("[UnifiedInputCoordinator] Input submitted: \(trimmedText.prefix(50))...")

        // Check if it's a command (starts with /)
        if trimmedText.hasPrefix("/") {
            handleCommandInput(trimmedText)
        } else {
            handleConversationInput(trimmedText)
        }
    }

    /// Handle command input (starts with /)
    ///
    /// - Parameter text: The command text including the "/" prefix
    private func handleCommandInput(_ text: String) {
        print("[UnifiedInputCoordinator] Command input: \(text)")

        // Extract command key and content
        let parts = text.dropFirst().split(separator: " ", maxSplits: 1)
        let commandKey = String(parts.first ?? "")
        let commandContent = parts.count > 1 ? String(parts[1]) : ""

        print("[UnifiedInputCoordinator] Command key: \(commandKey), content: \(commandContent.prefix(30))...")

        // Process the command with the content
        processCommandWithContent(commandKey: commandKey, content: commandContent)
    }

    /// Handle conversation input (no command prefix)
    ///
    /// - Parameter text: The conversation text
    private func handleConversationInput(_ text: String) {
        print("[UnifiedInputCoordinator] Conversation input: \(text.prefix(50))...")

        guard let core = core else {
            print("[UnifiedInputCoordinator] ❌ Core not available")
            return
        }

        // Start CLI output in SubPanel
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.startCLIOutput()
            slf.appendCLIOutput(L("subpanel.cli.sending"), type: .command)
        }

        // Capture context
        let windowContext = ContextCapture.captureContext()
        let capturedContext = CapturedContext(
            appBundleId: windowContext.bundleId ?? "unknown",
            windowTitle: windowContext.windowTitle,
            attachments: nil
        )

        // Store conversation context for output handling
        conversationCoordinator?.storeConversationContext(
            textSource: .accessibilityAPI,  // We're using unified input, not selection
            useCutMode: false,  // Unified input appends, doesn't replace
            originalClipboard: clipboardManager.getText()
        )
        conversationCoordinator?.previousFrontmostApp = previousFrontmostApp

        // Start or continue conversation
        if currentTurnCount == 0 {
            // First turn - start new conversation
            conversationCoordinator?.startConversation(userInput: text, context: capturedContext)
        } else {
            // Continuation - continue existing conversation
            conversationCoordinator?.continueConversation(followUpInput: text)
        }

        currentTurnCount += 1
    }

    /// Process a command with its content
    ///
    /// - Parameters:
    ///   - commandKey: The command key (without "/")
    ///   - content: The content following the command
    private func processCommandWithContent(commandKey: String, content: String) {
        guard let core = core else {
            print("[UnifiedInputCoordinator] ❌ Core not available")
            return
        }

        // Build the full input for routing
        let fullInput = "/\(commandKey) \(content)"

        // Start CLI output in SubPanel
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.startCLIOutput()
            slf.appendCLIOutput("/\(commandKey)", type: .command)
            slf.appendThinkingOutput(L("subpanel.cli.routing"))
        }

        // Capture context
        let windowContext = ContextCapture.captureContext()
        let capturedContext = CapturedContext(
            appBundleId: windowContext.bundleId ?? "unknown",
            windowTitle: windowContext.windowTitle,
            attachments: nil
        )

        // Process in background
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }

            // Update CLI: connecting to AI
            self.appendThinkingOutput(L("subpanel.cli.connecting"))

            do {
                let response = try core.processInput(
                    userInput: fullInput,
                    context: capturedContext
                )

                print("[UnifiedInputCoordinator] Command response received (\(response.count) chars)")

                // Update CLI: response received
                self.appendCLIOutput(L("subpanel.cli.response_received"), type: .success)

                // Output the response
                self.outputResponse(response)

            } catch {
                print("[UnifiedInputCoordinator] ❌ Error processing command: \(error)")
                self.showCLIError(error.localizedDescription)
            }
        }
    }

    /// Output response to target application or CLI
    ///
    /// - Parameter response: The AI response to output
    private func outputResponse(_ response: String) {
        // Update CLI output status
        appendCLIOutput(L("subpanel.cli.outputting"), type: .info)
        completeCLIOutput()

        // If useCLIOutputMode is true, display in SubPanel instead of target app
        if useCLIOutputMode {
            // Show full response in CLI mode
            appendStreamingOutput(response)
            showCLISuccess(L("subpanel.cli.completed"))
            return
        }

        // Output to target application
        let outputContext = OutputContext(
            useReplaceMode: false,  // Unified input appends
            textSource: .accessibilityAPI,
            sessionType: .singleTurn,
            originalClipboard: clipboardManager.getText(),
            turnId: nil,
            conversationSessionId: nil
        )
        outputCoordinator?.previousFrontmostApp = previousFrontmostApp
        outputCoordinator?.performOutput(response: response, context: outputContext)
    }

    // MARK: - Command Selection

    /// Handle command selected from SubPanel
    ///
    /// - Parameter command: The selected CommandNode
    private func handleCommandSelected(_ command: CommandNode) {
        print("[UnifiedInputCoordinator] Command selected: /\(command.key)")

        // If command has children (namespace), show children
        if command.hasChildren {
            // TODO: Navigate to children in SubPanel
            print("[UnifiedInputCoordinator] Command has children, navigating...")
            return
        }

        // Execute the command without content for now
        // The user can add content in the input field
        processCommandWithContent(commandKey: command.key, content: "")
    }

    // MARK: - Exit

    /// Exit unified input mode and clean up
    func exitUnifiedInput() {
        print("[UnifiedInputCoordinator] Exiting unified input mode")

        // Clear state
        currentSessionId = nil
        currentTurnCount = 0
        targetAppInfo = nil

        // Clear callbacks
        haloWindowController?.window?.viewModel.callbacks.unifiedInputOnSubmit = nil
        haloWindowController?.window?.viewModel.callbacks.unifiedInputOnCancel = nil
        haloWindowController?.window?.viewModel.callbacks.unifiedInputOnCommandSelected = nil

        // Hide Halo
        haloWindowController?.window?.updateState(.idle)
        haloWindowController?.forceHide()
    }

    // MARK: - CLI Output

    /// Start CLI output mode in SubPanel
    private func startCLIOutput() {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.subPanelState?.showCLIOutput(initialLines: [
                CLIOutputLine(type: .info, content: L("subpanel.cli.processing"))
            ])
        }
    }

    /// Append a line to CLI output
    private func appendCLIOutput(_ text: String, type: CLIOutputType = .info) {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.subPanelState?.appendCLIText(text, type: type)
        }
    }

    /// Append thinking indicator to CLI output
    private func appendThinkingOutput(_ text: String) {
        appendCLIOutput(text, type: .thinking)
    }

    /// Append streaming response to CLI output
    private func appendStreamingOutput(_ text: String) {
        appendCLIOutput(text, type: .info)
    }

    /// Complete CLI output (stop streaming indicator)
    private func completeCLIOutput() {
        DispatchQueue.main.async { [weak self] in
            self?.subPanelState?.completeCLIOutput()
        }
    }

    /// Show success message in CLI output
    private func showCLISuccess(_ message: String) {
        appendCLIOutput(message, type: .success)
        completeCLIOutput()
    }

    /// Show error message in CLI output
    private func showCLIError(_ message: String) {
        appendCLIOutput(message, type: .error)
        completeCLIOutput()
    }

    // MARK: - Cleanup

    /// Clean up all resources
    func cleanup() {
        removeUnifiedHotkey()
        exitUnifiedInput()
    }
}

// MARK: - Localization Helper

private func L(_ key: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    return localized == key ? fallbackString(for: key) : localized
}

private func fallbackString(for key: String) -> String {
    switch key {
    case "unified.focus_warning.title": return "请先点击输入框"
    case "unified.focus_warning.message": return "将光标移动到输入框后再呼出 Aether"
    case "unified.accessibility_warning.title": return "需要辅助功能权限"
    case "unified.accessibility_warning.message": return "请在系统设置中授予辅助功能权限以获得更好的体验"
    // CLI output strings
    case "subpanel.cli.processing": return "处理中..."
    case "subpanel.cli.sending": return "发送请求..."
    case "subpanel.cli.routing": return "路由到 AI..."
    case "subpanel.cli.connecting": return "连接中..."
    case "subpanel.cli.response_received": return "✓ 收到响应"
    case "subpanel.cli.outputting": return "输出中..."
    case "subpanel.cli.completed": return "✓ 完成"
    default: return key
    }
}
