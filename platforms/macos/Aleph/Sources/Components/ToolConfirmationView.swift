//
//  ToolConfirmationView.swift
//  Aether
//
//  Tool confirmation UI for async dispatcher flow (Phase 6).
//  Displays tool information and confidence level for user confirmation.
//

import SwiftUI

/// View for displaying tool confirmation requests
///
/// Shows:
/// - Tool name and description
/// - Confidence indicator (visual bar)
/// - Reason for confirmation
/// - Execute/Cancel buttons
///
/// Design Philosophy:
/// - Clear: Shows exactly what tool will execute
/// - Informative: Displays confidence level and reasoning
/// - Actionable: Simple Execute/Cancel choice
struct ToolConfirmationView: View {
    let confirmationId: String
    let toolName: String
    let toolDescription: String
    let reason: String
    let confidence: Float
    let onExecute: () -> Void
    let onCancel: () -> Void

    /// Text color - white for dark background
    private let textColor = Color.white

    /// Background color (dark gray)
    private let backgroundColor = Color(white: 0.1)

    /// Accent color for execute button
    private let executeColor = Color(red: 0.2, green: 0.8, blue: 0.4)

    /// Cancel color
    private let cancelColor = Color(red: 0.9, green: 0.3, blue: 0.3)

    var body: some View {
        VStack(spacing: 16) {
            // Header with tool icon and name
            HStack(spacing: 12) {
                Image(systemName: "wrench.and.screwdriver.fill")
                    .font(.system(size: 24))
                    .foregroundColor(.accentColor)

                VStack(alignment: .leading, spacing: 2) {
                    Text(toolName)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(textColor)

                    Text(toolDescription)
                        .font(.system(size: 12))
                        .foregroundColor(textColor.opacity(0.7))
                        .lineLimit(2)
                }

                Spacer()
            }

            // Confidence indicator
            confidenceBar

            // Reason for confirmation
            HStack(spacing: 6) {
                Image(systemName: "info.circle")
                    .font(.system(size: 11))
                    .foregroundColor(textColor.opacity(0.5))

                Text(reason)
                    .font(.system(size: 11))
                    .foregroundColor(textColor.opacity(0.6))
                    .multilineTextAlignment(.leading)

                Spacer()
            }

            // Action buttons
            HStack(spacing: 12) {
                // Cancel button
                Button(action: onCancel) {
                    HStack(spacing: 6) {
                        Image(systemName: "xmark")
                            .font(.system(size: 12, weight: .medium))
                        Text(L("tool_confirmation.cancel", default: "Cancel"))
                            .font(.system(size: 13, weight: .medium))
                    }
                    .foregroundColor(cancelColor)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
                    .background(cancelColor.opacity(0.15))
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(cancelColor.opacity(0.3), lineWidth: 1)
                    )
                }
                .buttonStyle(.plain)

                // Execute button
                Button(action: onExecute) {
                    HStack(spacing: 6) {
                        Image(systemName: "play.fill")
                            .font(.system(size: 12, weight: .medium))
                        Text(L("tool_confirmation.execute", default: "Execute"))
                            .font(.system(size: 13, weight: .medium))
                    }
                    .foregroundColor(executeColor)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
                    .background(executeColor.opacity(0.15))
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(executeColor.opacity(0.3), lineWidth: 1)
                    )
                }
                .buttonStyle(.plain)
            }

            // Keyboard hint
            HStack(spacing: 16) {
                keyboardHint(key: "↵", action: L("tool_confirmation.enter_execute", default: "Execute"))
                keyboardHint(key: "Esc", action: L("tool_confirmation.esc_cancel", default: "Cancel"))
            }
        }
        .padding(16)
        .frame(minWidth: 280, maxWidth: 380)
        .background(backgroundColor.opacity(0.95))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .shadow(color: .black.opacity(0.2), radius: 8, x: 0, y: 4)
    }

    // MARK: - Confidence Bar

    private var confidenceBar: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(L("tool_confirmation.confidence", default: "Confidence"))
                    .font(.system(size: 11))
                    .foregroundColor(textColor.opacity(0.6))

                Spacer()

                Text("\(Int(confidence * 100))%")
                    .font(.system(size: 11, weight: .medium).monospacedDigit())
                    .foregroundColor(confidenceColor)
            }

            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    // Background bar
                    RoundedRectangle(cornerRadius: 3)
                        .fill(textColor.opacity(0.1))

                    // Filled bar
                    RoundedRectangle(cornerRadius: 3)
                        .fill(confidenceColor)
                        .frame(width: geometry.size.width * CGFloat(confidence))
                }
            }
            .frame(height: 6)
        }
    }

    /// Color based on confidence level
    private var confidenceColor: Color {
        if confidence >= 0.8 {
            return Color(red: 0.2, green: 0.8, blue: 0.4)  // Green
        } else if confidence >= 0.5 {
            return Color(red: 1.0, green: 0.7, blue: 0.2)  // Orange
        } else {
            return Color(red: 0.9, green: 0.3, blue: 0.3)  // Red
        }
    }

    // MARK: - Keyboard Hint

    private func keyboardHint(key: String, action: String) -> some View {
        HStack(spacing: 4) {
            Text(key)
                .font(.system(size: 10, weight: .medium).monospaced())
                .foregroundColor(textColor.opacity(0.4))
                .padding(.horizontal, 4)
                .padding(.vertical, 2)
                .background(textColor.opacity(0.1))
                .clipShape(RoundedRectangle(cornerRadius: 3))

            Text(action)
                .font(.system(size: 10))
                .foregroundColor(textColor.opacity(0.4))
        }
    }
}

// MARK: - Localization Helper

/// Localization helper with fallback
private func L(_ key: String, default defaultValue: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    return localized == key ? defaultValue : localized
}

// MARK: - Previews

#if DEBUG
struct ToolConfirmationView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            // High confidence
            ToolConfirmationView(
                confirmationId: "test-1",
                toolName: "Web Search",
                toolDescription: "Search the web for information",
                reason: "Confidence below threshold (0.85 < 0.90)",
                confidence: 0.85,
                onExecute: {},
                onCancel: {}
            )
            .padding()
            .background(Color.black.opacity(0.8))
            .previewDisplayName("High Confidence")

            // Medium confidence
            ToolConfirmationView(
                confirmationId: "test-2",
                toolName: "File System",
                toolDescription: "Read or write files on your system",
                reason: "Ambiguous tool selection - please confirm",
                confidence: 0.65,
                onExecute: {},
                onCancel: {}
            )
            .padding()
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Medium Confidence")

            // Low confidence
            ToolConfirmationView(
                confirmationId: "test-3",
                toolName: "Shell Command",
                toolDescription: "Execute a shell command",
                reason: "Low confidence - multiple tools matched",
                confidence: 0.35,
                onExecute: {},
                onCancel: {}
            )
            .padding()
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Low Confidence")
        }
    }
}
#endif
