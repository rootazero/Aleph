//
//  SubPanelView.swift
//  Aether
//
//  Main container view for the SubPanel component.
//  A Raycast-style expandable area below the main input field in unified Halo.
//
//  Part of: refactor-unified-halo-window
//

import SwiftUI

// MARK: - SubPanelView

/// The main SubPanel container that displays different content based on mode
///
/// Design Features:
/// - Dynamic height with smooth spring animation
/// - Frosted glass background (ultraThinMaterial)
/// - Subtle shadow for depth
/// - Top divider when visible
/// - Keyboard hints footer
struct SubPanelView: View {
    @ObservedObject var state: SubPanelState

    /// Callback when command is selected
    var onCommandSelected: ((CommandNode) -> Void)?

    /// Callback when selector options are confirmed
    var onSelectorConfirmed: (([SelectorOption]) -> Void)?

    /// Callback when confirmation is accepted
    var onConfirmationAccepted: (() -> Void)?

    /// Callback when cancelled (applies to all modes)
    var onCancelled: (() -> Void)?

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(spacing: 0) {
            // Top divider (only when visible)
            if state.mode.isVisible {
                Divider()
                    .background(Color.secondary.opacity(0.3))
            }

            // Content area with dynamic height
            contentView
                .frame(height: state.calculatedHeight)
                .clipped()
        }
        .background(backgroundView)
        .animation(.spring(response: 0.3, dampingFraction: 0.8), value: state.mode)
    }

    // MARK: - Content View

    @ViewBuilder
    private var contentView: some View {
        switch state.mode {
        case .hidden:
            EmptyView()

        case .commandCompletion(let commands, let selectedIndex, _):
            SubPanelCommandList(
                commands: commands,
                selectedIndex: selectedIndex,
                onSelect: { command in
                    onCommandSelected?(command)
                }
            )

        case .selector(let options, let prompt, let multiSelect):
            SubPanelSelector(
                options: options,
                prompt: prompt,
                multiSelect: multiSelect,
                onToggle: { index in
                    state.toggleSelectorOption(at: index)
                },
                onConfirm: {
                    onSelectorConfirmed?(state.getSelectedOptions())
                },
                onCancel: {
                    onCancelled?()
                }
            )

        case .cliOutput(let lines, let isStreaming):
            SubPanelCLIOutput(
                lines: lines,
                isStreaming: isStreaming
            )

        case .confirmation(let title, let message, let confirmLabel, let cancelLabel):
            SubPanelConfirmation(
                title: title,
                message: message,
                confirmLabel: confirmLabel,
                cancelLabel: cancelLabel,
                onConfirm: {
                    onConfirmationAccepted?()
                },
                onCancel: {
                    onCancelled?()
                }
            )
        }
    }

    // MARK: - Background

    @ViewBuilder
    private var backgroundView: some View {
        if state.mode.isVisible {
            RoundedRectangle(cornerRadius: 8)
                .fill(.ultraThinMaterial)
                .shadow(
                    color: .black.opacity(colorScheme == .dark ? 0.3 : 0.15),
                    radius: 6,
                    x: 0,
                    y: 4
                )
        }
    }
}

// MARK: - Command List

/// Command completion list for SubPanel
struct SubPanelCommandList: View {
    let commands: [CommandNode]
    let selectedIndex: Int
    let onSelect: (CommandNode) -> Void

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack(spacing: 6) {
                Image(systemName: "command")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(.secondary)
                Text(L("command.mode.title"))
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(.secondary)
                Spacer()
                // Keyboard hints
                Text("↑↓  ⏎  ⎋")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundColor(.secondary.opacity(0.6))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(Color.secondary.opacity(0.08))

            Divider()

            // Command list
            if commands.isEmpty {
                emptyStateView
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 2) {
                            ForEach(Array(commands.enumerated()), id: \.offset) { index, command in
                                SubPanelCommandRow(
                                    command: command,
                                    isSelected: index == selectedIndex,
                                    index: index
                                )
                                .id(index)
                                .onTapGesture {
                                    onSelect(command)
                                }
                            }
                        }
                        .padding(.vertical, 4)
                    }
                    .onChange(of: selectedIndex) { _, newIndex in
                        withAnimation(.easeInOut(duration: 0.15)) {
                            proxy.scrollTo(newIndex, anchor: .center)
                        }
                    }
                }
            }
        }
    }

    private var emptyStateView: some View {
        VStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 24))
                .foregroundColor(.secondary)
            Text(L("command.mode.no_results"))
                .font(.system(size: 13))
                .foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 24)
    }
}

