//
//  ConversationInputView.swift
//  Aether
//
//  Multi-turn conversation input UI component.
//  Displays a simple text input field within the Halo overlay for conversation continuation.
//

import SwiftUI

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
    @ObservedObject private var manager = ConversationManager.shared

    /// Focus state for keyboard input
    @FocusState private var isTextFieldFocused: Bool

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
                Text("Turn \(turnCount + 1)")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(textColor.opacity(0.7))

                Spacer()

                Text("ESC \(L("conversation.end", default: "结束"))")
                    .font(.system(size: 10))
                    .foregroundColor(textColor.opacity(0.5))
            }

            // Text input field
            TextField(
                L("conversation.continue_placeholder", default: "Continue the conversation..."),
                text: $manager.textInput
            )
            .textFieldStyle(.plain)
            .font(.system(size: 14))
            .foregroundColor(textColor)
            .padding(10)
            .background(textColor.opacity(0.05))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(accentColor.opacity(0.3), lineWidth: 1)
            )
            .focused($isTextFieldFocused)
            .onSubmit {
                submitInput()
            }

            // Hint
            HStack(spacing: 16) {
                Text(L("conversation.enter_to_send", default: "Enter to send"))
                    .font(.system(size: 10))
                    .foregroundColor(textColor.opacity(0.4))

                Spacer()
            }
        }
        .padding(12)
        .frame(width: 320)
        .background(backgroundColor.opacity(0.95))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .shadow(color: .black.opacity(0.2), radius: 8, x: 0, y: 4)
        .onAppear {
            // Auto-focus text field
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                isTextFieldFocused = true
            }
        }
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
