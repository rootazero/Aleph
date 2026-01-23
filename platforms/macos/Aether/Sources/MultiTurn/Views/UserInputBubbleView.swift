//
//  UserInputBubbleView.swift
//  Aether
//
//  Inline user input request view displayed in conversation area.
//  Shows question with optional choices and text input.
//

import SwiftUI

/// Inline user input request view displayed as a message bubble in conversation
struct UserInputBubbleView: View {
    let request: PendingUserInputRequest
    var onRespond: (String) -> Void
    var onCancel: () -> Void

    @State private var inputText: String = ""
    @State private var selectedOption: String?
    @State private var isHoveringSend = false
    @State private var isHoveringCancel = false

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            // Content aligned to left (assistant side)
            inputCard
            Spacer(minLength: 40)
        }
    }

    // MARK: - Input Card

    private var inputCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Header with icon and question
            header

            // Options or text input
            if request.hasOptions {
                optionsList
            } else {
                textInput
            }

            // Action buttons
            actionButtons
        }
        .padding(16)
        .background(cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(
            RoundedRectangle(cornerRadius: 16)
                .stroke(Color.blue.opacity(0.3), lineWidth: 1)
        )
        .frame(maxWidth: 500)
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "questionmark.circle.fill")
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(.blue)

            Text(NSLocalizedString("user_input.title", comment: "Input Required"))
                .font(.headline)
                .liquidGlassText()

            Spacer()
        }
    }

    // MARK: - Question Text

    private var questionText: some View {
        Text(request.question)
            .font(.subheadline)
            .liquidGlassText()
            .lineLimit(nil)
            .fixedSize(horizontal: false, vertical: true)
    }

    // MARK: - Options List

    private var optionsList: some View {
        VStack(alignment: .leading, spacing: 8) {
            questionText

            Divider()
                .opacity(0.3)

            ForEach(request.options, id: \.self) { option in
                optionRow(option)
            }
        }
    }

    private func optionRow(_ option: String) -> some View {
        Button(action: {
            selectedOption = option
            inputText = option
        }) {
            HStack(spacing: 8) {
                // Selection indicator
                Image(systemName: selectedOption == option ? "checkmark.circle.fill" : "circle")
                    .font(.caption)
                    .foregroundColor(selectedOption == option ? .blue : GlassColors.secondaryText)

                // Option text
                Text(option)
                    .font(.subheadline)
                    .liquidGlassText()
                    .lineLimit(2)

                Spacer()
            }
            .padding(.vertical, 6)
            .padding(.horizontal, 8)
            .background(
                selectedOption == option
                    ? Color.blue.opacity(0.1)
                    : Color.clear
            )
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
    }

    // MARK: - Text Input

    private var textInput: some View {
        VStack(alignment: .leading, spacing: 8) {
            questionText

            Divider()
                .opacity(0.3)

            TextField(
                NSLocalizedString("user_input.placeholder", comment: "Enter your response..."),
                text: $inputText,
                axis: .vertical
            )
            .textFieldStyle(.plain)
            .font(.subheadline)
            .liquidGlassText()
            .padding(10)
            .background(Color.black.opacity(0.1))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(Color.white.opacity(0.1), lineWidth: 1)
            )
            .lineLimit(1...5)
        }
    }

    // MARK: - Action Buttons

    private var actionButtons: some View {
        HStack(spacing: 12) {
            Spacer()

            // Cancel button
            Button(action: onCancel) {
                HStack(spacing: 4) {
                    Image(systemName: "xmark")
                        .font(.caption)
                    Text(NSLocalizedString("common.cancel", comment: ""))
                        .font(.subheadline)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(
                    isHoveringCancel
                        ? Color.gray.opacity(0.2)
                        : Color.gray.opacity(0.1)
                )
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .onHover { isHoveringCancel = $0 }

            // Send button
            Button(action: {
                let response = request.hasOptions ? (selectedOption ?? "") : inputText
                onRespond(response)
            }) {
                HStack(spacing: 4) {
                    Image(systemName: "paperplane.fill")
                        .font(.caption)
                    Text(NSLocalizedString("common.confirm", comment: ""))
                        .font(.subheadline)
                        .fontWeight(.medium)
                }
                .foregroundColor(.white)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(
                    canSend
                        ? (isHoveringSend ? Color.blue : Color.blue.opacity(0.9))
                        : Color.gray.opacity(0.5)
                )
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .onHover { isHoveringSend = $0 }
            .disabled(!canSend)
        }
    }

    // MARK: - Helper

    private var canSend: Bool {
        if request.hasOptions {
            return selectedOption != nil
        } else {
            return !inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }

    // MARK: - Background

    private var cardBackground: some View {
        ZStack {
            // Glass effect background
            RoundedRectangle(cornerRadius: 16)
                .fill(.ultraThinMaterial)

            // Subtle gradient overlay
            RoundedRectangle(cornerRadius: 16)
                .fill(
                    LinearGradient(
                        colors: [
                            Color.white.opacity(0.05),
                            Color.clear
                        ],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                )
        }
    }
}

// MARK: - Preview

#Preview {
    VStack(spacing: 20) {
        // Free-form text input
        UserInputBubbleView(
            request: PendingUserInputRequest(
                requestId: "test-1",
                question: "What would you like the image to look like? Please describe the style and content.",
                options: []
            ),
            onRespond: { print("Response: \($0)") },
            onCancel: { print("Cancelled") }
        )

        // Multiple choice options
        UserInputBubbleView(
            request: PendingUserInputRequest(
                requestId: "test-2",
                question: "Which output format do you prefer?",
                options: ["PNG (Best quality)", "JPEG (Smaller size)", "WebP (Modern format)"]
            ),
            onRespond: { print("Response: \($0)") },
            onCancel: { print("Cancelled") }
        )
    }
    .padding()
    .frame(width: 600)
    .background(Color.black.opacity(0.8))
}