/// Single command row in SubPanel
struct SubPanelCommandRow: View {
    let command: CommandNode
    let isSelected: Bool
    let index: Int

    private var rowBackground: Color {
        if isSelected {
            return Color.accentColor
        }
        return index % 2 == 0 ? Color.clear : Color.secondary.opacity(0.06)
    }

    var body: some View {
        HStack(spacing: 8) {
            // Icon
            Image(systemName: "text.quote")
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(isSelected ? .white : .secondary)
                .frame(width: 18)

            // Command key
            Text("/\(command.key)")
                .font(.system(size: 13, weight: .semibold, design: .monospaced))
                .foregroundColor(isSelected ? .white : .primary)
                .fixedSize(horizontal: true, vertical: false)

            // Hint
            if let hint = command.hint, !hint.isEmpty {
                Text(hint)
                    .font(.system(size: 11))
                    .foregroundColor(isSelected ? .white.opacity(0.7) : .secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }

            Spacer(minLength: 4)

            // Chevron for namespaces
            if command.hasChildren {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundColor(isSelected ? .white.opacity(0.6) : .secondary)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(
            RoundedRectangle(cornerRadius: 6)
                .fill(rowBackground)
        )
        .contentShape(Rectangle())
    }
}

// MARK: - Selector

/// Selector view for SubPanel (yes/no, multiple choice)
struct SubPanelSelector: View {
    let options: [SelectorOption]
    let prompt: String
    let multiSelect: Bool
    let onToggle: (Int) -> Void
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            // Prompt
            Text(prompt)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(.primary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 12)
                .padding(.top, 12)

            // Options
            ScrollView {
                VStack(spacing: 4) {
                    ForEach(Array(options.enumerated()), id: \.element.id) { index, option in
                        SelectorOptionRow(
                            option: option,
                            multiSelect: multiSelect,
                            onTap: { onToggle(index) }
                        )
                    }
                }
                .padding(.horizontal, 8)
            }

            // Action buttons
            HStack(spacing: 12) {
                Button(action: onCancel) {
                    Text(L("button.cancel"))
                        .font(.system(size: 13))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .background(Color.secondary.opacity(0.2))
                .cornerRadius(6)

                Button(action: onConfirm) {
                    Text(L("button.confirm"))
                        .font(.system(size: 13, weight: .medium))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .background(Color.accentColor)
                .foregroundColor(.white)
                .cornerRadius(6)
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 12)
        }
    }
}

/// Single option row in selector
struct SelectorOptionRow: View {
    let option: SelectorOption
    let multiSelect: Bool
    let onTap: () -> Void

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 10) {
                // Selection indicator
                Image(systemName: option.isSelected ?
                      (multiSelect ? "checkmark.square.fill" : "checkmark.circle.fill") :
                      (multiSelect ? "square" : "circle"))
                    .font(.system(size: 18))
                    .foregroundColor(option.isSelected ? .accentColor : .secondary)

                // Icon (if any)
                if let iconName = option.iconName {
                    Image(systemName: iconName)
                        .font(.system(size: 14))
                        .foregroundColor(.secondary)
                }

                // Label and description
                VStack(alignment: .leading, spacing: 2) {
                    Text(option.label)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(.primary)

                    if let desc = option.description {
                        Text(desc)
                            .font(.system(size: 11))
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                    }
                }

                Spacer()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(option.isSelected ? Color.accentColor.opacity(0.1) : Color.clear)
            )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - CLI Output

/// CLI output view for SubPanel (terminal-like display)
struct SubPanelCLIOutput: View {
    let lines: [CLIOutputLine]
    let isStreaming: Bool

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(lines) { line in
                        CLIOutputLineView(line: line)
                            .id(line.id)
                    }

                    // Streaming indicator
                    if isStreaming {
                        HStack(spacing: 4) {
                            ProgressView()
                                .scaleEffect(0.6)
                            Text(L("cli.processing"))
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundColor(.secondary)
                        }
                        .id("streaming-indicator")
                    }
                }
                .padding(10)
            }
            .onChange(of: lines.count) { _, _ in
                // Auto-scroll to bottom
                withAnimation(.easeOut(duration: 0.1)) {
                    if isStreaming {
                        proxy.scrollTo("streaming-indicator", anchor: .bottom)
                    } else if let lastLine = lines.last {
                        proxy.scrollTo(lastLine.id, anchor: .bottom)
                    }
                }
            }
        }
        .font(.system(size: 11, design: .monospaced))
    }
}

