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
    @Published var showCommandList: Bool = false
    @Published var topics: [Topic] = []
    @Published var filteredTopics: [Topic] = []
    @Published var commands: [CommandNode] = []
    @Published var selectedCommandIndex: Int = 0

    // MARK: - Callbacks

    var onSubmit: ((String) -> Void)?
    var onCancel: (() -> Void)?
    var onTopicSelected: ((Topic) -> Void)?

    // MARK: - Focus Control

    @Published var shouldFocusInput: Bool = false

    func focusInput() {
        shouldFocusInput = true
    }

    // MARK: - Core Reference

    private var core: AetherCore? {
        (NSApplication.shared.delegate as? AppDelegate)?.core
    }

    // MARK: - Actions

    func handleInputChange(_ newValue: String) {
        inputText = newValue

        // Check for // command (topic list)
        if newValue.hasPrefix("//") {
            showTopicList = true
            showCommandList = false
            loadTopics()
            filterTopics(query: String(newValue.dropFirst(2)))
        }
        // Check for / command (command completion)
        else if newValue.hasPrefix("/") && !newValue.hasPrefix("//") {
            showTopicList = false
            showCommandList = true
            loadCommands(prefix: String(newValue.dropFirst()))
        } else {
            showTopicList = false
            showCommandList = false
        }
    }

    func submit() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !text.hasPrefix("//") else { return }

        // If command list is showing, complete the selected command
        if showCommandList, selectedCommandIndex < commands.count {
            let command = commands[selectedCommandIndex]
            inputText = "/\(command.key) "
            showCommandList = false
            return
        }

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

    func selectCommand(_ command: CommandNode) {
        inputText = "/\(command.key) "
        showCommandList = false
    }

    func reset() {
        inputText = ""
        turnCount = 0
        showTopicList = false
        showCommandList = false
        topics = []
        filteredTopics = []
        commands = []
        selectedCommandIndex = 0
    }

    // MARK: - Navigation

    func moveSelectionUp() {
        if showCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex - 1 + commands.count) % commands.count
        }
    }

    func moveSelectionDown() {
        if showCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex + 1) % commands.count
        }
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

    // MARK: - Command Loading

    private func loadCommands(prefix: String) {
        guard let core = core else {
            print("[MultiTurnInputViewModel] Core not available")
            commands = []
            return
        }

        let allCommands = core.getRootCommandsFromRegistry()
        if prefix.isEmpty {
            commands = allCommands
        } else {
            let lowercasedPrefix = prefix.lowercased()
            commands = allCommands.filter {
                $0.key.lowercased().hasPrefix(lowercasedPrefix) ||
                $0.description.lowercased().contains(lowercasedPrefix)
            }
        }
        selectedCommandIndex = 0
        print("[MultiTurnInputViewModel] Loaded \(commands.count) commands for prefix '\(prefix)'")
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

            // Command list (when showing)
            if viewModel.showCommandList {
                commandList
            }

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
            TextField("Type a message... (/ for commands, // for topics)", text: $viewModel.inputText)
                .textFieldStyle(.plain)
                .font(.system(size: 16))
                .focused($isInputFocused)
                .onChange(of: viewModel.inputText) { _, newValue in
                    viewModel.handleInputChange(newValue)
                }
                .onSubmit {
                    viewModel.submit()
                }
                .onKeyPress(.upArrow) {
                    viewModel.moveSelectionUp()
                    return .handled
                }
                .onKeyPress(.downArrow) {
                    viewModel.moveSelectionDown()
                    return .handled
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

    // MARK: - Command List

    private var commandList: some View {
        VStack(spacing: 0) {
            Divider()

            if viewModel.commands.isEmpty {
                Text("No commands found")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                    .padding()
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(viewModel.commands.enumerated()), id: \.element.key) { index, command in
                            CommandRowView(
                                command: command,
                                isSelected: index == viewModel.selectedCommandIndex
                            ) {
                                viewModel.selectCommand(command)
                            }
                        }
                    }
                }
                .frame(maxHeight: 300)
            }
        }
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

// MARK: - CommandRowView

/// Row view for command list
struct CommandRowView: View {
    let command: CommandNode
    let isSelected: Bool
    let onSelect: () -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: onSelect) {
            HStack(spacing: 12) {
                // Command icon
                Image(systemName: command.icon.isEmpty ? "terminal" : command.icon)
                    .font(.system(size: 14))
                    .foregroundColor(.purple)
                    .frame(width: 20)

                // Command info
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text("/\(command.key)")
                            .font(.system(size: 14, weight: .medium))

                        // Source badge
                        Text(sourceTypeName(command.sourceType))
                            .font(.system(size: 9))
                            .padding(.horizontal, 4)
                            .padding(.vertical, 1)
                            .background(Color.purple.opacity(0.2))
                            .cornerRadius(3)
                    }

                    if !command.description.isEmpty {
                        Text(command.description)
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                    }
                }

                Spacer()

                Image(systemName: "chevron.right")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background((isHovering || isSelected) ? Color.purple.opacity(0.15) : Color.clear)
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHovering = hovering
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

// MARK: - Helper Functions

/// Convert ToolSourceType enum to display string
private func sourceTypeName(_ type: ToolSourceType) -> String {
    switch type {
    case .native: return "Native"
    case .builtin: return "System"
    case .mcp: return "MCP"
    case .skill: return "Skill"
    case .custom: return "Custom"
    }
}
