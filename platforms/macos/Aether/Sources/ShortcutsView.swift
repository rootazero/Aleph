//
//  ShortcutsView.swift
//  Aether
//
//  Keyboard shortcuts configuration tab for command completion and OCR capture.
//

import SwiftUI
import AppKit

struct ShortcutsView: View {
    // Dependencies
    let core: AetherCore?
    @Binding var hasUnsavedChanges: Bool

    // Command completion hotkey (one modifier + character, configurable to two)
    @State private var commandModifier1: CommandModifier = .option
    @State private var commandModifier2: CommandModifier? = nil  // Optional second modifier
    @State private var commandCharKey: CommandCharKey = .space

    // OCR capture hotkey (two modifiers + character)
    @State private var ocrModifier1: CommandModifier = .command
    @State private var ocrModifier2: CommandModifier = .option
    @State private var ocrCharKey: OcrCharKey = .o

    // Saved settings (for comparison)
    @State private var savedCommandModifier1: CommandModifier = .option
    @State private var savedCommandModifier2: CommandModifier? = nil
    @State private var savedCommandCharKey: CommandCharKey = .space
    @State private var savedOcrModifier1: CommandModifier = .command
    @State private var savedOcrModifier2: CommandModifier = .option
    @State private var savedOcrCharKey: OcrCharKey = .o

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    // Default values
    private let defaultCommandModifier1: CommandModifier = .option
    private let defaultCommandModifier2: CommandModifier? = nil
    private let defaultCommandCharKey: CommandCharKey = .space
    private let defaultOcrModifier1: CommandModifier = .command
    private let defaultOcrModifier2: CommandModifier = .option
    private let defaultOcrCharKey: OcrCharKey = .o

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    // Command Completion Hotkey Card
                    commandCompletionCard

                    // OCR Capture Hotkey Card
                    ocrCaptureCard

