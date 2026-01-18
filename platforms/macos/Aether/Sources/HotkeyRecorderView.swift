//
//  HotkeyRecorderView.swift
//  Aether
//
//  Visual hotkey recorder component for capturing global keyboard shortcuts.
//

import SwiftUI
import AppKit

/// Represents a keyboard shortcut with modifiers and key
struct Hotkey: Equatable {
    var modifiers: NSEvent.ModifierFlags
    var keyCode: UInt16
    var character: String

    init(modifiers: NSEvent.ModifierFlags, keyCode: UInt16, character: String) {
        // Normalize modifiers by removing non-user-facing flags
        self.modifiers = modifiers.intersection([.command, .option, .shift, .control])
        self.keyCode = keyCode
        self.character = character
    }

    /// Human-readable representation (e.g., "⌘ + Shift + A")
    var displayString: String {
        var parts: [String] = []

        if modifiers.contains(.control) {
            parts.append("⌃")
        }
        if modifiers.contains(.option) {
            parts.append("⌥")
        }
        if modifiers.contains(.shift) {
            parts.append("⇧")
        }
        if modifiers.contains(.command) {
            parts.append("⌘")
        }

        parts.append(character.uppercased())

        return parts.joined(separator: " + ")
    }

    /// Convert to config format (e.g., "Command+Shift+A")
    var configString: String {
        var parts: [String] = []

        if modifiers.contains(.control) {
            parts.append("Control")
        }
        if modifiers.contains(.option) {
            parts.append("Option")
        }
        if modifiers.contains(.shift) {
            parts.append("Shift")
        }
        if modifiers.contains(.command) {
            parts.append("Command")
        }

        parts.append(character.uppercased())

        return parts.joined(separator: "+")
    }

    /// Parse from config format (e.g., "Command+Grave" -> Hotkey)
    static func from(configString: String) -> Hotkey? {
        let components = configString.split(separator: "+").map { $0.trimmingCharacters(in: .whitespaces) }

        guard !components.isEmpty else { return nil }

        var modifiers: NSEvent.ModifierFlags = []
        var keyChar = ""

        for component in components {
            switch component.lowercased() {
            case "command", "cmd":
                modifiers.insert(.command)
            case "option", "opt", "alt":
                modifiers.insert(.option)
            case "shift":
                modifiers.insert(.shift)
            case "control", "ctrl":
                modifiers.insert(.control)
            case "grave", "~", "`":
                keyChar = "`"
            default:
                keyChar = component.lowercased()
            }
        }

        guard !keyChar.isEmpty else { return nil }

        // Convert character to key code (simplified mapping)
        let keyCode = keyCodeForCharacter(keyChar)

        return Hotkey(modifiers: modifiers, keyCode: keyCode, character: keyChar)
    }

    private static func keyCodeForCharacter(_ char: String) -> UInt16 {
        // Common key codes (simplified)
        let keyMap: [String: UInt16] = [
            "`": 50, "~": 50,  // Grave/Tilde
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7,
            "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15,
            "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22,
            "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29,
            "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37,
            "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44,
            "n": 45, "m": 46, ".": 47, " ": 49, "space": 49
        ]

        return keyMap[char.lowercased()] ?? 0
    }
}

/// Hotkey recorder view with visual feedback
struct HotkeyRecorderView: View {
    @Binding var hotkey: Hotkey?
    @State private var isRecording = false
    @State private var eventMonitor: Any?

    var onHotkeyChanged: ((Hotkey?) -> Void)?

    var body: some View {
        HStack(spacing: 12) {
            // Display current hotkey or recording prompt
            ZStack {
                RoundedRectangle(cornerRadius: 8)
                    .fill(isRecording ? Color.blue.opacity(0.2) : Color.gray.opacity(0.2))
                    .frame(height: 36)

                if isRecording {
                    Text("Press key combination...")
                        .foregroundColor(.secondary)
                        .font(.callout)
                } else if let hotkey = hotkey {
                    Text(hotkey.displayString)
                        .font(.system(.body, design: .monospaced))
                        .foregroundColor(.primary)
                } else {
                    Text("No hotkey set")
                        .foregroundColor(.secondary)
                        .font(.callout)
                }
            }
            .frame(minWidth: 200)

            // Record button
            Button(isRecording ? "Recording..." : "Record") {
                if isRecording {
                    stopRecording()
                } else {
                    startRecording()
                }
            }
            .buttonStyle(.borderedProminent)
            .disabled(isRecording && hotkey == nil)

            // Clear button
            Button("Clear") {
                hotkey = nil
                onHotkeyChanged?(nil)
            }
            .disabled(hotkey == nil)
        }
    }

    private func startRecording() {
        isRecording = true

        // Add local event monitor to capture key events
        eventMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .flagsChanged]) { event in
            handleKeyEvent(event)
            return nil // Consume the event
        }
    }

    private func stopRecording() {
        isRecording = false

        if let monitor = eventMonitor {
            NSEvent.removeMonitor(monitor)
            eventMonitor = nil
        }
    }

    private func handleKeyEvent(_ event: NSEvent) {
        // Ignore if just modifier keys are pressed
        if event.type == .flagsChanged {
            return
        }

        // Get the character from the event
        guard let characters = event.charactersIgnoringModifiers,
              !characters.isEmpty else {
            return
        }

        let char = characters.first!
        let newHotkey = Hotkey(
            modifiers: event.modifierFlags,
            keyCode: event.keyCode,
            character: String(char)
        )

        // Require at least one modifier key for manual recording
        // (Default single-key ` is set via "Reset to Default" button only)
        if newHotkey.modifiers.isEmpty {
            // Show visual feedback that modifier is required
            NSSound.beep()
            return
        }

        hotkey = newHotkey
        onHotkeyChanged?(newHotkey)
        stopRecording()
    }
}

