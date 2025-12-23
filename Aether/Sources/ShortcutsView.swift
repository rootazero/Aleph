//
//  ShortcutsView.swift
//  Aether
//
//  Keyboard shortcuts configuration tab (read-only for Phase 2).
//

import SwiftUI

struct ShortcutsView: View {
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
                        HStack {
                            Text("Summon Aether:")
                            Spacer()
                            Text("⌘ + ~")
                                .font(.system(.body, design: .monospaced))
                                .padding(6)
                                .background(Color.gray.opacity(0.2))
                                .cornerRadius(4)

                            Button("Change") {
                                showComingSoonAlert()
                            }
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
    }

    private func showComingSoonAlert() {
        let alert = NSAlert()
        alert.messageText = "Coming Soon"
        alert.informativeText = "Shortcut customization will be available in Phase 3."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}
