//
//  CommandListView.swift
//  Aether
//
//  Command completion list for Halo overlay.
//  Displays available commands with icons, descriptions, and hints.
//

import SwiftUI

/// Single command row in the command list
struct CommandRow: View {
    let command: CommandNode
    let isSelected: Bool

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        HStack(spacing: 8) {
            // Command icon
            Image(systemName: command.icon)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(isSelected ? .white : command.typeColor)
                .frame(width: 18)

            // Command key
            Text("/\(command.key)")
                .font(.system(size: 13, weight: .semibold, design: .monospaced))
                .foregroundColor(isSelected ? .white : .primary)
                .fixedSize(horizontal: true, vertical: false)

            // Hint (if available) - flexible width
            if let hint = command.hint, !hint.isEmpty {
                Text(hint)
                    .font(.system(size: 11))
                    .foregroundColor(isSelected ? .white.opacity(0.7) : .secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }

            Spacer(minLength: 4)

            // Namespace indicator
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
                .fill(isSelected ? Color.accentColor : Color.clear)
        )
        .contentShape(Rectangle())
    }
}

/// Command completion list view
struct CommandListView: View {
    @ObservedObject var manager: CommandCompletionManager
    let maxHeight: CGFloat

    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header - compact design
            HStack(spacing: 6) {
                Image(systemName: "command")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(.secondary)
                Text(NSLocalizedString("command.mode.title", comment: "Command Mode"))
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(.secondary)
                Spacer()
                // Simplified hints
                Text("↑↓  ⏎  ⎋")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundColor(.secondary.opacity(0.6))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(Color.secondary.opacity(0.08))

            Divider()

            // Command list
            if manager.displayedCommands.isEmpty {
                // Empty state
                VStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 24))
                        .foregroundColor(.secondary)
                    Text(NSLocalizedString("command.mode.no_results", comment: "No matching commands"))
                        .font(.system(size: 13))
                        .foregroundColor(.secondary)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 24)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 2) {
                            ForEach(0..<manager.displayedCommands.count, id: \.self) { index in
                                let command = manager.displayedCommands[index]
                                CommandRow(
                                    command: command,
                                    isSelected: index == manager.selectedIndex
                                )
                                .id("\(manager.inputPrefix)-\(index)")
                                .onTapGesture {
                                    manager.selectedIndex = index
                                    manager.selectCurrentCommand()
                                }
                            }
                        }
                        .padding(.vertical, 4)
                    }
                    .frame(maxHeight: maxHeight - 50) // Account for header
                    .onChange(of: manager.selectedIndex) { _, newIndex in
                        withAnimation(.easeInOut(duration: 0.15)) {
                            proxy.scrollTo("\(manager.inputPrefix)-\(newIndex)", anchor: .center)
                        }
                    }
                }
                // Force view recreation when filter changes
                .id(manager.inputPrefix)
            }
        }
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(colorScheme == .dark ? Color(white: 0.15) : Color.white)
                .shadow(color: .black.opacity(0.2), radius: 10, y: 4)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color.secondary.opacity(0.2), lineWidth: 1)
        )
    }
}

// MARK: - Preview

#Preview {
    let manager = CommandCompletionManager()
    return CommandListView(manager: manager, maxHeight: 300)
        .frame(width: 320)
        .padding()
}