/// Preset shortcuts library
struct PresetShortcut: Identifiable {
    let id = UUID()
    let name: String
    let hotkey: Hotkey
    let description: String
}

extension PresetShortcut {
    static let presets: [PresetShortcut] = [
        PresetShortcut(
            name: "Command + Grave",
            hotkey: Hotkey(modifiers: .command, keyCode: 50, character: "`"),
            description: "⌘ + ` (Safe, no typing conflicts)"
        ),
        PresetShortcut(
            name: "Command + Shift + A",
            hotkey: Hotkey(modifiers: [.command, .shift], keyCode: 0, character: "A"),
            description: "Popular AI assistant shortcut"
        ),
        PresetShortcut(
            name: "Option + Space",
            hotkey: Hotkey(modifiers: .option, keyCode: 49, character: "Space"),
            description: "Alfred-style shortcut"
        ),
        PresetShortcut(
            name: "Command + Shift + Space",
            hotkey: Hotkey(modifiers: [.command, .shift], keyCode: 49, character: "Space"),
            description: "Extended modifier combo"
        ),
        PresetShortcut(
            name: "Command + Option + Space",
            hotkey: Hotkey(modifiers: [.command, .option], keyCode: 49, character: "Space"),
            description: "Power user combo"
        )
    ]
}

/// Conflict detection helper
struct HotkeyConflictDetector {
    /// Known macOS system shortcuts that might conflict
    static let systemShortcuts: [Hotkey] = [
        // Spotlight
        Hotkey(modifiers: .command, keyCode: 49, character: "Space"),
        // Mission Control
        Hotkey(modifiers: .control, keyCode: 126, character: "↑"),
        // Application Windows
        Hotkey(modifiers: .control, keyCode: 125, character: "↓"),
        // Show Desktop
        Hotkey(modifiers: [.command, .option], keyCode: 126, character: "↑"),
    ]

    /// Check if hotkey conflicts with known system shortcuts
    /// - Parameters:
    ///   - hotkey: The hotkey to check
    ///   - isDefault: Whether this is the default hotkey (single ` key)
    static func detectConflict(for hotkey: Hotkey, isDefault: Bool = false) -> String? {
        // Allow default single-key ` without warning
        if hotkey.modifiers.isEmpty && hotkey.character == "`" && isDefault {
            return nil
        }

        // Skip warning for other single-key shortcuts (they are blocked in UI)
        if hotkey.modifiers.isEmpty {
            return nil
        }

        for systemHotkey in systemShortcuts {
            if systemHotkey == hotkey {
                return "This hotkey conflicts with a macOS system shortcut: \(systemHotkey.displayString)"
            }
        }

        // Check if it's a common app shortcut
        if hotkey.modifiers == .command {
            let reservedChars = ["c", "v", "x", "z", "a", "s", "w", "q", "n", "t", "o", "p"]
            if reservedChars.contains(hotkey.character.lowercased()) {
                return "This hotkey is commonly used by applications (e.g., ⌘\(hotkey.character.uppercased()) for standard actions)"
            }
        }

        return nil
    }
}

// MARK: - Preview

@available(macOS 14.0, *)
#Preview("Hotkey Recorder") {
    @Previewable @State var hotkey: Hotkey? = Hotkey(modifiers: .command, keyCode: 50, character: "`")

    VStack(spacing: 20) {
        Text("Hotkey Recorder Demo")
            .font(.title2)

        HotkeyRecorderView(hotkey: $hotkey) { newHotkey in
            print("Hotkey changed: \(newHotkey?.displayString ?? "None")")
        }

        if let hotkey = hotkey {
            VStack(alignment: .leading, spacing: 8) {
                Text("Current Hotkey:")
                    .font(.headline)
                Text("Display: \(hotkey.displayString)")
                    .font(.caption)
                Text("Config: \(hotkey.configString)")
                    .font(.caption)

                if let conflict = HotkeyConflictDetector.detectConflict(for: hotkey, isDefault: hotkey.modifiers.isEmpty && hotkey.character == "`") {
                    Label(conflict, systemImage: "exclamationmark.triangle")
                        .foregroundColor(.orange)
                        .font(.caption)
                }
            }
            .padding()
            .background(Color.gray.opacity(0.1))
            .cornerRadius(8)
        }

        Divider()

        Text("Preset Shortcuts")
            .font(.headline)

        ForEach(PresetShortcut.presets) { preset in
            HStack {
                Text(preset.name)
                    .font(.body)
                Spacer()
                Text(preset.description)
                    .font(.caption)
                    .foregroundColor(.secondary)
                Button("Apply") {
                    hotkey = preset.hotkey
                }
                .buttonStyle(.bordered)
            }
        }
    }
    .padding()
    .frame(width: 600)
}
