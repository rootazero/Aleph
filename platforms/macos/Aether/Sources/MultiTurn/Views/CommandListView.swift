//
//  CommandListView.swift
//  Aether
//
//  Command and topic list components for unified window.
//

import SwiftUI

// MARK: - CommandListView

/// Command list display (for / prefix)
struct CommandListView: View {
    @Bindable var viewModel: UnifiedConversationViewModel
    let maxHeight: CGFloat

    var body: some View {
        VStack(spacing: 0) {
            if viewModel.commands.isEmpty {
                Text(NSLocalizedString("commands.empty", comment: ""))
                    .font(.subheadline)
                    .liquidGlassSecondaryText()
                    .padding()
                    .onAppear {
                        viewModel.reportHeightChange(60)  // Report empty state height
                    }
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(spacing: 0) {
                            ForEach(Array(viewModel.commands.enumerated()), id: \.element.key) { index, command in
                                CommandRowView(
                                    command: command,
                                    isSelected: index == viewModel.selectedCommandIndex
                                ) {
                                    viewModel.selectCommand(command)
                                }
                                .id("cmd-\(index)")
                            }
                        }
                        .background(
                            GeometryReader { geometry in
                                Color.clear
                                    .onChange(of: geometry.size.height) { _, newHeight in
                                        viewModel.reportHeightChange(newHeight)
                                    }
                                    .onAppear {
                                        DispatchQueue.main.async {
                                            viewModel.reportHeightChange(geometry.size.height)
                                        }
                                    }
                            }
                        )
                    }
                    .frame(maxHeight: maxHeight)
                    .onChange(of: viewModel.selectedCommandIndex) { _, newIndex in
                        withAnimation(.easeInOut(duration: 0.15)) {
                            proxy.scrollTo("cmd-\(newIndex)", anchor: nil)
                        }
                    }
                }
            }
        }
        .onChange(of: viewModel.commands.count) { _, newCount in
            // Report height when command count changes
            let estimatedHeight = max(CGFloat(newCount) * 44 + 20, 60)
            viewModel.reportHeightChange(min(estimatedHeight, maxHeight))
        }
    }
}

// MARK: - TopicListView

/// Topic list display (for // prefix)
struct TopicListView: View {
    @Bindable var viewModel: UnifiedConversationViewModel
    let maxHeight: CGFloat

    var body: some View {
        VStack(spacing: 0) {
            if viewModel.filteredTopics.isEmpty {
                Text(NSLocalizedString("topics.empty", comment: ""))
                    .font(.subheadline)
                    .liquidGlassSecondaryText()
                    .padding()
                    .onAppear {
                        viewModel.reportHeightChange(60)  // Report empty state height
                    }
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(spacing: 0) {
                            ForEach(Array(viewModel.filteredTopics.enumerated()), id: \.element.id) { index, topic in
                                TopicRowView(
                                    topic: topic,
                                    isSelected: index == viewModel.selectedTopicIndex,
                                    onSelect: { viewModel.selectTopic(topic) },
                                    onDelete: { viewModel.deleteTopic(topic) },
                                    onRename: { viewModel.renameTopic(topic, newTitle: $0) }
                                )
                                .id("topic-\(index)")
                            }
                        }
                        .background(
                            GeometryReader { geometry in
                                Color.clear
                                    .onChange(of: geometry.size.height) { _, newHeight in
                                        viewModel.reportHeightChange(newHeight)
                                    }
                                    .onAppear {
                                        DispatchQueue.main.async {
                                            viewModel.reportHeightChange(geometry.size.height)
                                        }
                                    }
                            }
                        )
                    }
                    .frame(maxHeight: maxHeight)
                    .onChange(of: viewModel.selectedTopicIndex) { _, newIndex in
                        withAnimation(.easeInOut(duration: 0.15)) {
                            proxy.scrollTo("topic-\(newIndex)", anchor: nil)
                        }
                    }
                }
            }
        }
        .onChange(of: viewModel.filteredTopics.count) { _, newCount in
            // Report height when topic count changes
            let estimatedHeight = max(CGFloat(newCount) * 44 + 20, 60)
            viewModel.reportHeightChange(min(estimatedHeight, maxHeight))
        }
    }
}

// MARK: - CommandRowView

/// A single command row in the command list
struct CommandRowView: View {
    let command: CommandNode
    let isSelected: Bool
    let onSelect: () -> Void

    var body: some View {
        Button(action: onSelect) {
            HStack(spacing: 8) {
                Image(systemName: command.icon)
                    .font(.system(size: 16))
                    .frame(width: 24)
                    .liquidGlassSecondaryText()

                VStack(alignment: .leading, spacing: 2) {
                    Text("/\(command.key)")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(.primary)

                    Text(command.description)
                        .font(.system(size: 11))
                        .liquidGlassSecondaryText()
                        .lineLimit(1)
                }

                Spacer()

                if command.hasChildren {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .liquidGlassSecondaryText()
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(isSelected ? Color.accentColor.opacity(0.15) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - TopicRowView

/// A single topic row in the topic list with swipe actions
struct TopicRowView: View {
    let topic: Topic
    let isSelected: Bool
    let onSelect: () -> Void
    let onDelete: () -> Void
    let onRename: (String) -> Void

    @State private var isEditing = false
    @State private var editedTitle = ""

    var body: some View {
        Button(action: onSelect) {
            HStack(spacing: 8) {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 14))
                    .liquidGlassSecondaryText()
                    .frame(width: 24)

                if isEditing {
                    TextField("", text: $editedTitle, onCommit: {
                        onRename(editedTitle)
                        isEditing = false
                    })
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .onExitCommand {
                        isEditing = false
                    }
                } else {
                    Text(topic.title)
                        .font(.system(size: 13))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                }

                Spacer()

                Text(topic.updatedAt.formatted(.relative(presentation: .named)))
                    .font(.system(size: 10))
                    .liquidGlassSecondaryText()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(isSelected ? Color.accentColor.opacity(0.15) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button {
                editedTitle = topic.title
                isEditing = true
            } label: {
                Label(NSLocalizedString("topic.rename", comment: ""), systemImage: "pencil")
            }

            Button(role: .destructive) {
                onDelete()
            } label: {
                Label(NSLocalizedString("topic.delete", comment: ""), systemImage: "trash")
            }
        }
    }
}
