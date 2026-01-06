//
//  TriggerTypes.swift
//  Aether
//
//  Hotkey trigger system types for the refactored hotkey system.
//  This module defines types for configuring Replace/Append hotkeys.
//

import Foundation

// MARK: - ModifierKey

/// Modifier key types that can be used for double-tap triggers
///
/// Supported modifier keys with left/right variants.
/// Note: fn key is NOT supported (macOS system-level interception)
enum ModifierKey: String, CaseIterable, Codable {
    // Shift
    case leftShift = "leftShift"
    case rightShift = "rightShift"
    // Control
    case leftControl = "leftControl"
    case rightControl = "rightControl"
    // Option
    case leftOption = "leftOption"
    case rightOption = "rightOption"
    // Command
    case leftCommand = "leftCommand"
    case rightCommand = "rightCommand"

    /// macOS keyCode for this modifier key
    var keyCode: UInt16 {
        switch self {
        case .leftShift: return 56
        case .rightShift: return 60
        case .leftControl: return 59
        case .rightControl: return 62
        case .leftOption: return 58
        case .rightOption: return 61
        case .leftCommand: return 55
        case .rightCommand: return 54
        }
    }

    /// Display name for UI
    var displayName: String {
        switch self {
        case .leftShift: return L("modifier.left_shift")
        case .rightShift: return L("modifier.right_shift")
        case .leftControl: return L("modifier.left_control")
        case .rightControl: return L("modifier.right_control")
        case .leftOption: return L("modifier.left_option")
        case .rightOption: return L("modifier.right_option")
        case .leftCommand: return L("modifier.left_command")
        case .rightCommand: return L("modifier.right_command")
        }
    }

    /// Keyboard symbol for display
    var symbol: String {
        switch self {
        case .leftShift, .rightShift: return "⇧"
        case .leftControl, .rightControl: return "⌃"
        case .leftOption, .rightOption: return "⌥"
        case .leftCommand, .rightCommand: return "⌘"
        }
    }

    /// Short display name with symbol
    var shortDisplayName: String {
        let side: String
        switch self {
        case .leftShift, .leftControl, .leftOption, .leftCommand:
            side = L("modifier.side.left")
        case .rightShift, .rightControl, .rightOption, .rightCommand:
            side = L("modifier.side.right")
        }
        return "\(side) \(symbol)"
    }

    /// Config string format: "DoubleTap+{modifierKey}"
    var configString: String {
        "DoubleTap+\(rawValue)"
    }

    /// Parse from config string
    /// Format: "DoubleTap+leftShift"
    static func from(configString: String) -> ModifierKey? {
        let components = configString.split(separator: "+")
        guard components.count == 2,
              components[0].lowercased() == "doubletap"
        else {
            return nil
        }
        return ModifierKey(rawValue: String(components[1]))
    }

    /// Get ModifierKey from keyCode
    static func from(keyCode: UInt16) -> ModifierKey? {
        allCases.first { $0.keyCode == keyCode }
    }

    /// Check if this is a Shift key (left or right)
    var isShift: Bool {
        self == .leftShift || self == .rightShift
    }
}

// MARK: - HotkeyAction

/// Actions that can be triggered by hotkeys
enum HotkeyAction: String, CaseIterable, Codable {
    case replace = "replace"  // Replace mode: AI response replaces original text
    case append = "append"    // Append mode: AI response appends after original text

    var displayName: String {
        switch self {
        case .replace: return L("action.replace")
        case .append: return L("action.append")
        }
    }

    var description: String {
        switch self {
        case .replace: return L("action.replace.description")
        case .append: return L("action.append.description")
        }
    }

    var iconName: String {
        switch self {
        case .replace: return "arrow.left.arrow.right"
        case .append: return "text.append"
        }
    }
}

// MARK: - TriggerConfig Extension

/// Extension to convert between UniFFI TriggerConfig and Swift types
extension TriggerConfig {
    /// Get replace hotkey as ModifierKey
    var replaceKey: ModifierKey {
        ModifierKey.from(configString: replaceHotkey) ?? .leftShift
    }

    /// Get append hotkey as ModifierKey
    var appendKey: ModifierKey {
        ModifierKey.from(configString: appendHotkey) ?? .rightShift
    }

    /// Create TriggerConfig from Swift types
    static func create(
        replaceKey: ModifierKey = .leftShift,
        appendKey: ModifierKey = .rightShift
    ) -> TriggerConfig {
        TriggerConfig(
            replaceHotkey: replaceKey.configString,
            appendHotkey: appendKey.configString
        )
    }

    /// Default configuration
    static var defaultConfig: TriggerConfig {
        create(replaceKey: .leftShift, appendKey: .rightShift)
    }
}
