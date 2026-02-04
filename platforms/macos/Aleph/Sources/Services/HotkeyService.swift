//
//  HotkeyService.swift
//  Aether
//
//  Unified hotkey management service that coordinates hotkey systems:
//  - Conversation hotkey: Command prompt hotkey (Option+Space)
//
//  Extracted from AppDelegate to improve separation of concerns.
//

import AppKit
import Combine

// MARK: - Hotkey Service

/// Unified service for managing all application hotkeys
///
/// This service consolidates hotkey management that was previously scattered
/// across AppDelegate, providing a single point of control for:
/// - Conversation hotkey (inline implementation)
final class HotkeyService {

    // MARK: - Properties

    /// Conversation hotkey monitors
    private var conversationGlobalMonitor: Any?
    private var conversationLocalMonitor: Any?

    /// Conversation hotkey configuration
    private var conversationModifiers: NSEvent.ModifierFlags = [.option]
    private var conversationKeyCode: UInt16 = 49 // Space key

    /// Reference to core for loading config
    private weak var core: AetherCore?

    // MARK: - Initialization

    init() {}

    // MARK: - Configuration

    /// Configure the hotkey service with core reference
    ///
    /// - Parameter core: AetherCore instance for loading configuration
    func configure(core: AetherCore?) {
        self.core = core
        print("[HotkeyService] Configured with core: \(core != nil ? "available" : "nil")")
    }

    // MARK: - Start/Stop All Hotkeys

    /// Start all hotkey monitoring
    func startAllHotkeys() {
        startConversationHotkey()
        print("[HotkeyService] All hotkey systems started")
    }

    /// Stop all hotkey monitoring
    func stopAllHotkeys() {
        stopConversationHotkey()
        print("[HotkeyService] All hotkey systems stopped")
    }

    // MARK: - Conversation Hotkey (Command Prompt)

    /// Start conversation hotkey monitoring
    private func startConversationHotkey() {
        // Load configuration from core
        loadConversationConfig()

        // Create hotkey handler
        let hotkeyHandler: (NSEvent) -> Bool = { [weak self] event in
            guard let self = self else { return false }

            // Check modifier match
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.conversationModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }

            if modifiersMatch && event.keyCode == self.conversationKeyCode {
                // Dispatch to MainActor since HaloInputCoordinator is @MainActor isolated
                Task { @MainActor in
                    HaloInputCoordinator.shared.handleHotkey()
                }
                return true
            }
            return false
        }

        // Global monitor - when OTHER apps are active
        conversationGlobalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { event in
            _ = hotkeyHandler(event)
        }

        // Local monitor - when AETHER is active
        conversationLocalMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            if hotkeyHandler(event) {
                return nil // Consume event
            }
            return event // Pass through
        }

        print("[HotkeyService] Conversation hotkey started (keyCode: \(conversationKeyCode), modifiers: \(conversationModifiers))")
    }

    /// Stop conversation hotkey monitoring
    private func stopConversationHotkey() {
        if let monitor = conversationGlobalMonitor {
            NSEvent.removeMonitor(monitor)
            conversationGlobalMonitor = nil
        }
        if let monitor = conversationLocalMonitor {
            NSEvent.removeMonitor(monitor)
            conversationLocalMonitor = nil
        }
    }

    /// Load conversation hotkey configuration from core
    private func loadConversationConfig() {
        guard let core = core else {
            print("[HotkeyService] WARNING: Core is nil, cannot load hotkey config")
            print("[HotkeyService] Using default: Option+Space")
            return
        }

        do {
            let config = try core.loadConfig()
            if let shortcuts = config.shortcuts {
                print("[HotkeyService] Loading conversation hotkey from config: \(shortcuts.commandPrompt)")
                parseConversationHotkey(shortcuts.commandPrompt)
            } else {
                print("[HotkeyService] No shortcuts section in config, using default")
            }
        } catch {
            print("[HotkeyService] Failed to load hotkey config: \(error)")
        }
    }

    /// Parse conversation hotkey config string (e.g., "Option+Space")
    private func parseConversationHotkey(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count >= 2 else {
            print("[HotkeyService] Invalid hotkey config: \(configString)")
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
        case "Esc", "Escape": keyCode = 53
        default: keyCode = 44 // Default to /
        }

        conversationModifiers = modifiers
        conversationKeyCode = keyCode
        print("[HotkeyService] Conversation hotkey configured: \(configString)")
    }

    /// Update conversation hotkey at runtime
    ///
    /// - Parameter shortcuts: New shortcuts configuration
    func updateConversationHotkey(shortcuts: ShortcutsConfig) {
        parseConversationHotkey(shortcuts.commandPrompt)

        // Reinstall monitors with new settings
        stopConversationHotkey()
        startConversationHotkey()

        print("[HotkeyService] Conversation hotkey updated and monitors reinstalled")
    }

    // MARK: - Cleanup

    deinit {
        stopAllHotkeys()
    }
}
