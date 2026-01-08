//
//  ConversationInputView.swift
//  Aether
//
//  Multi-turn conversation input UI component.
//  Displays a simple text input field within the Halo overlay for conversation continuation.
//

import SwiftUI
import AppKit

/// View for multi-turn conversation input
///
/// Design Philosophy:
/// - Minimal: Only shows input field with turn indicator
/// - Ephemeral: Appears for input, dissolves after submit
/// - Non-intrusive: Small footprint, doesn't block much screen space
///
/// Layout:
/// ```
/// ┌─────────────────────────────────────┐
/// │  Turn 2                   ESC 结束  │
/// │ ┌─────────────────────────────────┐ │
/// │ │ Continue the conversation...    │ │
/// │ └─────────────────────────────────┘ │
/// └─────────────────────────────────────┘
/// ```
struct ConversationInputView: View {
    let sessionId: String
    let turnCount: UInt32
    @ObservedObject private var manager = DependencyContainer.shared.conversationManagerConcrete

    /// Text color - white for dark background
    private let textColor = Color.white

    /// Background color (dark gray)
    private let backgroundColor = Color(white: 0.1)

    /// Accent color for input field border
    private let accentColor = Color.accentColor

    var body: some View {
        VStack(spacing: 8) {
            // Header with turn count and ESC hint
            HStack {
                Text(String(format: L("conversation.turn", default: "Turn %d"), turnCount + 1))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(textColor.opacity(0.7))

                Spacer()

                Text("ESC \(L("conversation.end", default: "结束"))")
                    .font(.system(size: 10))
                    .foregroundColor(textColor.opacity(0.5))
            }

            // Text input field (using IMETextField for proper Chinese/Japanese/Korean input)
            IMETextField(
                text: $manager.textInput,
                placeholder: L("conversation.continue_placeholder", default: "Continue the conversation..."),
                font: .systemFont(ofSize: 18),
                textColor: .white,
                placeholderColor: NSColor.white.withAlphaComponent(0.5),
                backgroundColor: .clear,
                onSubmit: { submitInput() },
                onEscape: { manager.cancelConversation() }
            )
            .frame(height: 26)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(accentColor.opacity(0.4), lineWidth: 1)
            )

            // Hint
            HStack(spacing: 16) {
                Text(L("conversation.enter_to_send", default: "Enter to send"))
                    .font(.system(size: 10))
                    .foregroundColor(textColor.opacity(0.4))

                Spacer()
            }
        }
        .padding(12)
        .frame(width: 480)
        .background(backgroundColor.opacity(0.95))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .shadow(color: .black.opacity(0.2), radius: 8, x: 0, y: 4)
    }

    // MARK: - Actions

    private func submitInput() {
        let text = manager.textInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        manager.submitContinuationInput(text)
    }
}

// MARK: - Keyboard Navigation

extension ConversationInputView {
    /// Handle keyboard events for navigation
    func handleKeyEvent(_ event: NSEvent) -> Bool {
        switch event.keyCode {
        case 36:  // Return/Enter - submit input
            submitInput()
            return true

        case 53:  // Escape - cancel conversation
            manager.cancelConversation()
            return true

        default:
            return false
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
struct ConversationInputView_Previews: PreviewProvider {
    static var previews: some View {
        ConversationInputView(sessionId: "test-session", turnCount: 1)
            .padding()
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Turn 2 Input")
    }
}
#endif
