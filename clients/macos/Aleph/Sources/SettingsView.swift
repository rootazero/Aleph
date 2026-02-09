//
//  SettingsView.swift
//  Aleph
//
//  Simplified settings view - all configuration now managed via ControlPlane
//

import SwiftUI
import AppKit

// MARK: - Settings Tab Enum (Simplified)

enum SettingsTab: Hashable {
    case general
}

// MARK: - Simplified Settings View

struct SettingsView: View {
    let core: AlephCore?

    @State private var connectionStatus: String = "Checking..."
    @State private var currentProvider: String = "Loading..."

    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

    var body: some View {
        VStack(spacing: 24) {
            // Connection Status
            VStack(alignment: .leading, spacing: 8) {
                Text("Connection Status")
                    .font(.headline)

                HStack {
                    Circle()
                        .fill(connectionStatus == "Connected" ? Color.green : Color.red)
                        .frame(width: 10, height: 10)
                    Text(connectionStatus)
                        .foregroundColor(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Divider()

            // Current Configuration (Read-only)
            VStack(alignment: .leading, spacing: 8) {
                Text("Current AI Provider")
                    .font(.headline)
                Text(currentProvider)
                    .foregroundColor(.secondary)
                Text("To modify settings, use the Control Panel")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Divider()

            // Open ControlPlane Button
            Button(action: openControlPlane) {
                HStack {
                    Image(systemName: "gearshape.2")
                    Text("Open Control Panel")
                }
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(core == nil)

            Spacer()

            // About
            VStack(spacing: 4) {
                Text("Aleph")
                    .font(.headline)
                Text("Version \(appVersion)")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
        }
        .padding()
        .frame(width: 400, height: 300)
        .onAppear {
            updateConnectionStatus()
            loadCurrentProvider()
        }
    }

    private func openControlPlane() {
        // Default ControlPlane URL
        let controlPlaneURL = "http://127.0.0.1:18790/cp"

        if let url = URL(string: controlPlaneURL) {
            NSWorkspace.shared.open(url)
        }
    }

    private func updateConnectionStatus() {
        // TODO: Get actual connection status from core
        connectionStatus = core != nil ? "Connected" : "Disconnected"
    }

    private func loadCurrentProvider() {
        // TODO: Get current provider from core
        currentProvider = "Claude (Anthropic)"
    }
}

// MARK: - Preview

#if DEBUG
struct SettingsView_Previews: PreviewProvider {
    static var previews: some View {
        SettingsView(core: nil)
    }
}
#endif
