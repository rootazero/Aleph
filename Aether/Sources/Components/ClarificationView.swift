//
//  ClarificationView.swift
//  Aether
//
//  Phantom Flow clarification UI component.
//  Displays in-place options or text input within the Halo overlay.
//

import SwiftUI
import AppKit

/// View for displaying Phantom Flow clarification requests
///
/// Supports two interaction modes:
/// - Select: Display a list of options for the user to choose from
/// - Text: Display a text input field for free-form input
///
/// Design Philosophy:
/// - Ephemeral: Appears quickly, responds to input, dissolves
/// - Minimal: Only shows what's needed, no extra chrome
/// - Native: Uses system-standard interactions (arrow keys, enter, escape)
struct ClarificationView: View {
    let request: ClarificationRequest
    @ObservedObject private var manager = ClarificationManager.shared

    /// Accent color from system
    private let accentColor = Color.accentColor

    /// Text color - white for dark background
    private let textColor = Color.white

    /// Background color (dark gray)
    private let backgroundColor = Color(white: 0.1)

    var body: some View {
        VStack(spacing: 12) {
            // Prompt
            Text(request.prompt)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(textColor)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 8)

            // Content based on type
            switch request.clarificationType {
            case .select:
                selectContent
            case .text:
                textContent
            }

            // Source indicator (optional)
            if let source = request.source {
                sourceIndicator(source)
            }
        }
        .padding(16)
        .frame(minWidth: 200, maxWidth: 320)
        .background(backgroundColor.opacity(0.95))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .shadow(color: .black.opacity(0.2), radius: 8, x: 0, y: 4)
    }

    // MARK: - Select Content

    private var selectContent: some View {
        VStack(spacing: 4) {
            if let options = request.options {
                ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                    optionRow(option: option, index: index, isSelected: manager.selectedIndex == index)
                        .onTapGesture {
                            selectOption(index: index, option: option)
                        }
                }
            }
        }
    }

    private func optionRow(option: ClarificationOption, index: Int, isSelected: Bool) -> some View {
        HStack(spacing: 8) {
            // Selection indicator
            Circle()
                .fill(isSelected ? accentColor : Color.clear)
                .frame(width: 8, height: 8)
                .overlay(
                    Circle()
                        .stroke(isSelected ? accentColor : textColor.opacity(0.3), lineWidth: 1)
                )

            VStack(alignment: .leading, spacing: 2) {
                Text(option.label)
                    .font(.system(size: 13, weight: isSelected ? .semibold : .regular))
                    .foregroundColor(isSelected ? accentColor : textColor)

                if let description = option.description {
                    Text(description)
                        .font(.system(size: 11))
                        .foregroundColor(textColor.opacity(0.6))
                }
            }

            Spacer()

            // Keyboard shortcut hint
            Text("\(index + 1)")
                .font(.system(size: 10, weight: .medium).monospacedDigit())
                .foregroundColor(textColor.opacity(0.4))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(textColor.opacity(0.1))
                .clipShape(RoundedRectangle(cornerRadius: 4))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(isSelected ? accentColor.opacity(0.1) : Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .contentShape(Rectangle())
    }

    private func selectOption(index: Int, option: ClarificationOption) {
        manager.selectedIndex = index
        // Auto-confirm on click
        manager.completeWithSelection(index: index, value: option.value)
    }

    // MARK: - Text Content

    private var textContent: some View {
        VStack(spacing: 8) {
            // Using IMETextField for proper Chinese/Japanese/Korean input
            IMETextField(
                text: $manager.textInput,
                placeholder: request.placeholder ?? "Enter text...",
                font: .systemFont(ofSize: 14),
                textColor: .white,
                backgroundColor: NSColor.white.withAlphaComponent(0.05),
                onSubmit: { confirmTextInput() },
                onEscape: { manager.cancel() }
            )
            .frame(height: 32)
            .padding(10)
            .background(textColor.opacity(0.05))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(accentColor.opacity(0.3), lineWidth: 1)
            )

            // Hint
            HStack(spacing: 16) {
                Text(L("clarification.enter_to_confirm", default: "Enter to confirm"))
                    .font(.system(size: 10))
                    .foregroundColor(textColor.opacity(0.5))

                Text(L("clarification.esc_to_cancel", default: "Esc to cancel"))
                    .font(.system(size: 10))
                    .foregroundColor(textColor.opacity(0.5))
            }
        }
    }

    private func confirmTextInput() {
        guard !manager.textInput.isEmpty else { return }
        manager.completeWithText(manager.textInput)
    }

    // MARK: - Source Indicator

    private func sourceIndicator(_ source: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: "sparkles")
                .font(.system(size: 9))
            Text(source)
                .font(.system(size: 9))
        }
        .foregroundColor(textColor.opacity(0.4))
        .padding(.top, 4)
    }
}

// MARK: - Keyboard Navigation

extension ClarificationView {
    /// Handle keyboard events for navigation
    func handleKeyEvent(_ event: NSEvent) -> Bool {
        guard request.clarificationType == .select else {
            // For text mode, only handle Escape
            if event.keyCode == 53 { // Escape
                manager.cancel()
                return true
            }
            return false
        }

        guard let options = request.options, !options.isEmpty else { return false }

        switch event.keyCode {
        case 125: // Down arrow
            let newIndex = min(manager.selectedIndex + 1, options.count - 1)
            manager.selectedIndex = newIndex
            return true

        case 126: // Up arrow
            let newIndex = max(manager.selectedIndex - 1, 0)
            manager.selectedIndex = newIndex
            return true

        case 36: // Return/Enter
            let index = manager.selectedIndex
            if index < options.count {
                manager.completeWithSelection(index: index, value: options[index].value)
            }
            return true

        case 53: // Escape
            manager.cancel()
            return true

        case 18...26: // Number keys 1-9
            let numberIndex = Int(event.keyCode) - 18
            if numberIndex < options.count {
                manager.selectedIndex = numberIndex
                manager.completeWithSelection(index: numberIndex, value: options[numberIndex].value)
            }
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
struct ClarificationView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            // Select type preview
            ClarificationView(request: ClarificationManager.testSelectRequest())
                .padding()
                .background(Color.black.opacity(0.8))
                .previewDisplayName("Select Type")

            // Text type preview
            ClarificationView(request: ClarificationManager.testTextRequest())
                .padding()
                .background(Color.black.opacity(0.8))
                .previewDisplayName("Text Type")
        }
    }
}
#endif
