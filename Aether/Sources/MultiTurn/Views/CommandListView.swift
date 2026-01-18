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
                    .foregroundColor(.primary.opacity(0.7))
                    .padding()
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
                    .foregroundColor(.primary.opacity(0.7))
                    .padding()
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
    }
}
