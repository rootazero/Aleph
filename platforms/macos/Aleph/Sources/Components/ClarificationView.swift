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
/// Supports three interaction modes:
/// - Select: Display a list of options for the user to choose from (with "Other" option for custom input)
/// - Text: Display a text input field for free-form input
/// - MultiGroup: Display multiple question groups, each with its own set of options
///
/// Design Philosophy:
/// - Ephemeral: Appears quickly, responds to input, dissolves
/// - Minimal: Only shows what's needed, no extra chrome
/// - Native: Uses system-standard interactions (arrow keys, enter, escape)
struct ClarificationView: View {
    let request: ClarificationRequest
    @ObservedObject private var manager = ClarificationManager.shared

    /// Track if user selected "Other" to show custom input
    @State private var isCustomInputMode = false

    /// Track selections for each group in multi-group mode
    @State private var groupSelections: [String: Int] = [:]

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

            // Content based on type and custom input mode
            if request.clarificationType == .select && isCustomInputMode {
                // Show custom input when "Other" is selected
                customInputContent
            } else {
                // Show original content
                switch request.clarificationType {
                case .select:
                    selectContent
                case .text:
                    textContent
                case .multiGroup:
                    multiGroupContent
                }
            }

            // Source indicator (optional)
            if let source = request.source {
                sourceIndicator(source)
            }
        }
        .padding(16)
        .frame(minWidth: 200, maxWidth: 400)  // Wider for multi-group
        .background(backgroundColor.opacity(0.95))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .shadow(color: .black.opacity(0.2), radius: 8, x: 0, y: 4)
        .onAppear {
            initializeGroupSelections()
        }
    }

    // MARK: - Select Content

    private var selectContent: some View {
        VStack(spacing: 4) {
            // Display provided options
            if let options = request.options {
                ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                    optionRow(option: option, index: index, isSelected: manager.selectedIndex == index)
                        .onTapGesture {
                            selectOption(index: index, option: option)
                        }
                }
            }

            // Always add "Other" option for custom input
            let otherIndex = request.options?.count ?? 0
            otherOptionRow(index: otherIndex, isSelected: manager.selectedIndex == otherIndex)
                .onTapGesture {
                    selectOtherOption(index: otherIndex)
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

    /// Option row for "Other" custom input
    private func otherOptionRow(index: Int, isSelected: Bool) -> some View {
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
                Text(L("clarification.other", default: "Other (输入自定义)"))
                    .font(.system(size: 13, weight: isSelected ? .semibold : .regular))
                    .foregroundColor(isSelected ? accentColor : textColor)

                Text(L("clarification.other_description", default: "Enter your custom response"))
                    .font(.system(size: 11))
                    .foregroundColor(textColor.opacity(0.6))
            }

            Spacer()

            // Icon for custom input
            Image(systemName: "pencil.line")
                .font(.system(size: 10))
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

    /// Handle selection of "Other" option - switch to custom input mode
    private func selectOtherOption(index: Int) {
        manager.selectedIndex = index
        // Switch to custom input mode instead of auto-confirming
        withAnimation(.easeInOut(duration: 0.2)) {
            isCustomInputMode = true
        }
        // Focus on text input
        manager.textInput = ""
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

    // MARK: - Custom Input Content

    private var customInputContent: some View {
        VStack(spacing: 8) {
            // Back button
            HStack {
                Button(action: {
                    withAnimation(.easeInOut(duration: 0.2)) {
                        isCustomInputMode = false
                        manager.textInput = ""
                    }
                }) {
                    HStack(spacing: 4) {
                        Image(systemName: "chevron.left")
                            .font(.system(size: 10))
                        Text(L("clarification.back_to_options", default: "Back to options"))
                            .font(.system(size: 11))
                    }
                    .foregroundColor(textColor.opacity(0.6))
                }
                .buttonStyle(.plain)

                Spacer()
            }
            .padding(.bottom, 4)

            // Using IMETextField for proper Chinese/Japanese/Korean input
            IMETextField(
                text: $manager.textInput,
                placeholder: L("clarification.custom_placeholder", default: "Enter your response..."),
                font: .systemFont(ofSize: 14),
                textColor: .white,
                backgroundColor: NSColor.white.withAlphaComponent(0.05),
                onSubmit: { confirmCustomInput() },
                onEscape: {
                    withAnimation(.easeInOut(duration: 0.2)) {
                        isCustomInputMode = false
                        manager.textInput = ""
                    }
                }
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

                Text(L("clarification.esc_to_go_back", default: "Esc to go back"))
                    .font(.system(size: 10))
                    .foregroundColor(textColor.opacity(0.5))
            }
        }
    }

    private func confirmCustomInput() {
        guard !manager.textInput.isEmpty else { return }
        manager.completeWithText(manager.textInput)
    }

    // MARK: - Multi-Group Content

    private var multiGroupContent: some View {
        VStack(spacing: 16) {
            if let groups = request.groups {
                ForEach(Array(groups.enumerated()), id: \.offset) { _, group in
                    groupView(for: group)
                }

                // Confirm button
                Button(action: confirmMultiGroup) {
                    Text(L("clarification.confirm", default: "确认"))
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(.white)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                        .background(accentColor)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
                .disabled(!allGroupsSelected)
            }
        }
    }

    /// Single group view
    private func groupView(for group: QuestionGroup) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            // Group prompt
            Text(group.prompt)
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(textColor.opacity(0.8))

            // Options for this group
            ForEach(Array(group.options.enumerated()), id: \.offset) { index, option in
                groupOptionRow(
                    option: option,
                    index: index,
                    groupId: group.id,
                    isSelected: groupSelections[group.id] == index
                )
                .onTapGesture {
                    selectGroupOption(groupId: group.id, index: index)
                }
            }
        }
        .padding(10)
        .background(textColor.opacity(0.03))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    /// Option row for a group
    private func groupOptionRow(option: ClarificationOption, index: Int, groupId: String, isSelected: Bool) -> some View {
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
                    .font(.system(size: 12, weight: isSelected ? .medium : .regular))
                    .foregroundColor(isSelected ? accentColor : textColor)

                if let description = option.description {
                    Text(description)
                        .font(.system(size: 10))
                        .foregroundColor(textColor.opacity(0.5))
                }
            }

            Spacer()
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(isSelected ? accentColor.opacity(0.1) : Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle())
    }

    /// Initialize group selections with default values
    private func initializeGroupSelections() {
        guard request.clarificationType == .multiGroup, let groups = request.groups else {
            return
        }

        for group in groups {
            if groupSelections[group.id] == nil {
                // Use default index if provided, otherwise 0
                groupSelections[group.id] = Int(group.defaultIndex ?? 0)
            }
        }
    }

    /// Select an option in a group
    private func selectGroupOption(groupId: String, index: Int) {
        groupSelections[groupId] = index
    }

    /// Check if all groups have a selection
    private var allGroupsSelected: Bool {
        guard let groups = request.groups else { return false }
        return groups.allSatisfy { group in
            groupSelections[group.id] != nil
        }
    }

    /// Confirm multi-group selection
    private func confirmMultiGroup() {
        guard let groups = request.groups else { return }

        var answers: [String: String] = [:]
        for group in groups {
            if let selectedIndex = groupSelections[group.id],
               selectedIndex < group.options.count {
                let selectedOption = group.options[selectedIndex]
                answers[group.id] = selectedOption.value
            }
        }

        manager.completeWithMultiGroup(answers)
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
        // For custom input mode, only handle Escape
        if isCustomInputMode {
            if event.keyCode == 53 { // Escape
                withAnimation(.easeInOut(duration: 0.2)) {
                    isCustomInputMode = false
                    manager.textInput = ""
                }
                return true
            }
            return false
        }

        guard request.clarificationType == .select else {
            // For text mode, only handle Escape
            if event.keyCode == 53 { // Escape
                manager.cancel()
                return true
            }
            return false
        }

        // Calculate total options count (including "Other")
        let totalOptions = (request.options?.count ?? 0) + 1
        guard totalOptions > 0 else { return false }

        switch event.keyCode {
        case 125: // Down arrow
            let newIndex = min(manager.selectedIndex + 1, totalOptions - 1)
            manager.selectedIndex = newIndex
            return true

        case 126: // Up arrow
            let newIndex = max(manager.selectedIndex - 1, 0)
            manager.selectedIndex = newIndex
            return true

        case 36: // Return/Enter
            let index = manager.selectedIndex
            let providedOptionsCount = request.options?.count ?? 0

            if index < providedOptionsCount {
                // Regular option selected
                if let option = request.options?[index] {
                    manager.completeWithSelection(index: index, value: option.value)
                }
            } else {
                // "Other" option selected
                selectOtherOption(index: index)
            }
            return true

        case 53: // Escape
            manager.cancel()
            return true

        case 18...26: // Number keys 1-9
            let numberIndex = Int(event.keyCode) - 18
            if numberIndex < totalOptions {
                manager.selectedIndex = numberIndex
                let providedOptionsCount = request.options?.count ?? 0

                if numberIndex < providedOptionsCount {
                    // Regular option
                    if let option = request.options?[numberIndex] {
                        manager.completeWithSelection(index: numberIndex, value: option.value)
                    }
                } else {
                    // "Other" option
                    selectOtherOption(index: numberIndex)
                }
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
