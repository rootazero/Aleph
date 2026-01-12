//
//  MultiTurnInputView.swift
//  Aether
//
//  SwiftUI view for multi-turn input window.
//

import AppKit
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
    @Published var selectedTopicIndex: Int = 0

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
        // Note: inputText is already updated via IMETextField binding
        // We only need to handle command/topic detection here

        print("[MultiTurnInputViewModel] handleInputChange: '\(newValue)'")

        // Check for // command (topic list)
        if newValue.hasPrefix("//") {
            showTopicList = true
            showCommandList = false
            loadTopics()
            filterTopics(query: String(newValue.dropFirst(2)))
            print("[MultiTurnInputViewModel] Showing topic list")
        }
        // Check for / command (command completion)
        else if newValue.hasPrefix("/") && !newValue.hasPrefix("//") {
            showTopicList = false
            showCommandList = true
            loadCommands(prefix: String(newValue.dropFirst()))
            print("[MultiTurnInputViewModel] Showing command list")
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
        selectedTopicIndex = 0
    }

    // MARK: - Navigation

    func moveSelectionUp() {
        if showCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex - 1 + commands.count) % commands.count
        } else if showTopicList && !filteredTopics.isEmpty {
            selectedTopicIndex = (selectedTopicIndex - 1 + filteredTopics.count) % filteredTopics.count
        }
    }

    func moveSelectionDown() {
        if showCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex + 1) % commands.count
        } else if showTopicList && !filteredTopics.isEmpty {
            selectedTopicIndex = (selectedTopicIndex + 1) % filteredTopics.count
        }
    }

    // MARK: - Topic Loading

    private func loadTopics() {
        topics = ConversationStore.shared.getAllTopics()
        filteredTopics = topics
        selectedTopicIndex = 0
    }

    private func filterTopics(query: String) {
        if query.isEmpty {
            filteredTopics = topics
        } else {
            filteredTopics = topics.filter { topic in
                topic.title.localizedCaseInsensitiveContains(query)
            }
        }
        selectedTopicIndex = 0
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

    var body: some View {
        // Fixed height container - window never resizes
        // Content aligns to top, background only covers content area
        VStack(spacing: 0) {
            // Content area with background
            contentWithBackground

            // Transparent spacer fills remaining window space
            Spacer(minLength: 0)
        }
    }

    /// Content area with animated background
    private var contentWithBackground: some View {
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
        // Smooth animation for background expansion
        .animation(.spring(response: 0.25, dampingFraction: 0.85), value: viewModel.showCommandList)
        .animation(.spring(response: 0.25, dampingFraction: 0.85), value: viewModel.showTopicList)
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

            // Text field - using IMETextField for proper IME and focus handling in floating windows
            IMETextField(
                text: $viewModel.inputText,
                placeholder: "Type a message... (/ for commands, // for topics)",
                font: .systemFont(ofSize: 16),
                textColor: .labelColor,
                placeholderColor: NSColor.secondaryLabelColor,
                backgroundColor: .clear,
                autoFocus: true,
                onSubmit: { viewModel.submit() },
                onEscape: { viewModel.cancel() },
                onTextChange: { newValue in
                    viewModel.handleInputChange(newValue)
                },
                onArrowUp: { viewModel.moveSelectionUp() },
                onArrowDown: { viewModel.moveSelectionDown() },
                onTab: {
                    // Tab to complete selected command
                    if viewModel.showCommandList, viewModel.selectedCommandIndex < viewModel.commands.count {
                        let command = viewModel.commands[viewModel.selectedCommandIndex]
                        viewModel.selectCommand(command)
                    }
                    // Tab to select topic
                    else if viewModel.showTopicList, viewModel.selectedTopicIndex < viewModel.filteredTopics.count {
                        let topic = viewModel.filteredTopics[viewModel.selectedTopicIndex]
                        viewModel.selectTopic(topic)
                    }
                }
            )
            .frame(height: 24)

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
                        ForEach(Array(viewModel.filteredTopics.enumerated()), id: \.element.id) { index, topic in
                            TopicRowView(
                                topic: topic,
                                isSelected: index == viewModel.selectedTopicIndex
                            ) {
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
    let isSelected: Bool
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
            .background((isHovering || isSelected) ? Color.purple.opacity(0.15) : Color.clear)
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