                    // Permission Card
                    permissionCard
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(DesignTokens.Spacing.lg)
            }
            .scrollEdge(edges: [.top, .bottom], style: .hard())
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            UnifiedSaveBar(
                hasUnsavedChanges: hasLocalUnsavedChanges,
                isSaving: isSaving,
                statusMessage: errorMessage,
                onSave: { await saveSettings() },
                onCancel: { cancelEditing() }
            )
        }
        .onAppear {
            loadSettings()
            syncUnsavedChanges()
        }
        .onChange(of: commandModifier1) { _, _ in syncUnsavedChanges() }
        .onChange(of: commandCharKey) { _, _ in syncUnsavedChanges() }
        .onChange(of: ocrModifier1) { _, newValue in
            // Ensure modifier2 is different from modifier1
            if ocrModifier2 == newValue {
                ocrModifier2 = CommandModifier.allCases.first { $0 != newValue } ?? .option
            }
            syncUnsavedChanges()
        }
        .onChange(of: ocrModifier2) { _, newValue in
            // Ensure modifier1 is different from modifier2
            if ocrModifier1 == newValue {
                ocrModifier1 = CommandModifier.allCases.first { $0 != newValue } ?? .command
            }
            syncUnsavedChanges()
        }
        .onChange(of: ocrCharKey) { _, _ in syncUnsavedChanges() }
        .onChange(of: isSaving) { _, _ in syncUnsavedChanges() }
    }

    // MARK: - View Components

    /// Command completion hotkey card (customizable)
    private var commandCompletionCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.general.command_completion"), systemImage: "command")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                Text(L("settings.shortcuts.command_completion_description"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                // Command completion hotkey row
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    HStack {
                        Image(systemName: "terminal")
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .frame(width: 20)

                        VStack(alignment: .leading, spacing: 2) {
                            Text(L("settings.shortcuts.command_completion_title"))
                                .font(DesignTokens.Typography.body)
                            Text(L("settings.shortcuts.command_completion_hint"))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }

                        Spacer()

                        // Show current hotkey
                        Text(commandPromptDisplayString)
                            .font(DesignTokens.Typography.code)
                            .foregroundColor(DesignTokens.Colors.accentBlue)
                            .padding(.horizontal, DesignTokens.Spacing.sm)
                            .padding(.vertical, DesignTokens.Spacing.xs)
                            .background(DesignTokens.Colors.accentBlue.opacity(0.1))
                            .cornerRadius(DesignTokens.CornerRadius.small)
                    }

                    // Hotkey pickers (single modifier + character)
                    HStack(spacing: DesignTokens.Spacing.sm) {
                        // Modifier key
                        Picker("", selection: $commandModifier1) {
                            ForEach(CommandModifier.allCases, id: \.self) { modifier in
                                Text(modifier.displayName).tag(modifier)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .frame(width: 100)

                        Text("+")
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        // Character key
                        Picker("", selection: $commandCharKey) {
                            ForEach(CommandCharKey.allCases, id: \.self) { key in
                                Text(key.displayName).tag(key)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .frame(width: 80)

                        Spacer()

                        // Reset to default button
                        if !isCommandPromptDefault {
                            Button {
                                commandModifier1 = defaultCommandModifier1
                                commandModifier2 = defaultCommandModifier2
                                commandCharKey = defaultCommandCharKey
                            } label: {
                                HStack(spacing: 4) {
                                    Image(systemName: "arrow.counterclockwise")
                                    Text(L("common.reset"))
                                }
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .padding(DesignTokens.Spacing.sm)
                .background(DesignTokens.Colors.border.opacity(0.3))
                .cornerRadius(DesignTokens.CornerRadius.small)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    /// OCR capture hotkey card (screen capture OCR)
    private var ocrCaptureCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.shortcuts.ocr_capture"), systemImage: "text.viewfinder")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                Text(L("settings.shortcuts.ocr_capture_description"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                // OCR capture hotkey row
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    HStack {
                        Image(systemName: "camera.viewfinder")
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .frame(width: 20)

                        VStack(alignment: .leading, spacing: 2) {
                            Text(L("settings.shortcuts.ocr_capture_title"))
                                .font(DesignTokens.Typography.body)
                            Text(L("settings.shortcuts.ocr_capture_hint"))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }

                        Spacer()

                        // Show current hotkey
                        Text(ocrCaptureDisplayString)
                            .font(DesignTokens.Typography.code)
                            .foregroundColor(DesignTokens.Colors.accentBlue)
                            .padding(.horizontal, DesignTokens.Spacing.sm)
                            .padding(.vertical, DesignTokens.Spacing.xs)
                            .background(DesignTokens.Colors.accentBlue.opacity(0.1))
                            .cornerRadius(DesignTokens.CornerRadius.small)
                    }

                    // Hotkey pickers (two modifiers + key)
                    HStack(spacing: DesignTokens.Spacing.sm) {
                        // First modifier
                        Picker("", selection: $ocrModifier1) {
                            ForEach(CommandModifier.allCases, id: \.self) { modifier in
                                Text(modifier.displayName).tag(modifier)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .frame(width: 100)

                        Text("+")
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        // Second modifier (filtered to exclude first)
                        Picker("", selection: $ocrModifier2) {
                            ForEach(CommandModifier.allCases.filter { $0 != ocrModifier1 }, id: \.self) { modifier in
                                Text(modifier.displayName).tag(modifier)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .frame(width: 100)

                        Text("+")
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        // Character key
                        Picker("", selection: $ocrCharKey) {
                            ForEach(OcrCharKey.allCases, id: \.self) { key in
                                Text(key.displayName).tag(key)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .frame(width: 80)

                        Spacer()

                        // Reset to default button
                        if !isOcrCaptureDefault {
                            Button {
                                ocrModifier1 = defaultOcrModifier1
                                ocrModifier2 = defaultOcrModifier2
                                ocrCharKey = defaultOcrCharKey
                            } label: {
                                HStack(spacing: 4) {
                                    Image(systemName: "arrow.counterclockwise")
                                    Text(L("common.reset"))
                                }
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .padding(DesignTokens.Spacing.sm)
                .background(DesignTokens.Colors.border.opacity(0.3))
                .cornerRadius(DesignTokens.CornerRadius.small)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    /// Permission card
    private var permissionCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.shortcuts.permission_required"), systemImage: "lock.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                Text(L("settings.shortcuts.permission_description"))
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Text(L("settings.shortcuts.why_needed"))
                    .font(DesignTokens.Typography.caption)
                    .fontWeight(.semibold)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Label(L("settings.shortcuts.permission_detect"), systemImage: "checkmark.circle")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Label(L("settings.shortcuts.permission_read"), systemImage: "checkmark.circle")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Label(L("settings.shortcuts.permission_simulate"), systemImage: "checkmark.circle")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                ActionButton(L("settings.shortcuts.open_settings_button"), icon: "gear", style: .primary) {
                    openAccessibilitySettings()
                }
                .padding(.top, DesignTokens.Spacing.sm)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    // MARK: - Computed Properties

    /// Display string for command prompt hotkey (e.g., "⌥ ␣" or "⌥ ⌘ ␣")
    private var commandPromptDisplayString: String {
        if let mod2 = commandModifier2 {
            return "\(commandModifier1.symbol) \(mod2.symbol) \(commandCharKey.displayChar)"
        } else {
            return "\(commandModifier1.symbol) \(commandCharKey.displayChar)"
        }
    }

    /// Config string for command prompt (e.g., "Option+Space" or "Option+Command+Space")
    private var commandPromptConfigString: String {
        if let mod2 = commandModifier2 {
            return "\(commandModifier1.rawValue)+\(mod2.rawValue)+\(commandCharKey.rawValue)"
        } else {
            return "\(commandModifier1.rawValue)+\(commandCharKey.rawValue)"
        }
    }

    /// Check if command prompt is at default value
    private var isCommandPromptDefault: Bool {
        commandModifier1 == defaultCommandModifier1 &&
        commandModifier2 == defaultCommandModifier2 &&
        commandCharKey == defaultCommandCharKey
    }

    /// Display string for OCR capture hotkey (e.g., "⌘ ⌥ O")
    private var ocrCaptureDisplayString: String {
        "\(ocrModifier1.symbol) \(ocrModifier2.symbol) \(ocrCharKey.displayChar)"
    }

    /// Config string for OCR capture (e.g., "Command+Option+O")
    private var ocrCaptureConfigString: String {
        "\(ocrModifier1.rawValue)+\(ocrModifier2.rawValue)+\(ocrCharKey.rawValue)"
    }

    /// Check if OCR capture is at default value
    private var isOcrCaptureDefault: Bool {
        ocrModifier1 == defaultOcrModifier1 &&
        ocrModifier2 == defaultOcrModifier2 &&
        ocrCharKey == defaultOcrCharKey
    }

    /// Check if current state differs from saved state
    private var hasLocalUnsavedChanges: Bool {
        return commandModifier1 != savedCommandModifier1 ||
               commandModifier2 != savedCommandModifier2 ||
               commandCharKey != savedCommandCharKey ||
               ocrModifier1 != savedOcrModifier1 ||
               ocrModifier2 != savedOcrModifier2 ||
               ocrCharKey != savedOcrCharKey
    }

    /// Status message for UnifiedSaveBar
    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasLocalUnsavedChanges {
            return L("settings.unsaved_changes.title")
        }
        return nil
    }

    // MARK: - Actions

    private func loadSettings() {
        guard let core = core else {
            // Use defaults if core is not available
            return
        }

        Task {
            do {
                let config = try core.loadConfig()

                await MainActor.run {
                    // Load shortcuts config (Command completion + OCR capture)
                    if let shortcuts = config.shortcuts {
                        parseCommandPrompt(shortcuts.commandPrompt)
                        savedCommandModifier1 = commandModifier1
                        savedCommandModifier2 = commandModifier2
                        savedCommandCharKey = commandCharKey

                        parseOcrCapture(shortcuts.ocrCapture)
                        savedOcrModifier1 = ocrModifier1
                        savedOcrModifier2 = ocrModifier2
                        savedOcrCharKey = ocrCharKey
                    }
                }
            } catch {
                print("Failed to load shortcut settings: \(error)")
            }
        }
    }

    /// Parse command prompt string (e.g., "Option+Space" or "Option+Command+Space") into components
    private func parseCommandPrompt(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }

        if parts.count == 2 {
            // Single modifier + key (e.g., "Option+Space")
            if let mod1 = CommandModifier(rawValue: parts[0]) {
                commandModifier1 = mod1
            }
            commandModifier2 = nil
            if let key = CommandCharKey(rawValue: parts[1]) {
                commandCharKey = key
            }
        } else if parts.count == 3 {
            // Two modifiers + key (e.g., "Option+Command+Space")
            if let mod1 = CommandModifier(rawValue: parts[0]) {
                commandModifier1 = mod1
            }
            if let mod2 = CommandModifier(rawValue: parts[1]) {
                commandModifier2 = mod2
            }
            if let key = CommandCharKey(rawValue: parts[2]) {
                commandCharKey = key
            }
        }
    }

    /// Parse OCR capture string (e.g., "Command+Option+O") into components
    private func parseOcrCapture(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count == 3 else { return }

        if let mod1 = CommandModifier(rawValue: parts[0]) {
            ocrModifier1 = mod1
        }
        if let mod2 = CommandModifier(rawValue: parts[1]) {
            ocrModifier2 = mod2
        }
        if let key = OcrCharKey(rawValue: parts[2]) {
            ocrCharKey = key
        }
    }

    private func saveSettings() async {
        guard let core = core else {
            await MainActor.run {
                errorMessage = L("error.core_not_initialized")
            }
            return
        }

        await MainActor.run {
            isSaving = true
            errorMessage = nil
        }

        do {
            // Save shortcuts config (Command completion + OCR capture)
            let shortcutsConfig = ShortcutsConfig(
                summon: "Command+Grave",  // Legacy, not used
                cancel: "Escape",
                commandPrompt: commandPromptConfigString,
                ocrCapture: ocrCaptureConfigString
            )
            try core.updateShortcuts(shortcuts: shortcutsConfig)

            print("Shortcut settings saved successfully:")
            print("  Command Prompt: \(commandPromptConfigString)")
            print("  OCR Capture: \(ocrCaptureConfigString)")

            await MainActor.run {
                // Update saved state to match current state
                savedCommandModifier1 = commandModifier1
                savedCommandModifier2 = commandModifier2
                savedCommandCharKey = commandCharKey
                savedOcrModifier1 = ocrModifier1
                savedOcrModifier2 = ocrModifier2
                savedOcrCharKey = ocrCharKey

                isSaving = false
                errorMessage = nil

                // Notify AppDelegate to update hotkeys at runtime
                if let appDelegate = NSApp.delegate as? AppDelegate {
                    appDelegate.updateCommandPromptHotkey(shortcutsConfig)
                    appDelegate.updateOcrCaptureHotkey(shortcutsConfig)
                }

                // Post notification for other components
                NotificationCenter.default.post(
                    name: .aetherConfigSavedInternally,
                    object: nil
                )
            }
        } catch {
            print("Failed to save shortcut settings: \(error)")
            await MainActor.run {
                errorMessage = "Failed to save: \(error.localizedDescription)"
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        commandModifier1 = savedCommandModifier1
        commandModifier2 = savedCommandModifier2
        commandCharKey = savedCommandCharKey
        ocrModifier1 = savedOcrModifier1
        ocrModifier2 = savedOcrModifier2
        ocrCharKey = savedOcrCharKey
        errorMessage = nil
    }

    /// Sync local unsaved changes state to parent binding
    private func syncUnsavedChanges() {
        hasUnsavedChanges = hasLocalUnsavedChanges
    }

    private func openAccessibilitySettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }
}

// MARK: - Command Modifier Enum

/// Modifier keys for command completion hotkey
enum CommandModifier: String, CaseIterable {
    case command = "Command"
    case option = "Option"
    case control = "Control"
    case shift = "Shift"

    var displayName: String {
        switch self {
        case .command: return "Command"
        case .option: return "Option"
        case .control: return "Control"
        case .shift: return "Shift"
        }
    }

    var symbol: String {
        switch self {
        case .command: return "⌘"
        case .option: return "⌥"
        case .control: return "⌃"
        case .shift: return "⇧"
        }
    }

    var eventModifier: NSEvent.ModifierFlags {
        switch self {
        case .command: return .command
        case .option: return .option
        case .control: return .control
        case .shift: return .shift
        }
    }
}

// MARK: - Command Character Key Enum

/// Character keys for command completion hotkey
enum CommandCharKey: String, CaseIterable {
    case slash = "/"
    case grave = "`"
    case backslash = "\\"
    case semicolon = ";"
    case comma = ","
    case period = "."
    case space = "Space"
    case esc = "Esc"

    var displayName: String {
        switch self {
        case .slash: return "/"
        case .grave: return "`"
        case .backslash: return "\\"
        case .semicolon: return ";"
        case .comma: return ","
        case .period: return "."
        case .space: return "Space"
        case .esc: return "Esc"
        }
    }

    var displayChar: String {
        switch self {
        case .space: return "␣"
        case .esc: return "⎋"
        default: return rawValue
        }
    }

    var keyCode: UInt16 {
        switch self {
        case .slash: return 44
        case .grave: return 50
        case .backslash: return 42
        case .semicolon: return 41
        case .comma: return 43
        case .period: return 47
        case .space: return 49
        case .esc: return 53
        }
    }
}

// MARK: - OCR Character Key Enum

/// Character keys for OCR capture hotkey (includes letters and numbers)
enum OcrCharKey: String, CaseIterable {
    // Letters (for semantic hotkeys like O for OCR)
    case o = "O"
    case s = "S"
    // Numbers
    case zero = "0"
    case one = "1"
    case two = "2"
    case three = "3"
    case four = "4"
    case five = "5"
    case six = "6"
    case seven = "7"
    case eight = "8"
    case nine = "9"
    // Common symbols
    case slash = "/"
    case grave = "`"

    var displayName: String {
        rawValue
    }

    var displayChar: String {
        rawValue
    }
}

// MARK: - Preview

struct ShortcutsView_Previews: PreviewProvider {
    static var previews: some View {
        ShortcutsView(core: nil, hasUnsavedChanges: .constant(false))
    }
}
