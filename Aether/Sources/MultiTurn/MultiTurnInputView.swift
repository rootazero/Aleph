//
//  MultiTurnInputView.swift
//  Aether
//
//  SwiftUI view for multi-turn input window.
//

import Combine
import SwiftUI

// MARK: - MultiTurnInputViewModel

/// View model for multi-turn input
final class MultiTurnInputViewModel: ObservableObject {

    // MARK: - Published Properties

    @Published var inputText: String = ""
    @Published var turnCount: Int = 0
    @Published var showTopicList: Bool = false
    @Published var topics: [Topic] = []
    @Published var filteredTopics: [Topic] = []

    // MARK: - Callbacks

    var onSubmit: ((String) -> Void)?
    var onCancel: (() -> Void)?
    var onTopicSelected: ((Topic) -> Void)?

    // MARK: - Focus Control

    @Published var shouldFocusInput: Bool = false

    func focusInput() {
        shouldFocusInput = true
    }

    // MARK: - Actions

    func handleInputChange(_ newValue: String) {
        inputText = newValue

        // Check for // command
        if newValue.hasPrefix("//") {
            showTopicList = true
            loadTopics()
            filterTopics(query: String(newValue.dropFirst(2)))
        } else {
            showTopicList = false
        }
    }

    func submit() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !text.hasPrefix("//") else { return }

        onSubmit?(text)
        inputText = ""
    }

    func cancel() {
        onCancel?()
    }

    func selectTopic(_ topic: Topic) {
        onTopicSelected?(topic)
        inputText = ""
        showTopicList = false
    }

    func reset() {
        inputText = ""
        turnCount = 0
        showTopicList = false
        topics = []
        filteredTopics = []
    }

    // MARK: - Topic Loading

    private func loadTopics() {
        topics = ConversationStore.shared.getAllTopics()
        filteredTopics = topics
    }

    private func filterTopics(query: String) {
        if query.isEmpty {
            filteredTopics = topics
        } else {
            filteredTopics = topics.filter { topic in
                topic.title.localizedCaseInsensitiveContains(query)
            }
        }
    }
}

// MARK: - MultiTurnInputView

/// SwiftUI view for multi-turn input
struct MultiTurnInputView: View {
    @ObservedObject var viewModel: MultiTurnInputViewModel
    @FocusState private var isInputFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            // Input field
            inputField

            // Topic list (when showing)
            if viewModel.showTopicList {
                topicList
            }
        }
        .background(
            VisualEffectBackground(material: .hudWindow, blendingMode: .behindWindow)
        )
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .onChange(of: viewModel.shouldFocusInput) { _, shouldFocus in
            if shouldFocus {
                isInputFocused = true
                viewModel.shouldFocusInput = false
            }
        }
    }

    // MARK: - Input Field

    private var inputField: some View {
        HStack(spacing: 12) {
            // Turn indicator
            if viewModel.turnCount > 0 {
                Text("Turn \(viewModel.turnCount + 1)")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(Color.purple.opacity(0.1))
                    .cornerRadius(4)
            }

            // Text field
            TextField("Type a message... (// for topics)", text: $viewModel.inputText)
                .textFieldStyle(.plain)
                .font(.system(size: 16))
                .focused($isInputFocused)
                .onChange(of: viewModel.inputText) { _, newValue in
                    viewModel.handleInputChange(newValue)
                }
                .onSubmit {
                    viewModel.submit()
                }

            // Submit button
            Button(action: viewModel.submit) {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.system(size: 24))
                    .foregroundColor(.purple)
            }
            .buttonStyle(.plain)
            .disabled(viewModel.inputText.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .padding(16)
    }

    // MARK: - Topic List

    private var topicList: some View {
        VStack(spacing: 0) {
            Divider()

            if viewModel.filteredTopics.isEmpty {
                Text("No topics found")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                    .padding()
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(viewModel.filteredTopics) { topic in
                            TopicRowView(topic: topic) {
                                viewModel.selectTopic(topic)
                            }
                        }
                    }
                }
                .frame(maxHeight: 300)
            }
        }
    }
}

// MARK: - TopicRowView

/// Row view for topic list
struct TopicRowView: View {
    let topic: Topic
    let onSelect: () -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: onSelect) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(topic.title)
                        .font(.system(size: 14))
                        .lineLimit(1)

                    Text(formatDate(topic.updatedAt))
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                Spacer()

                Image(systemName: "chevron.right")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(isHovering ? Color.purple.opacity(0.1) : Color.clear)
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHovering = hovering
        }
    }

    private func formatDate(_ date: Date) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }
}