/// Single CLI output line
struct CLIOutputLineView: View {
    let line: CLIOutputLine

    private var timeString: String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter.string(from: line.timestamp)
    }

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            // Timestamp
            Text(timeString)
                .foregroundColor(.secondary.opacity(0.5))
                .frame(width: 55, alignment: .leading)

            // Type indicator
            typeIndicator
                .frame(width: 8)

            // Content
            Text(line.content)
                .foregroundColor(line.type.color)
                .fontWeight(line.type.fontWeight)
                .textSelection(.enabled)
        }
    }

    @ViewBuilder
    private var typeIndicator: some View {
        switch line.type {
        case .success:
            Text("✓")
                .foregroundColor(.green)
        case .error:
            Text("✗")
                .foregroundColor(.red)
        case .warning:
            Text("!")
                .foregroundColor(.orange)
        case .command:
            Text("$")
                .foregroundColor(.cyan)
        case .thinking:
            Text("…")
                .foregroundColor(.secondary)
        default:
            Text("›")
                .foregroundColor(.secondary)
        }
    }
}

// MARK: - Confirmation

/// Confirmation dialog for SubPanel
struct SubPanelConfirmation: View {
    let title: String
    let message: String
    let confirmLabel: String
    let cancelLabel: String
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            // Title
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(.primary)
                .padding(.top, 16)

            // Message
            Text(message)
                .font(.system(size: 13))
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 16)

            // Buttons
            HStack(spacing: 12) {
                Button(action: onCancel) {
                    Text(cancelLabel)
                        .font(.system(size: 13))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .background(Color.secondary.opacity(0.2))
                .cornerRadius(6)
                .keyboardShortcut(.escape, modifiers: [])

                Button(action: onConfirm) {
                    Text(confirmLabel)
                        .font(.system(size: 13, weight: .medium))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .background(Color.accentColor)
                .foregroundColor(.white)
                .cornerRadius(6)
                .keyboardShortcut(.return, modifiers: [])
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 16)
        }
    }
}

// MARK: - Localization Helper

/// Localization helper with fallback
private func L(_ key: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    return localized == key ? fallbackString(for: key) : localized
}

/// Fallback strings for missing localizations
private func fallbackString(for key: String) -> String {
    switch key {
    case "command.mode.title": return "Commands"
    case "command.mode.no_results": return "No commands found"
    case "button.cancel": return "Cancel"
    case "button.confirm": return "Confirm"
    case "cli.processing": return "Processing..."
    default: return key
    }
}

// MARK: - Previews

#if DEBUG
struct SubPanelView_Previews: PreviewProvider {
    static var previews: some View {
        let state = SubPanelState()

        VStack(spacing: 20) {
            // Command completion preview
            SubPanelView(state: {
                let s = SubPanelState()
                // Note: Can't preview with actual CommandNode without Rust core
                return s
            }())
            .frame(width: 400)
            .previewDisplayName("Command Completion")

            // CLI Output preview
            SubPanelView(state: {
                let s = SubPanelState()
                s.showCLIOutput(initialLines: [
                    CLIOutputLine(type: .command, content: "Searching for information..."),
                    CLIOutputLine(type: .info, content: "Found 3 relevant documents"),
                    CLIOutputLine(type: .success, content: "Analysis complete")
                ])
                return s
            }())
            .frame(width: 400)
            .previewDisplayName("CLI Output")

            // Confirmation preview
            SubPanelView(state: {
                let s = SubPanelState()
                s.showConfirmation(
                    title: "Confirm Action",
                    message: "Are you sure you want to proceed with this action?",
                    confirmLabel: "Yes, Proceed",
                    cancelLabel: "Cancel"
                )
                return s
            }())
            .frame(width: 400)
            .previewDisplayName("Confirmation")
        }
        .padding()
        .background(Color.black.opacity(0.8))
    }
}
#endif
