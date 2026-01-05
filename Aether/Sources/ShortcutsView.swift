//
//  ShortcutsView.swift
//  Aether
//
//  Keyboard shortcuts configuration tab with hotkey recorder.
//  Supports double-tap Space (default) and custom modifier + key combos.
//

import SwiftUI
import AppKit

struct ShortcutsView: View {
    @ObservedObject var saveBarState: SettingsSaveBarState

    @State private var currentHotkeyMode: HotkeyMode = .default
    @State private var showingCustomHotkeyRecorder = false
    @State private var conflictWarning: String?
    @State private var showingSaveConfirmation = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Global Hotkey Card
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label(L("settings.shortcuts.global_hotkey"), systemImage: "keyboard")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        // Current hotkey display
                        HStack {
                            Text(L("settings.shortcuts.current_hotkey"))
                                .font(DesignTokens.Typography.body)
                                .frame(width: 80, alignment: .leading)

                            ZStack {
                                RoundedRectangle(cornerRadius: 8)
                                    .fill(DesignTokens.Colors.cardBackground)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 8)
                                            .stroke(DesignTokens.Colors.accentBlue.opacity(0.3), lineWidth: 1)
                                    )
                                    .frame(height: 36)

                                Text(currentHotkeyMode.displayString)
                                    .font(.system(.body, design: .monospaced))
                                    .fontWeight(.medium)
                                    .foregroundColor(DesignTokens.Colors.textPrimary)
                            }
                            .frame(minWidth: 200)

                            Spacer()
                        }

                        // Mode description
                        switch currentHotkeyMode {
                        case .doubleTapShift:
                            HStack(spacing: DesignTokens.Spacing.sm) {
                                Image(systemName: "info.circle")
                                    .foregroundColor(DesignTokens.Colors.accentBlue)
                                Text(L("settings.shortcuts.double_tap_shift_description"))
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }
                        case .doubleTap:
                            HStack(spacing: DesignTokens.Spacing.sm) {
                                Image(systemName: "info.circle")
                                    .foregroundColor(DesignTokens.Colors.accentBlue)
                                Text(L("settings.shortcuts.double_tap_description"))
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }
                        case .modifierCombo:
                            HStack(spacing: DesignTokens.Spacing.sm) {
                                Image(systemName: "info.circle")
                                    .foregroundColor(DesignTokens.Colors.accentBlue)
                                Text(L("settings.shortcuts.modifier_combo_description"))
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }
                        }

                        // Conflict warning
                        if let warning = conflictWarning {
                            HStack(spacing: DesignTokens.Spacing.sm) {
                                Image(systemName: "exclamationmark.triangle")
                                    .foregroundColor(DesignTokens.Colors.warning)
                                Text(warning)
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.warning)
                            }
                            .padding(DesignTokens.Spacing.sm)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(DesignTokens.Colors.warning.opacity(0.1))
                            .cornerRadius(DesignTokens.CornerRadius.small)
                        }

                        Divider()
                            .padding(.vertical, DesignTokens.Spacing.sm)

                        // Action buttons
                        HStack(spacing: DesignTokens.Spacing.md) {
                            ActionButton(L("settings.shortcuts.reset_default"), style: .secondary) {
                                resetToDefault()
                            }

                            ActionButton(L("settings.shortcuts.custom_hotkey"), style: .secondary) {
                                showingCustomHotkeyRecorder = true
                            }

                            Spacer()

                            if showingSaveConfirmation {
                                Label(L("settings.shortcuts.saved"), systemImage: "checkmark.circle.fill")
                                    .foregroundColor(DesignTokens.Colors.providerActive)
                                    .font(DesignTokens.Typography.caption)
                            }
                        }
                    }
                }
                .padding(DesignTokens.Spacing.md)
                .background(DesignTokens.Colors.cardBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))

                // Preset Shortcuts Card
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label(L("settings.shortcuts.presets"), systemImage: "star")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(spacing: DesignTokens.Spacing.sm) {
                        PresetHotkeyRow(
                            name: L("settings.shortcuts.preset_double_tap_shift"),
                            mode: .doubleTapShift,
                            description: L("settings.shortcuts.preset_default_description"),
                            isSelected: currentHotkeyMode == .doubleTapShift
                        ) {
                            applyHotkey(.doubleTapShift)
                        }

                        PresetHotkeyRow(
                            name: "⌘ + `",
                            mode: .modifierCombo(keyCode: 50, modifiers: .maskCommand),
                            description: L("settings.shortcuts.preset_command_grave"),
                            isSelected: currentHotkeyMode == .modifierCombo(keyCode: 50, modifiers: .maskCommand)
                        ) {
                            applyHotkey(.modifierCombo(keyCode: 50, modifiers: .maskCommand))
                        }

                        PresetHotkeyRow(
                            name: "⌥ + ␣",
                            mode: .modifierCombo(keyCode: 49, modifiers: .maskAlternate),
                            description: L("settings.shortcuts.preset_option_space"),
                            isSelected: currentHotkeyMode == .modifierCombo(keyCode: 49, modifiers: .maskAlternate)
                        ) {
                            applyHotkey(.modifierCombo(keyCode: 49, modifiers: .maskAlternate))
                        }

                        PresetHotkeyRow(
                            name: "⌃ + ⌘ + A",
                            mode: .modifierCombo(keyCode: 0, modifiers: [.maskControl, .maskCommand]),
                            description: L("settings.shortcuts.preset_control_command_a"),
                            isSelected: currentHotkeyMode == .modifierCombo(keyCode: 0, modifiers: [.maskControl, .maskCommand])
                        ) {
                            applyHotkey(.modifierCombo(keyCode: 0, modifiers: [.maskControl, .maskCommand]))
                        }
                    }
                }
                .padding(DesignTokens.Spacing.md)
                .background(DesignTokens.Colors.cardBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))

                // Permission Card
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
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .sheet(isPresented: $showingCustomHotkeyRecorder) {
            CustomHotkeyRecorderSheet { newMode in
                applyHotkey(newMode)
                showingCustomHotkeyRecorder = false
            }
        }
        .onAppear {
            loadCurrentHotkey()
            // Set save bar to disabled state for instant-save view
            saveBarState.update(
                hasUnsavedChanges: false,
                isSaving: false,
                statusMessage: nil,
                onSave: nil,
                onCancel: nil
            )
        }
    }

    private func loadCurrentHotkey() {
        // Load from config file
        let configPath = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aether/config.toml")

        if FileManager.default.fileExists(atPath: configPath.path) {
            do {
                let content = try String(contentsOf: configPath, encoding: .utf8)
                if let summonLine = content.split(separator: "\n").first(where: { $0.hasPrefix("summon") }) {
                    let value = summonLine
                        .split(separator: "=")
                        .last?
                        .trimmingCharacters(in: .whitespaces)
                        .replacingOccurrences(of: "\"", with: "")
                        ?? ""

                    if let mode = HotkeyMode.from(configString: value) {
                        currentHotkeyMode = mode
                        return
                    }
                }
            } catch {
                print("[ShortcutsView] Failed to read config: \(error)")
            }
        }

        // Default
        currentHotkeyMode = .default
    }

    private func applyHotkey(_ mode: HotkeyMode) {
        currentHotkeyMode = mode
        conflictWarning = detectConflict(for: mode)
        saveHotkey(mode)

        // Update the running hotkey monitor
        if let appDelegate = NSApp.delegate as? AppDelegate {
            appDelegate.updateHotkeyConfiguration(mode)
        }
    }

    private func detectConflict(for mode: HotkeyMode) -> String? {
        // Check for common system hotkey conflicts
        switch mode {
        case .doubleTapShift, .doubleTap:
            return nil // Double-tap is safe

        case .modifierCombo(let keyCode, let modifiers):
            // Check Command+Space (Spotlight)
            if keyCode == 49 && modifiers == .maskCommand {
                return L("settings.shortcuts.conflict_spotlight")
            }
            // Check Option+Space (some input methods)
            if keyCode == 49 && modifiers == .maskAlternate {
                return L("settings.shortcuts.conflict_input_method")
            }
            // Check Control+Space (input method switch)
            if keyCode == 49 && modifiers == .maskControl {
                return L("settings.shortcuts.conflict_input_switch")
            }
            return nil
        }
    }

    private func saveHotkey(_ mode: HotkeyMode) {
        // Save to config file
        let configPath = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aether/config.toml")

        do {
            var content = ""
            if FileManager.default.fileExists(atPath: configPath.path) {
                content = try String(contentsOf: configPath, encoding: .utf8)
            }

            // Update or add summon line in [shortcuts] section
            let newSummon = "summon = \"\(mode.configString)\""

            if content.contains("summon = ") {
                // Replace existing summon line
                content = content.replacingOccurrences(
                    of: #"summon = \"[^\"]*\""#,
                    with: newSummon,
                    options: .regularExpression
                )
            } else if content.contains("[shortcuts]") {
                // Add after [shortcuts] section
                content = content.replacingOccurrences(
                    of: "[shortcuts]",
                    with: "[shortcuts]\n\(newSummon)"
                )
            } else {
                // Add new section
                content += "\n\n[shortcuts]\n\(newSummon)\n"
            }

            try content.write(to: configPath, atomically: true, encoding: .utf8)
            print("[ShortcutsView] Saved hotkey: \(mode.configString)")

            showingSaveConfirmation = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                showingSaveConfirmation = false
            }
        } catch {
            print("[ShortcutsView] Failed to save hotkey: \(error)")
        }
    }

    private func resetToDefault() {
        applyHotkey(.default)
    }

    private func openAccessibilitySettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }
}

