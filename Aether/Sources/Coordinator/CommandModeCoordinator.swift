//
//  CommandModeCoordinator.swift
//  Aether
//
//  Coordinator for managing command mode (slash command completion).
//  Extracted from AppDelegate to improve separation of concerns.
//
//  DEPRECATED: This coordinator is being replaced by UnifiedInputCoordinator
//  as part of the refactor-unified-halo-window initiative.
//  See UnifiedInputCoordinator.swift for the new implementation.
//

import AppKit
import Carbon.HIToolbox

// MARK: - Command Mode Coordinator

/// Coordinator for managing command mode input and completion
///
/// - Important: This class is deprecated and will be removed in a future version.
///   Use `UnifiedInputCoordinator` instead, which provides a unified Halo window
///   experience with integrated command completion, focus detection, and SubPanel support.
///
/// Responsibilities:
/// - Setup and manage command mode hotkey (Cmd+Opt+/)
/// - Handle keyboard input during command mode
/// - Coordinate with Halo for command completion UI
/// - Handle command selection and text insertion
@available(*, deprecated, message: "Use UnifiedInputCoordinator instead. This will be removed in Phase 8.")
final class CommandModeCoordinator {

    // MARK: - Dependencies

    /// Reference to core for config loading
    private weak var core: AetherCore?

    /// Reference to Halo window controller for command mode UI
    private weak var haloWindowController: HaloWindowController?

    // MARK: - Hotkey Configuration

    /// Command mode hotkey monitor
    private var commandHotkeyMonitor: Any?

    /// Command mode hotkey modifiers (default: Cmd+Opt)
    private var commandHotkeyModifiers: NSEvent.ModifierFlags = [.command, .option]

    /// Command mode hotkey key code (default: / = 44)
    private var commandHotkeyKeyCode: UInt16 = 44

    // MARK: - Input State

    /// Command mode input listener (captures keyboard input while command mode is active)
    private var commandModeInputMonitor: Any?

    // MARK: - Initialization

    init() {}

    /// Configure dependencies after initialization
    ///
    /// - Parameters:
    ///   - core: AetherCore instance for config
    ///   - haloWindowController: HaloWindowController for command mode UI
    func configure(core: AetherCore, haloWindowController: HaloWindowController?) {
        self.core = core
        self.haloWindowController = haloWindowController
    }

    // MARK: - Hotkey Setup

