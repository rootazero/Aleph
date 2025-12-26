//
//  ShortcutsView.swift
//  Aether
//
//  Keyboard shortcuts configuration tab with hotkey recorder.
//

import SwiftUI
import AppKit

struct ShortcutsView: View {
    @State private var currentHotkey: Hotkey?
    @State private var showingPresets = false
    @State private var conflictWarning: String?
    @State private var showingSaveConfirmation = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Header
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Text("Keyboard Shortcuts")
                        .font(DesignTokens.Typography.title)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Text("Configure global keyboard shortcuts for Aether.")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                // Global Hotkey Card
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label("Global Hotkey", systemImage: "keyboard")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        HStack {
                            Text("Summon Aether:")
                                .font(DesignTokens.Typography.body)
                                .frame(width: 120, alignment: .leading)
                            Spacer()
                        }

                        HotkeyRecorderView(hotkey: $currentHotkey) { newHotkey in
                            handleHotkeyChange(newHotkey)
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

                        // Action buttons
                        HStack(spacing: DesignTokens.Spacing.md) {
                            ActionButton("Reset to Default", style: .secondary) {
                                resetToDefault()
                            }

                            ActionButton("Choose Preset...", style: .secondary) {
                                showingPresets = true
                            }

                            Spacer()

                            if showingSaveConfirmation {
                                Label("Saved!", systemImage: "checkmark.circle.fill")
                                    .foregroundColor(DesignTokens.Colors.providerActive)
                                    .font(DesignTokens.Typography.caption)
                            }
                        }
                        .padding(.top, DesignTokens.Spacing.sm)
                    }
                }
                .padding(DesignTokens.Spacing.md)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .fill(DesignTokens.Colors.cardBackground)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )
                .shadow(DesignTokens.Shadows.card)

                // Permission Card
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label("Permission Required", systemImage: "lock.shield")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Text("Aether requires **Accessibility** permission to detect global hotkeys.")
                            .font(DesignTokens.Typography.body)
                            .foregroundColor(DesignTokens.Colors.textPrimary)

                        Text("Why this is needed:")
                            .font(DesignTokens.Typography.caption)
                            .fontWeight(.semibold)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                            Label("Detect ⌘~ hotkey in any app", systemImage: "checkmark.circle")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                            Label("Read clipboard content", systemImage: "checkmark.circle")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                            Label("Simulate keyboard input for paste", systemImage: "checkmark.circle")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }

                        ActionButton("Open System Settings", icon: "gear", style: .primary) {
                            openAccessibilitySettings()
                        }
                        .padding(.top, DesignTokens.Spacing.sm)
                    }
                }
                .padding(DesignTokens.Spacing.md)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .fill(DesignTokens.Colors.cardBackground)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )
                .shadow(DesignTokens.Shadows.card)
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .sheet(isPresented: $showingPresets) {
            PresetShortcutsSheet(selectedHotkey: $currentHotkey) { hotkey in
                handleHotkeyChange(hotkey)
                showingPresets = false
            }
        }
        .onAppear {
            loadCurrentHotkey()
        }
    }

    private func loadCurrentHotkey() {
        // Load from config - for now, use default
        currentHotkey = Hotkey(modifiers: .command, keyCode: 50, character: "`")
    }

    private func handleHotkeyChange(_ newHotkey: Hotkey?) {
        guard let hotkey = newHotkey else {
            conflictWarning = nil
            return
        }

        // Check for conflicts
        conflictWarning = HotkeyConflictDetector.detectConflict(for: hotkey)

        // Save to config
        saveHotkey(hotkey)
    }

    private func saveHotkey(_ hotkey: Hotkey) {
        // TODO: Save to config via Rust core
        // For now, just show confirmation
        print("Saving hotkey: \(hotkey.configString)")

        showingSaveConfirmation = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            showingSaveConfirmation = false
        }
    }

    private func resetToDefault() {
        currentHotkey = Hotkey(modifiers: .command, keyCode: 50, character: "`")
        handleHotkeyChange(currentHotkey)
    }

    private func openAccessibilitySettings() {
        // Open System Settings to Accessibility preferences
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }
}

/// Preset shortcuts selection sheet
struct PresetShortcutsSheet: View {
    @Binding var selectedHotkey: Hotkey?
    let onSelect: (Hotkey) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
            HStack {
                Text("Choose a Preset Shortcut")
                    .font(DesignTokens.Typography.title)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
                Spacer()
                Button("Close") {
                    dismiss()
                }
            }

            Text("Select a common keyboard shortcut combination:")
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Divider()

            ScrollView {
                VStack(spacing: DesignTokens.Spacing.sm) {
                    ForEach(PresetShortcut.presets) { preset in
                        PresetShortcutRow(
                            preset: preset,
                            isSelected: selectedHotkey == preset.hotkey
                        ) {
                            onSelect(preset.hotkey)
                        }
                    }
                }
            }
        }
        .padding(DesignTokens.Spacing.lg)
        .frame(width: 600, height: 500)
    }
}

/// Row view for a single preset shortcut
struct PresetShortcutRow: View {
    let preset: PresetShortcut
    let isSelected: Bool
    let onSelect: () -> Void

    @State private var conflictWarning: String?

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Text(preset.hotkey.displayString)
                        .font(DesignTokens.Typography.code)
                        .fontWeight(.semibold)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    if isSelected {
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundColor(DesignTokens.Colors.providerActive)
                    }
                }

                Text(preset.description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                if let warning = conflictWarning {
                    Label(warning, systemImage: "exclamationmark.triangle")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.warning)
                }
            }

            Spacer()

            ActionButton("Use This", style: .primary, isDisabled: isSelected) {
                onSelect()
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(isSelected ? DesignTokens.Colors.accentBlue.opacity(0.1) : DesignTokens.Colors.cardBackground)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .stroke(isSelected ? DesignTokens.Colors.borderSelected : DesignTokens.Colors.border, lineWidth: isSelected ? 2 : 1)
        )
        .shadow(DesignTokens.Shadows.card)
        .onAppear {
            conflictWarning = HotkeyConflictDetector.detectConflict(for: preset.hotkey)
        }
    }
}
