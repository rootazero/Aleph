//
//  HotkeyService.swift
//  Aether
//
//  Unified hotkey management service that coordinates all hotkey systems:
//  - GlobalHotkeyMonitor: Double-tap modifier keys (Replace/Append)
//  - VisionHotkeyManager: OCR capture hotkey
//  - Multi-turn hotkey: Command prompt hotkey (Cmd+Opt+/)
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
/// - Replace/Append double-tap hotkeys (via GlobalHotkeyMonitor)
/// - Vision/OCR capture hotkey (via VisionHotkeyManager)
/// - Multi-turn conversation hotkey (inline implementation)
final class HotkeyService {

    // MARK: - Properties

    /// Global hotkey monitor for Replace/Append double-tap
    private var globalHotkeyMonitor: GlobalHotkeyMonitor?

    /// Vision hotkey manager for OCR capture
    private var visionHotkeyManager: VisionHotkeyManager?

    /// Multi-turn hotkey monitors
    private var multiTurnGlobalMonitor: Any?
    private var multiTurnLocalMonitor: Any?

    /// Multi-turn hotkey configuration
    private var multiTurnModifiers: NSEvent.ModifierFlags = [.command, .option]
    private var multiTurnKeyCode: UInt16 = 44 // "/" key

    /// Callbacks for hotkey events
    private var onReplaceTriggered: (() -> Void)?
    private var onAppendTriggered: (() -> Void)?

    /// Reference to core for loading config
    private weak var core: AetherCore?

    // MARK: - Initialization

    init() {}

    // MARK: - Configuration

    /// Configure the hotkey service with callbacks and core reference
    ///
    /// - Parameters:
    ///   - core: AetherCore instance for loading configuration
    ///   - onReplace: Callback when Replace hotkey is triggered
    ///   - onAppend: Callback when Append hotkey is triggered
    func configure(
        core: AetherCore?,
        onReplace: @escaping () -> Void,
        onAppend: @escaping () -> Void
    ) {
        self.core = core
        self.onReplaceTriggered = onReplace
        self.onAppendTriggered = onAppend
    }

    // MARK: - Start/Stop All Hotkeys

    /// Start all hotkey monitoring
    ///
    /// - Parameter triggerConfig: Trigger configuration for Replace/Append keys
    func startAllHotkeys(triggerConfig: TriggerConfig) {
        startGlobalHotkeys(triggerConfig: triggerConfig)
        startVisionHotkeys()
        startMultiTurnHotkey()

        print("[HotkeyService] All hotkey systems started")
    }

    /// Stop all hotkey monitoring
    func stopAllHotkeys() {
        stopGlobalHotkeys()
        stopVisionHotkeys()
        stopMultiTurnHotkey()

        print("[HotkeyService] All hotkey systems stopped")
    }

    // MARK: - Global Hotkeys (Replace/Append)

    /// Start global hotkey monitoring for Replace/Append
    private func startGlobalHotkeys(triggerConfig: TriggerConfig) {
        guard let onReplace = onReplaceTriggered,
              let onAppend = onAppendTriggered else {
            print("[HotkeyService] WARNING: Callbacks not configured for global hotkeys")
            return
        }

        // Parse Replace key
        let replaceKey = parseModifierKey(triggerConfig.replaceHotkey) ?? .leftShift

        // Parse Append key
        let appendKey = parseModifierKey(triggerConfig.appendHotkey) ?? .rightShift

        globalHotkeyMonitor = GlobalHotkeyMonitor(
            replaceKey: replaceKey,
            appendKey: appendKey,
            onReplaceTriggered: onReplace,
            onAppendTriggered: onAppend
        )

        if globalHotkeyMonitor?.startMonitoring() == true {
            print("[HotkeyService] Global hotkeys started: replace=\(replaceKey.displayName), append=\(appendKey.displayName)")
        } else {
            print("[HotkeyService] WARNING: Failed to start global hotkey monitoring")
        }
    }

    /// Stop global hotkey monitoring
    private func stopGlobalHotkeys() {
        globalHotkeyMonitor?.stopMonitoring()
        globalHotkeyMonitor = nil
    }

    /// Update global hotkey configuration at runtime
    ///
    /// - Parameter triggerConfig: New trigger configuration
    func updateGlobalHotkeys(triggerConfig: TriggerConfig) {
        let replaceKey = parseModifierKey(triggerConfig.replaceHotkey) ?? .leftShift
        let appendKey = parseModifierKey(triggerConfig.appendHotkey) ?? .rightShift

        globalHotkeyMonitor?.configureTrigger(
            replaceKey: replaceKey,
            appendKey: appendKey
        )

        print("[HotkeyService] Global hotkeys updated: replace=\(replaceKey.displayName), append=\(appendKey.displayName)")
    }

    // MARK: - Vision Hotkeys (OCR Capture)