// MARK: - Preset Hotkey Row

struct PresetHotkeyRow: View {
    let name: String
    let mode: HotkeyMode
    let description: String
    let isSelected: Bool
    let onSelect: () -> Void

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Text(name)
                        .font(DesignTokens.Typography.code)
                        .fontWeight(.semibold)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    if isSelected {
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundColor(DesignTokens.Colors.providerActive)
                    }
                }

                Text(description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            Spacer()

            ActionButton(L("settings.shortcuts.use_this"), style: .primary, isDisabled: isSelected) {
                onSelect()
            }
        }
        .padding(DesignTokens.Spacing.sm)
        .background(isSelected ? DesignTokens.Colors.accentBlue.opacity(0.1) : Color.clear)
        .cornerRadius(DesignTokens.CornerRadius.small)
    }
}

// MARK: - Custom Hotkey Recorder Sheet

struct CustomHotkeyRecorderSheet: View {
    let onSelect: (HotkeyMode) -> Void
    @Environment(\.dismiss) private var dismiss

    @State private var isRecording = false
    @State private var recordedHotkey: HotkeyMode?
    @State private var errorMessage: String?
    @State private var eventMonitor: Any?

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
            HStack {
                Text(L("settings.shortcuts.custom_hotkey_title"))
                    .font(DesignTokens.Typography.title)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
                Spacer()
                Button(L("common.cancel")) {
                    stopRecording()
                    dismiss()
                }
            }

