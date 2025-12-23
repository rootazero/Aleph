//
//  RoutingView.swift
//  Aether
//
//  Routing rules configuration tab (read-only for Phase 2).
//

import SwiftUI

struct RoutingRule: Identifiable {
    let id = UUID()
    let pattern: String
    let provider: String
    let description: String
}

struct RoutingView: View {
    // Hardcoded routing rules for Phase 2
    private let rules = [
        RoutingRule(
            pattern: "^/draw",
            provider: "OpenAI",
            description: "DALL-E image generation"
        ),
        RoutingRule(
            pattern: "^/code|rust|python",
            provider: "Claude",
            description: "Code-related queries"
        ),
        RoutingRule(
            pattern: ".*",
            provider: "OpenAI",
            description: "Catch-all default"
        )
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Routing Rules")
                .font(.title2)

            Text("Define how clipboard content is routed to AI providers based on patterns.")
                .foregroundColor(.secondary)
                .font(.callout)

            List(rules) { rule in
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("Pattern:")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(rule.pattern)
                            .font(.system(.body, design: .monospaced))
                    }

                    HStack {
                        Text("Provider:")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(rule.provider)
                            .font(.body)
                    }

                    Text(rule.description)
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                .padding(.vertical, 4)
            }
            .listStyle(.inset)

            HStack {
                Spacer()
                Button("Add Rule") {
                    showComingSoonAlert()
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(20)
    }

    private func showComingSoonAlert() {
        let alert = NSAlert()
        alert.messageText = "Coming Soon"
        alert.informativeText = "Rule editing will be available in Phase 4."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}
