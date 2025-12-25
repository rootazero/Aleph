//
//  ShortcutsView.swift
//  Aether
//
//  Keyboard shortcuts configuration tab with hotkey recorder.
//

import SwiftUI

struct ShortcutsView: View {
    @State private var currentHotkey: Hotkey?
    @State private var showingPresets = false
    @State private var conflictWarning: String?
    @State private var showingSaveConfirmation = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Keyboard Shortcuts")
                    .font(.title2)

                Text("Configure global keyboard shortcuts for Aether.")
                    .foregroundColor(.secondary)
                    .font(.callout)

                Form {
                    Section(header: Text("Global Hotkey")) {
                        VStack(alignment: .leading, spacing: 12) {
                            HStack {
                                Text("Summon Aether:")
                                    .frame(width: 120, alignment: .leading)
                                Spacer()
                            }

                            HotkeyRecorderView(hotkey: $currentHotkey) { newHotkey in
                                handleHotkeyChange(newHotkey)
                            }

                            // Conflict warning
                            if let warning = conflictWarning {
                                Label(warning, systemImage: "exclamationmark.triangle")
                                    .foregroundColor(.orange)
                                    .font(.caption)
                                    .padding(8)
                                    .background(Color.orange.opacity(0.1))
                                    .cornerRadius(6)
                            }

                            // Action buttons
                            HStack(spacing: 12) {
                                Button("Reset to Default") {
                                    resetToDefault()
                                }
                                .buttonStyle(.bordered)

                                Button("Choose Preset...") {
                                    showingPresets = true
                                }
                                .buttonStyle(.bordered)

                                Spacer()

                                if showingSaveConfirmation {
                                    Label("Saved!", systemImage: "checkmark.circle.fill")
                                        .foregroundColor(.green)
                                        .font(.caption)
                                }
                            }
                            .padding(.top, 8)
                        }
                    }

                    Section(header: Text("Permission Required")) {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("Aether requires **Accessibility** permission to detect global hotkeys.")
                                .font(.callout)

                            Text("Why this is needed:")
                                .font(.caption)
                                .fontWeight(.semibold)

                            VStack(alignment: .leading, spacing: 4) {
                                Label("Detect ⌘~ hotkey in any app", systemImage: "checkmark.circle")
                                    .font(.caption)
                                Label("Read clipboard content", systemImage: "checkmark.circle")
                                    .font(.caption)
                                Label("Simulate keyboard input for paste", systemImage: "checkmark.circle")
                                    .font(.caption)
                            }

                            Button("Open System Settings") {
                                PermissionManager().openAccessibilitySettings()
                            }
                            .padding(.top, 8)
                        }
                    }
                }
                .formStyle(.grouped)
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(20)
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
}

/// Preset shortcuts selection sheet
struct PresetShortcutsSheet: View {
    @Binding var selectedHotkey: Hotkey?
    let onSelect: (Hotkey) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Choose a Preset Shortcut")
                    .font(.title2)
                Spacer()
                Button("Close") {
                    dismiss()
                }
            }

            Text("Select a common keyboard shortcut combination:")
                .foregroundColor(.secondary)
                .font(.callout)

            Divider()

            ScrollView {
                VStack(spacing: 8) {
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
        .padding(24)
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
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(preset.hotkey.displayString)
                        .font(.system(.body, design: .monospaced))
                        .fontWeight(.semibold)

                    if isSelected {
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundColor(.green)
                    }
                }

                Text(preset.description)
                    .font(.caption)
                    .foregroundColor(.secondary)

                if let warning = conflictWarning {
                    Label(warning, systemImage: "exclamationmark.triangle")
                        .foregroundColor(.orange)
                        .font(.caption2)
                }
            }

            Spacer()

            Button("Use This") {
                onSelect()
            }
            .buttonStyle(.borderedProminent)
            .disabled(isSelected)
        }
        .padding(12)
        .background(isSelected ? Color.blue.opacity(0.1) : Color.gray.opacity(0.05))
        .cornerRadius(8)
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(isSelected ? Color.blue : Color.clear, lineWidth: 2)
        )
        .onAppear {
            conflictWarning = HotkeyConflictDetector.detectConflict(for: preset.hotkey)
        }
    }
}