    /// Start vision hotkey monitoring
    private func startVisionHotkeys() {
        visionHotkeyManager = VisionHotkeyManager()
        visionHotkeyManager?.registerHotkeys()

        // Load config and update hotkey if available
        if let core = core {
            do {
                let config = try core.loadConfig()
                if let shortcuts = config.shortcuts {
                    visionHotkeyManager?.updateHotkey(from: shortcuts)
                }
            } catch {
                print("[HotkeyService] Failed to load vision hotkey config: \(error)")
            }
        }

        print("[HotkeyService] Vision hotkeys started")
    }

    /// Stop vision hotkey monitoring
    private func stopVisionHotkeys() {
        visionHotkeyManager?.unregisterHotkeys()
        visionHotkeyManager = nil
    }

    /// Update vision hotkey configuration at runtime
    ///
    /// - Parameter shortcuts: New shortcuts configuration
    func updateVisionHotkey(shortcuts: ShortcutsConfig) {
        visionHotkeyManager?.updateHotkey(from: shortcuts)
        print("[HotkeyService] Vision hotkey updated")
    }

    // MARK: - Multi-Turn Hotkey (Command Prompt)

    /// Start multi-turn conversation hotkey monitoring
    private func startMultiTurnHotkey() {
        // Load configuration from core
        loadMultiTurnConfig()

        // Create hotkey handler
        let hotkeyHandler: (NSEvent) -> Bool = { [weak self] event in
            guard let self = self else { return false }

            // Check modifier match
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.multiTurnModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }

            if modifiersMatch && event.keyCode == self.multiTurnKeyCode {
                // Dispatch to MainActor since MultiTurnCoordinator is @MainActor isolated
                Task { @MainActor in
                    MultiTurnCoordinator.shared.handleHotkey()
                }
                return true
            }
            return false
        }

        // Global monitor - when OTHER apps are active
        multiTurnGlobalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { event in
            _ = hotkeyHandler(event)
        }

        // Local monitor - when AETHER is active
        multiTurnLocalMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            if hotkeyHandler(event) {
                return nil // Consume event
            }
            return event // Pass through
        }

        print("[HotkeyService] Multi-turn hotkey started (keyCode: \(multiTurnKeyCode), modifiers: \(multiTurnModifiers))")
    }

    /// Stop multi-turn hotkey monitoring
    private func stopMultiTurnHotkey() {
        if let monitor = multiTurnGlobalMonitor {
            NSEvent.removeMonitor(monitor)
            multiTurnGlobalMonitor = nil
        }
        if let monitor = multiTurnLocalMonitor {
            NSEvent.removeMonitor(monitor)
            multiTurnLocalMonitor = nil
        }
    }

    /// Load multi-turn hotkey configuration from core
    private func loadMultiTurnConfig() {
        guard let core = core else { return }

        do {
            let config = try core.loadConfig()
            if let shortcuts = config.shortcuts {
                parseMultiTurnHotkey(shortcuts.commandPrompt)
            }
        } catch {
            print("[HotkeyService] Failed to load multi-turn hotkey config: \(error)")
        }
    }

    /// Parse multi-turn hotkey config string (e.g., "Command+Option+/")
    private func parseMultiTurnHotkey(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count >= 2 else {
            print("[HotkeyService] Invalid multi-turn hotkey config: \(configString)")
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
        default: keyCode = 44 // Default to /
        }

        multiTurnModifiers = modifiers
        multiTurnKeyCode = keyCode
        print("[HotkeyService] Multi-turn hotkey configured: \(configString)")
    }

    /// Update multi-turn hotkey at runtime
    ///
    /// - Parameter shortcuts: New shortcuts configuration
    func updateMultiTurnHotkey(shortcuts: ShortcutsConfig) {
        parseMultiTurnHotkey(shortcuts.commandPrompt)

        // Reinstall monitors with new settings
        stopMultiTurnHotkey()
        startMultiTurnHotkey()

        print("[HotkeyService] Multi-turn hotkey updated and monitors reinstalled")
    }

    // MARK: - Helper Methods

    /// Parse modifier key from config string
    ///
    /// Note: Only Shift, Option, Command are supported (no Control - macOS limitation)
    private func parseModifierKey(_ configString: String) -> ModifierKey? {
        let lowercased = configString.lowercased()

        if lowercased.contains("left") {
            if lowercased.contains("shift") { return ModifierKey.leftShift }
            if lowercased.contains("option") || lowercased.contains("alt") { return ModifierKey.leftOption }
            if lowercased.contains("command") || lowercased.contains("cmd") { return ModifierKey.leftCommand }
        } else if lowercased.contains("right") {
            if lowercased.contains("shift") { return ModifierKey.rightShift }
            if lowercased.contains("option") || lowercased.contains("alt") { return ModifierKey.rightOption }
            if lowercased.contains("command") || lowercased.contains("cmd") { return ModifierKey.rightCommand }
        }

        // Default to left variant
        if lowercased.contains("shift") { return ModifierKey.leftShift }
        if lowercased.contains("option") || lowercased.contains("alt") { return ModifierKey.leftOption }
        if lowercased.contains("command") || lowercased.contains("cmd") { return ModifierKey.leftCommand }

        return nil
    }

    // MARK: - Cleanup

    deinit {
        stopAllHotkeys()
    }
}
