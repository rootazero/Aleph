//
//  HaloCommandListView.swift
//  Aether
//
//  Command list panel view for the / command.
//  Displays available skills/commands with search filtering.
//

import SwiftUI

/// Command list panel for navigating available skills
struct HaloCommandListView: View {
    @Binding var context: CommandListContext
    let onSelect: (CommandItem) -> Void
    let onDismiss: () -> Void

    @State private var isAppearing = false

    var body: some View {
        VStack(spacing: 0) {
            headerView
            searchFieldView
            commandListView
        }
        .frame(width: 360, height: 380)
        .background(.ultraThinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .scaleEffect(isAppearing ? 1.0 : 0.95)
        .opacity(isAppearing ? 1.0 : 0.0)
        .onAppear {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                isAppearing = true
            }
        }
    }

    // MARK: - Header

    private var headerView: some View {
        HStack(spacing: 10) {
            Image(systemName: "command.circle.fill")
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(.blue)

            Text(L("commands.title"))
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)

            Spacer()

            Button(action: dismissWithAnimation) {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 18))
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    // MARK: - Search Field

    private var searchFieldView: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 12))
                .foregroundColor(.secondary)

            TextField(L("commands.search"), text: $context.searchQuery)
                .textFieldStyle(.plain)
                .font(.system(size: 13))

            if !context.searchQuery.isEmpty {
                Button(action: { context.searchQuery = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(Color.primary.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
    }

    // MARK: - Command List

    private var commandListView: some View {
        Group {
            if context.filteredCommands.isEmpty {
                emptyStateView
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(context.filteredCommands.enumerated()), id: \.element.id) { index, command in
                            CommandRow(
                                command: command,
                                isSelected: context.selectedIndex == index
                            )
                            .onTapGesture {
                                onSelect(command)
                            }
                        }
                    }
                }
            }
        }
    }

    private var emptyStateView: some View {
        VStack(spacing: 12) {
            Spacer()
            Image(systemName: "command")
                .font(.system(size: 32))
                .foregroundColor(.secondary.opacity(0.5))
            Text(L("commands.empty"))
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: - Actions

    private func dismissWithAnimation() {
        withAnimation(.easeOut(duration: 0.2)) {
            isAppearing = false
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            onDismiss()
        }
    }
}

// MARK: - Command Row

private struct CommandRow: View {
    let command: CommandItem
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            // Command icon
            Image(systemName: command.icon)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(.blue)
                .frame(width: 24)

            // Command name and description
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 0) {
                    Text("/")
                        .font(.system(size: 13, weight: .medium, design: .monospaced))
                        .foregroundColor(.secondary)
                    Text(command.name)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(.primary)
                }

                Text(command.description)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }

            Spacer()

            // Return key hint
            Text("\u{21B5}")
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(.secondary.opacity(0.5))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(
            isSelected ? Color.accentColor.opacity(0.1) : Color.clear
        )
        .contentShape(Rectangle())
    }
}

// MARK: - Preview

#if DEBUG
struct HaloCommandListView_Previews: PreviewProvider {
    static var previews: some View {
        // Sample commands for preview
        let sampleCommands = [
            CommandItem(
                id: "commit",
                name: "commit",
                description: "Create a git commit with AI-generated message",
                icon: "arrow.up.doc.fill"
            ),
            CommandItem(
                id: "review-pr",
                name: "review-pr",
                description: "Review a pull request",
                icon: "doc.text.magnifyingglass"
            ),
            CommandItem(
                id: "explain",
                name: "explain",
                description: "Explain the selected code or concept",
                icon: "lightbulb.fill"
            ),
            CommandItem(
                id: "refactor",
                name: "refactor",
                description: "Suggest refactoring improvements",
                icon: "arrow.triangle.2.circlepath"
            ),
            CommandItem(
                id: "test",
                name: "test",
                description: "Generate unit tests for the code",
                icon: "checkmark.seal.fill"
            )
        ]

        Group {
            // Normal state with commands
            HaloCommandListView(
                context: .constant(CommandListContext(commands: sampleCommands)),
                onSelect: { command in print("Selected: \(command.name)") },
                onDismiss: { print("Dismissed") }
            )
            .previewDisplayName("With Commands")

            // Empty state
            HaloCommandListView(
                context: .constant(CommandListContext(commands: [])),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("Empty State")

            // With search query
            HaloCommandListView(
                context: .constant(CommandListContext(commands: sampleCommands, searchQuery: "commit")),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("With Search")

            // With selected item
            HaloCommandListView(
                context: .constant(CommandListContext(commands: sampleCommands, selectedIndex: 2)),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("With Selection")
        }
        .padding(40)
        .background(Color.gray.opacity(0.2))
    }
}
#endif
