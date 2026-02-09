//
//  SettingsView.swift
//  Aleph
//
//  Simplified settings view - all configuration now managed via ControlPlane
//  Uses WebSocket connection to Gateway instead of FFI
//

import SwiftUI
import AppKit

// MARK: - Settings Tab Enum (Simplified)

enum SettingsTab: Hashable {
    case general
}

// MARK: - Simplified Settings View

struct SettingsView: View {
    @StateObject private var wsClient = GatewayWebSocketClient()
    @State private var currentProvider: String = "Loading..."

    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

    private var connectionStatusText: String {
        switch wsClient.connectionState {
        case .disconnected:
            return "Disconnected"
        case .connecting:
            return "Connecting..."
        case .connected:
            return "Connected"
        case .reconnecting:
            return "Reconnecting..."
        }
    }

    private var connectionStatusColor: Color {
        switch wsClient.connectionState {
        case .disconnected:
            return .red
        case .connecting, .reconnecting:
            return .orange
        case .connected:
            return .green
        }
    }

    var body: some View {
        VStack(spacing: 24) {
            // Connection Status
            VStack(alignment: .leading, spacing: 8) {
                Text("Gateway Connection")
                    .font(.headline)

                HStack {
                    Circle()
                        .fill(connectionStatusColor)
                        .frame(width: 10, height: 10)
                    Text(connectionStatusText)
                        .foregroundColor(.secondary)
                }

                if let error = wsClient.lastError {
                    Text(error)
                        .font(.caption)
                        .foregroundColor(.red)
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

            // Actions
            VStack(spacing: 12) {
                // Open ControlPlane Button
                Button(action: openControlPlane) {
                    HStack {
                        Image(systemName: "gearshape.2")
                        Text("Open Control Panel")
                    }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)

                // Connect/Disconnect Button
                Button(action: toggleConnection) {
                    HStack {
                        Image(systemName: wsClient.connectionState == .connected ? "network.slash" : "network")
                        Text(wsClient.connectionState == .connected ? "Disconnect" : "Connect")
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
            }

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
        .frame(width: 400, height: 350)
        .onAppear {
            wsClient.connect()
            loadCurrentProvider()
        }
        .onDisappear {
            wsClient.disconnect()
        }
    }

    private func openControlPlane() {
        // Default ControlPlane URL
        let controlPlaneURL = "http://127.0.0.1:18790/cp"

        if let url = URL(string: controlPlaneURL) {
            NSWorkspace.shared.open(url)
        }
    }

    private func toggleConnection() {
        if wsClient.connectionState == .connected {
            wsClient.disconnect()
        } else {
            wsClient.connect()
        }
    }

    private func loadCurrentProvider() {
        // TODO: Get current provider from Gateway via RPC
        Task {
            do {
                // Wait for connection
                try await Task.sleep(nanoseconds: 2_000_000_000) // 2 seconds

                if wsClient.connectionState == .connected {
                    // Example RPC call (will implement proper config.get later)
                    // let response: JSONRPCResponse = try await wsClient.sendRequest(method: "config.get", params: nil)
                    currentProvider = "Claude (Anthropic)"
                }
            } catch {
                print("[SettingsView] Failed to load provider: \(error)")
                currentProvider = "Unknown"
            }
        }
    }
}

// MARK: - Preview

#if DEBUG
struct SettingsView_Previews: PreviewProvider {
    static var previews: some View {
        SettingsView()
    }
}
#endif
