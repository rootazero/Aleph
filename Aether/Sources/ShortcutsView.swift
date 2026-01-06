//
//  ShortcutsView.swift
//  Aether
//
//  Keyboard shortcuts configuration tab for trigger hotkeys and command completion.
//  Supports Replace/Append hotkeys (double-tap modifier) and Command Completion (modifier combo).
//

import SwiftUI
import AppKit

struct ShortcutsView: View {
    // Dependencies
    let core: AetherCore?
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Trigger hotkeys (double-tap modifier keys)
    @State private var replaceKey: ModifierKey = .leftShift
    @State private var appendKey: ModifierKey = .rightShift

    // Command completion hotkey (two modifiers + character)
    @State private var commandModifier1: CommandModifier = .command
    @State private var commandModifier2: CommandModifier = .option
    @State private var commandCharKey: CommandCharKey = .slash

    // Saved settings (for comparison)
    @State private var savedReplaceKey: ModifierKey = .leftShift
    @State private var savedAppendKey: ModifierKey = .rightShift
    @State private var savedCommandModifier1: CommandModifier = .command
    @State private var savedCommandModifier2: CommandModifier = .option
    @State private var savedCommandCharKey: CommandCharKey = .slash

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    // Default values
    private let defaultReplaceKey: ModifierKey = .leftShift
    private let defaultAppendKey: ModifierKey = .rightShift
    private let defaultCommandModifier1: CommandModifier = .command
    private let defaultCommandModifier2: CommandModifier = .option
    private let defaultCommandCharKey: CommandCharKey = .slash

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Trigger Hotkeys Card (Replace/Append)
                triggerHotkeyCard

                // Command Completion Hotkey Card
                commandCompletionCard