            Text(L("settings.shortcuts.custom_hotkey_instruction"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Divider()

            // Recording area
            ZStack {
                RoundedRectangle(cornerRadius: 12)
                    .fill(isRecording ? DesignTokens.Colors.accentBlue.opacity(0.1) : DesignTokens.Colors.cardBackground)
                    .overlay(
                        RoundedRectangle(cornerRadius: 12)
                            .stroke(isRecording ? DesignTokens.Colors.accentBlue : DesignTokens.Colors.border, lineWidth: 2)
                    )
                    .frame(height: 80)

                VStack(spacing: DesignTokens.Spacing.sm) {
                    if isRecording {
                        HStack(spacing: 8) {
                            ProgressView()
                                .scaleEffect(0.8)
                            Text(L("settings.shortcuts.recording_prompt"))
                                .font(DesignTokens.Typography.body)
                                .foregroundColor(DesignTokens.Colors.accentBlue)
                        }
                    } else if let hotkey = recordedHotkey {
                        Text(hotkey.displayString)
                            .font(.system(size: 24, weight: .bold, design: .monospaced))
                            .foregroundColor(DesignTokens.Colors.textPrimary)
                    } else {
                        Text(L("settings.shortcuts.start_recording_hint"))
                            .font(DesignTokens.Typography.body)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }
            }

            // Error message
            if let error = errorMessage {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundColor(DesignTokens.Colors.warning)
                    Text(error)
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.warning)
                }
            }

            Spacer()

            // Action buttons
            HStack(spacing: DesignTokens.Spacing.md) {
                if isRecording {
                    ActionButton(L("settings.shortcuts.stop_recording"), style: .secondary) {
                        stopRecording()
                    }
                } else {
                    ActionButton(L("settings.shortcuts.start_recording"), icon: "record.circle", style: .primary) {
                        startRecording()
                    }
                }

                Spacer()

                if let hotkey = recordedHotkey, !isRecording {
                    ActionButton(L("settings.shortcuts.apply_hotkey"), icon: "checkmark.circle", style: .primary) {
                        onSelect(hotkey)
                    }
                }
            }
        }
        .padding(DesignTokens.Spacing.lg)
        .frame(width: 450, height: 300)
        .onDisappear {
            stopRecording()
        }
    }

    private func startRecording() {
        isRecording = true
        errorMessage = nil
        recordedHotkey = nil

        // Monitor for key events
        eventMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { event in
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
        // Get modifiers
        let flags = event.modifierFlags
        var cgFlags: CGEventFlags = []

        if flags.contains(.command) { cgFlags.insert(.maskCommand) }
        if flags.contains(.option) { cgFlags.insert(.maskAlternate) }
        if flags.contains(.shift) { cgFlags.insert(.maskShift) }
        if flags.contains(.control) { cgFlags.insert(.maskControl) }

        // Require at least one modifier
        if cgFlags.isEmpty {
            errorMessage = L("settings.shortcuts.error_requires_modifier")
            NSSound.beep()
            return
        }

        // Record the hotkey
        let keyCode = event.keyCode
        recordedHotkey = .modifierCombo(keyCode: keyCode, modifiers: cgFlags)
        errorMessage = nil

        stopRecording()
    }
}