    /// Setup global hotkey for command mode (configurable, default: Cmd+Opt+/)
    func setupCommandModeHotkey() {
        // Load command prompt hotkey from config
        loadCommandPromptConfig()

        commandHotkeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return }
            // Check for configured hotkey
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.commandHotkeyModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }
            if modifiersMatch && event.keyCode == self.commandHotkeyKeyCode {
                self.handleCommandModeHotkey()
            }
        }
        print("[CommandModeCoordinator] Command mode hotkey monitor installed (keyCode: \(commandHotkeyKeyCode), modifiers: \(commandHotkeyModifiers))")
    }

    /// Load command prompt hotkey configuration from config
    private func loadCommandPromptConfig() {
        guard let core = core else { return }

        do {
            let config = try core.loadConfig()
            if let shortcuts = config.shortcuts {
                parseAndApplyCommandPromptHotkey(shortcuts.commandPrompt)
            }
        } catch {
            print("[CommandModeCoordinator] Failed to load command prompt config: \(error)")
        }
    }

    /// Parse command prompt config string (e.g., "Command+Option+/") and apply it
    private func parseAndApplyCommandPromptHotkey(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count == 3 else {
            print("[CommandModeCoordinator] Invalid command prompt config: \(configString)")
            return
        }

        var modifiers: NSEvent.ModifierFlags = []

        // Parse first two parts as modifiers
        for i in 0..<2 {
            switch parts[i] {
            case "Command": modifiers.insert(.command)
            case "Option": modifiers.insert(.option)
            case "Control": modifiers.insert(.control)
            case "Shift": modifiers.insert(.shift)
            default: break
            }
        }

        // Parse third part as key code
        let keyCode: UInt16
        switch parts[2] {
        case "/": keyCode = 44
        case "`": keyCode = 50
        case "\\": keyCode = 42
        case ";": keyCode = 41
        case ",": keyCode = 43
        case ".": keyCode = 47
        case "Space": keyCode = 49
        default: keyCode = 44  // Default to /
        }

        commandHotkeyModifiers = modifiers
        commandHotkeyKeyCode = keyCode
        print("[CommandModeCoordinator] Command prompt hotkey configured: \(configString) (keyCode: \(keyCode), modifiers: \(modifiers))")
    }

    /// Update command prompt hotkey at runtime (called from ShortcutsView)
    func updateCommandPromptHotkey(_ shortcuts: ShortcutsConfig) {
        parseAndApplyCommandPromptHotkey(shortcuts.commandPrompt)

        // Reinstall the monitor with new settings
        removeCommandModeHotkey()
        commandHotkeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return }
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.commandHotkeyModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }
            if modifiersMatch && event.keyCode == self.commandHotkeyKeyCode {
                self.handleCommandModeHotkey()
            }
        }
        print("[CommandModeCoordinator] Command prompt hotkey updated and monitor reinstalled")
    }

    /// Remove command mode hotkey monitor
    func removeCommandModeHotkey() {
        if let monitor = commandHotkeyMonitor {
            NSEvent.removeMonitor(monitor)
            commandHotkeyMonitor = nil
            print("[CommandModeCoordinator] Command mode hotkey monitor removed")
        }
    }

    // MARK: - Command Mode Handling

    /// Handle command mode hotkey (Cmd+Opt+/)
    private func handleCommandModeHotkey() {
        print("[CommandModeCoordinator] Command mode hotkey pressed")

        guard let haloWindow = haloWindowController?.window else {
            print("[CommandModeCoordinator] ❌ HaloWindow not available")
            return
        }

        // If already in command mode, toggle off
        if case .commandMode = haloWindow.viewModel.state {
            exitCommandMode()
            return
        }

        // Get best position: caret position (preferred) or mouse position (fallback)
        let haloPosition = CaretPositionHelper.getBestPosition()
        print("[CommandModeCoordinator] Command mode - haloPosition: (\(haloPosition.x), \(haloPosition.y))")

        // Type "/" character to the active application
        print("[CommandModeCoordinator] Typing '/' to active application")
        _ = KeyboardSimulator.shared.typeTextInstant("/")
        usleep(30_000) // 30ms delay

        // Activate command mode
        haloWindow.viewModel.commandManager.activateCommandMode { [weak self] selectedCommand in
            // When user selects a command, complete the input
            self?.handleCommandSelected(selectedCommand)
        }

        // CRITICAL: Set state directly (without animation) BEFORE showBelow
        // This ensures getWindowSize() returns the correct size for command mode
        // We bypass updateState() to avoid animation conflicts with showBelow()
        haloWindow.viewModel.state = .commandMode
        haloWindow.ignoresMouseEvents = false  // Enable mouse events for clicking commands

        // Show halo BELOW the caret (like IDE autocomplete)
        haloWindow.showBelow(at: haloPosition)

        // Start keyboard input listener for command mode
        startCommandModeInputListener()
    }

    /// Start listening for keyboard input during command mode
    private func startCommandModeInputListener() {
        // Remove existing monitor if any
        stopCommandModeInputListener()

        print("[CommandModeCoordinator] Starting command mode input listener")

        // Monitor global keyboard events
        commandModeInputMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.keyDown]) { [weak self] event in
            self?.handleCommandModeKeyEvent(event)
        }
    }

    /// Stop listening for keyboard input
    private func stopCommandModeInputListener() {
        if let monitor = commandModeInputMonitor {
            NSEvent.removeMonitor(monitor)
            commandModeInputMonitor = nil
            print("[CommandModeCoordinator] Stopped command mode input listener")
        }
    }

    /// Handle keyboard event during command mode
    private func handleCommandModeKeyEvent(_ event: NSEvent) {
        guard let haloWindow = haloWindowController?.window,
              case .commandMode = haloWindow.viewModel.state else {
            return
        }

        let commandManager = haloWindow.viewModel.commandManager
        let keyCode = event.keyCode

        // Handle special keys
        switch Int(keyCode) {
        case kVK_Escape:
            // Exit command mode
            print("[CommandModeCoordinator] Escape pressed, exiting command mode")
            exitCommandMode()
            return

        case kVK_Return:
            // Select current command
            print("[CommandModeCoordinator] Enter pressed, selecting current command")
            commandManager.selectCurrentCommand()
            return

        case kVK_UpArrow:
            // Move selection up
            commandManager.moveSelectionUp()
            return

        case kVK_DownArrow:
            // Move selection down
            commandManager.moveSelectionDown()
            return

        case kVK_Delete:
            // Backspace - remove last character from prefix
            var prefix = commandManager.inputPrefix
            if !prefix.isEmpty {
                prefix.removeLast()
                commandManager.inputPrefix = prefix
                print("[CommandModeCoordinator] Backspace, prefix now: '\(prefix)'")
            } else {
                // If prefix is empty and backspace, exit command mode
                print("[CommandModeCoordinator] Backspace on empty prefix, exiting command mode")
                exitCommandMode()
            }
            return

        case kVK_Tab:
            // Tab could auto-complete to first match
            if let firstCommand = commandManager.displayedCommands.first {
                commandManager.inputPrefix = firstCommand.key
            }
            return

        default:
            break
        }

        // Handle character input
        if let characters = event.charactersIgnoringModifiers, !characters.isEmpty {
            let char = characters.first!

            // Only accept alphanumeric and common command characters
            if char.isLetter || char.isNumber || char == "-" || char == "_" {
                let newPrefix = commandManager.inputPrefix + String(char)
                commandManager.inputPrefix = newPrefix
                print("[CommandModeCoordinator] Character input: '\(char)', prefix now: '\(newPrefix)'")
            }
        }
    }

    /// Exit command mode and clean up
    func exitCommandMode() {
        print("[CommandModeCoordinator] Exiting command mode")

        // Stop input listener first
        stopCommandModeInputListener()

        // Deactivate command manager
        haloWindowController?.window?.viewModel.commandManager.deactivateCommandMode()

        // Hide Halo
        haloWindowController?.window?.updateState(.idle)
        haloWindowController?.hide()
    }

    // MARK: - Command Selection

    /// Handle command selection from command completion
    private func handleCommandSelected(_ command: CommandNode) {
        print("[CommandModeCoordinator] Command selected: /\(command.key)")

        // Get the current input prefix (what user has typed so far, without the "/")
        let inputPrefix = haloWindowController?.window?.viewModel.commandManager.inputPrefix ?? ""

        // Stop input listener first
        stopCommandModeInputListener()

        // CRITICAL: Wait for Enter key event to be fully processed by the target app.
        usleep(100_000) // 100ms delay

        // NO-FLASH APPROACH: Use Accessibility API to read text and find "/" position.
        // Then use backspaces to delete exactly the right amount - no visual selection.

        var charsToDelete = 1 + inputPrefix.count  // Default: "/" + inputPrefix

        // Try to get text content via Accessibility API
        if let textBeforeCursor = getTextBeforeCursor(maxChars: charsToDelete + 5) {
            NSLog("[CommandModeCoordinator] DEBUG: Text before cursor: '%@'", textBeforeCursor)

            // Find "/" position from the end (rightmost "/")
            if let slashRange = textBeforeCursor.range(of: "/", options: .backwards) {
                let slashIndex = textBeforeCursor.distance(from: textBeforeCursor.startIndex, to: slashRange.lowerBound)
                charsToDelete = textBeforeCursor.count - slashIndex  // From "/" to end
                NSLog("[CommandModeCoordinator] DEBUG: Found '/' at index %d, will delete %d chars", slashIndex, charsToDelete)
            }
        } else {
            NSLog("[CommandModeCoordinator] DEBUG: Could not read text via Accessibility, using default count: %d", charsToDelete)
        }

        // Delete using backspaces (no visual selection)
        NSLog("[CommandModeCoordinator] Deleting %d characters with backspaces", charsToDelete)
        _ = KeyboardSimulator.shared.typeBackspaces(count: charsToDelete)
        usleep(50_000)

        // Type the complete command
        let commandText = "/\(command.key) "
        NSLog("[CommandModeCoordinator] Typing command: '%@'", commandText)
        _ = KeyboardSimulator.shared.typeTextInstant(commandText)

        // Note: deactivateCommandMode() will be called by selectCurrentCommand() after this callback returns

        // Hide Halo immediately (no success feedback needed since command is typed directly)
        haloWindowController?.window?.updateState(.idle)
        haloWindowController?.hide()
    }

    // MARK: - Accessibility Helpers

    /// Get text before cursor using Accessibility API (no visual selection)
    ///
    /// - Parameter maxChars: Maximum number of characters to retrieve
    /// - Returns: Text before cursor, or nil if unavailable
    private func getTextBeforeCursor(maxChars: Int) -> String? {
        let systemWide = AXUIElementCreateSystemWide()

        // Get focused element
        var focusedRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(systemWide, kAXFocusedUIElementAttribute as CFString, &focusedRef) == .success,
              let focused = focusedRef else {
            return nil
        }

        let element = focused as! AXUIElement

        // Get selected text range (cursor position)
        var rangeRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, &rangeRef) == .success,
              let rangeValue = rangeRef else {
            return nil
        }

        // Extract range
        var range = CFRange(location: 0, length: 0)
        guard AXValueGetValue(rangeValue as! AXValue, .cfRange, &range) else {
            return nil
        }

        // Calculate range for text before cursor
        let cursorPosition = range.location
        let startPosition = max(0, cursorPosition - maxChars)
        let length = cursorPosition - startPosition

        guard length > 0 else {
            return ""
        }

        // Create range for text before cursor
        var textRange = CFRange(location: startPosition, length: length)
        guard let textRangeValue = AXValueCreate(.cfRange, &textRange) else {
            return nil
        }

        // Get text for range
        var textRef: CFTypeRef?
        guard AXUIElementCopyParameterizedAttributeValue(
            element,
            kAXStringForRangeParameterizedAttribute as CFString,
            textRangeValue,
            &textRef
        ) == .success,
              let text = textRef as? String else {
            return nil
        }

        return text
    }

    // MARK: - Cleanup

    /// Clean up all resources
    func cleanup() {
        removeCommandModeHotkey()
        stopCommandModeInputListener()
    }
}
