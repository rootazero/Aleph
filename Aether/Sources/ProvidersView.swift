//
//  ProvidersView.swift
//  Aether
//
//  AI Providers configuration tab (read-only for Phase 2).
//

import SwiftUI

struct Provider: Identifiable {
    let id = UUID()
    let name: String
    let color: Color
    let apiKeyStatus: String
}

struct ProvidersView: View {
    // Hardcoded providers for Phase 2
    private let providers = [
        Provider(name: "OpenAI", color: Color(hex: "#10a37f") ?? .green, apiKeyStatus: "Not Configured"),
        Provider(name: "Claude", color: Color(hex: "#d97757") ?? .orange, apiKeyStatus: "Not Configured"),
        Provider(name: "Gemini", color: Color(hex: "#4285F4") ?? .blue, apiKeyStatus: "Not Configured"),
        Provider(name: "Ollama (Local)", color: .black, apiKeyStatus: "Not Configured")
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("AI Providers")
                .font(.title2)

            Text("Configure your AI provider API keys. These will be used for routing requests.")
                .foregroundColor(.secondary)
                .font(.callout)

            List(providers) { provider in
                HStack {
                    Circle()
                        .fill(provider.color)
                        .frame(width: 12, height: 12)

                    VStack(alignment: .leading) {
                        Text(provider.name)
                            .font(.headline)
                        Text("API Key: \(provider.apiKeyStatus)")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Spacer()

                    Button("Configure") {
                        showComingSoonAlert(provider: provider.name)
                    }
                }
                .padding(.vertical, 4)
            }
            .listStyle(.inset)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(20)
    }

    private func showComingSoonAlert(provider: String) {
        let alert = NSAlert()
        alert.messageText = "Coming Soon"
        alert.informativeText = "\(provider) configuration will be available in Phase 4."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}

// MARK: - Color Extension for Hex

extension Color {
    init?(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let a, r, g, b: UInt64
        switch hex.count {
        case 3: // RGB (12-bit)
            (a, r, g, b) = (255, (int >> 8) * 17, (int >> 4 & 0xF) * 17, (int & 0xF) * 17)
        case 6: // RGB (24-bit)
            (a, r, g, b) = (255, int >> 16, int >> 8 & 0xFF, int & 0xFF)
        case 8: // ARGB (32-bit)
            (a, r, g, b) = (int >> 24, int >> 16 & 0xFF, int >> 8 & 0xFF, int & 0xFF)
        default:
            return nil
        }

        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue:  Double(b) / 255,
            opacity: Double(a) / 255
        )
    }
}