                // Permission Card
                permissionCard
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadSettings()
            updateSaveBarState()
        }
        .onChange(of: replaceKey) { _, _ in updateSaveBarState() }
        .onChange(of: appendKey) { _, _ in updateSaveBarState() }
        .onChange(of: commandModifier1) { _, newValue in
            // Ensure modifier2 is different from modifier1
            if commandModifier2 == newValue {
                commandModifier2 = CommandModifier.allCases.first { $0 != newValue } ?? .option
            }
            updateSaveBarState()
        }
        .onChange(of: commandModifier2) { _, newValue in
            // Ensure modifier1 is different from modifier2
            if commandModifier1 == newValue {
                commandModifier1 = CommandModifier.allCases.first { $0 != newValue } ?? .command
            }
            updateSaveBarState()
        }
        .onChange(of: commandCharKey) { _, _ in updateSaveBarState() }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
    }

    // MARK: - View Components

    /// Trigger hotkey card (Replace/Append hotkeys configuration)
    private var triggerHotkeyCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.trigger.title"), systemImage: "keyboard")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                Text(L("settings.trigger.description"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                // Replace hotkey picker
                hotkeyPicker(
                    action: .replace,
                    selection: $replaceKey,
                    otherKey: appendKey,
                    defaultKey: defaultReplaceKey
                )

                // Append hotkey picker
                hotkeyPicker(
                    action: .append,
                    selection: $appendKey,
                    otherKey: replaceKey,
                    defaultKey: defaultAppendKey
                )
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    /// Hotkey picker row for Replace/Append configuration
    @ViewBuilder
    private func hotkeyPicker(
        action: HotkeyAction,
        selection: Binding<ModifierKey>,
        otherKey: ModifierKey,
        defaultKey: ModifierKey
    ) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            HStack {
                Image(systemName: action.iconName)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .frame(width: 20)
                VStack(alignment: .leading, spacing: 2) {
                    Text(action.displayName)
                        .font(DesignTokens.Typography.body)
                    Text(action.description)
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                Spacer()

                // Show current hotkey display
                Text(selection.wrappedValue.shortDisplayName)
                    .font(DesignTokens.Typography.code)
                    .foregroundColor(DesignTokens.Colors.accentBlue)
                    .padding(.horizontal, DesignTokens.Spacing.sm)
                    .padding(.vertical, DesignTokens.Spacing.xs)
                    .background(DesignTokens.Colors.accentBlue.opacity(0.1))
                    .cornerRadius(DesignTokens.CornerRadius.small)
            }

            HStack(spacing: DesignTokens.Spacing.sm) {
                // Modifier key picker (grouped by type)
                Picker("", selection: selection) {
                    // Shift group
                    Text(ModifierKey.leftShift.displayName).tag(ModifierKey.leftShift)
                    Text(ModifierKey.rightShift.displayName).tag(ModifierKey.rightShift)

                    Divider()

                    // Control group
                    Text(ModifierKey.leftControl.displayName).tag(ModifierKey.leftControl)
                    Text(ModifierKey.rightControl.displayName).tag(ModifierKey.rightControl)

                    Divider()

                    // Option group
                    Text(ModifierKey.leftOption.displayName).tag(ModifierKey.leftOption)
                    Text(ModifierKey.rightOption.displayName).tag(ModifierKey.rightOption)

                    Divider()

                    // Command group
                    Text(ModifierKey.leftCommand.displayName).tag(ModifierKey.leftCommand)
                    Text(ModifierKey.rightCommand.displayName).tag(ModifierKey.rightCommand)
                }
                .pickerStyle(.menu)
                .labelsHidden()

                // Reset to default button
                if selection.wrappedValue != defaultKey {
                    Button {
                        selection.wrappedValue = defaultKey
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

            // Warning if same key as other action
            if selection.wrappedValue == otherKey {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundColor(DesignTokens.Colors.warning)
                    Text(L("settings.trigger.same_key_warning"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.warning)
                }
            }
        }
        .padding(DesignTokens.Spacing.sm)
        .background(DesignTokens.Colors.border.opacity(0.3))
        .cornerRadius(DesignTokens.CornerRadius.small)
    }

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

                    // Hotkey pickers
                    HStack(spacing: DesignTokens.Spacing.sm) {
                        // First modifier
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

                        // Second modifier (filtered to exclude first)
                        Picker("", selection: $commandModifier2) {
                            ForEach(CommandModifier.allCases.filter { $0 != commandModifier1 }, id: \.self) { modifier in
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

    /// Display string for command prompt hotkey (e.g., "⌘ ⌥ /")
    private var commandPromptDisplayString: String {
        "\(commandModifier1.symbol) \(commandModifier2.symbol) \(commandCharKey.displayChar)"
    }

    /// Config string for command prompt (e.g., "Command+Option+/")
    private var commandPromptConfigString: String {
        "\(commandModifier1.rawValue)+\(commandModifier2.rawValue)+\(commandCharKey.rawValue)"
    }

    /// Check if command prompt is at default value
    private var isCommandPromptDefault: Bool {
        commandModifier1 == defaultCommandModifier1 &&
        commandModifier2 == defaultCommandModifier2 &&
        commandCharKey == defaultCommandCharKey
    }

    /// Check if current state differs from saved state
    private var hasUnsavedChanges: Bool {
        return replaceKey != savedReplaceKey ||
               appendKey != savedAppendKey ||
               commandModifier1 != savedCommandModifier1 ||
               commandModifier2 != savedCommandModifier2 ||
               commandCharKey != savedCommandCharKey
    }

    /// Status message for UnifiedSaveBar
    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasUnsavedChanges {
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
                    // Load trigger config (Replace/Append hotkeys)
                    if let trigger = config.trigger {
                        replaceKey = trigger.replaceKey
                        savedReplaceKey = replaceKey

                        appendKey = trigger.appendKey
                        savedAppendKey = appendKey
                    }

                    // Load shortcuts config (Command completion)
                    if let shortcuts = config.shortcuts {
                        parseCommandPrompt(shortcuts.commandPrompt)
                        savedCommandModifier1 = commandModifier1
                        savedCommandModifier2 = commandModifier2
                        savedCommandCharKey = commandCharKey
                    }
                }
            } catch {
                print("Failed to load shortcut settings: \(error)")
            }
        }
    }

    /// Parse command prompt string (e.g., "Command+Option+/") into components
    private func parseCommandPrompt(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count == 3 else { return }

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
            // Save trigger config (Replace/Append hotkeys)
            let triggerConfig = TriggerConfig.create(
                replaceKey: replaceKey,
                appendKey: appendKey
            )
            try core.updateTriggerConfig(trigger: triggerConfig)

            // Save shortcuts config (Command completion)
            let shortcutsConfig = ShortcutsConfig(
                summon: "Command+Grave",  // Legacy, not used
                cancel: "Escape",
                commandPrompt: commandPromptConfigString
            )
            try core.updateShortcuts(shortcuts: shortcutsConfig)

            print("Shortcut settings saved successfully:")
            print("  Replace Key: \(replaceKey.rawValue)")
            print("  Append Key: \(appendKey.rawValue)")
            print("  Command Prompt: \(commandPromptConfigString)")

            await MainActor.run {
                // Update saved state to match current state
                savedReplaceKey = replaceKey
                savedAppendKey = appendKey
                savedCommandModifier1 = commandModifier1
                savedCommandModifier2 = commandModifier2
                savedCommandCharKey = commandCharKey

                isSaving = false
                errorMessage = nil

                // Notify AppDelegate to update trigger system at runtime
                if let appDelegate = NSApp.delegate as? AppDelegate {
                    appDelegate.updateTriggerConfiguration(triggerConfig)
                    appDelegate.updateCommandPromptHotkey(shortcutsConfig)
                }

                // Post notification for other components
                NotificationCenter.default.post(
                    name: NSNotification.Name("AetherConfigSavedInternally"),
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
        replaceKey = savedReplaceKey
        appendKey = savedAppendKey
        commandModifier1 = savedCommandModifier1
        commandModifier2 = savedCommandModifier2
        commandCharKey = savedCommandCharKey
        errorMessage = nil
    }

    /// Update saveBarState to reflect current state
    private func updateSaveBarState() {
        saveBarState.update(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: saveSettings,
            onCancel: cancelEditing
        )
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

    var displayName: String {
        switch self {
        case .slash: return "/"
        case .grave: return "`"
        case .backslash: return "\\"
        case .semicolon: return ";"
        case .comma: return ","
        case .period: return "."
        case .space: return "Space"
        }
    }

    var displayChar: String {
        switch self {
        case .space: return "␣"
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
        }
    }
}

// MARK: - Preview

struct ShortcutsView_Previews: PreviewProvider {
    static var previews: some View {
        ShortcutsView(core: nil, saveBarState: SettingsSaveBarState())
    }
}
