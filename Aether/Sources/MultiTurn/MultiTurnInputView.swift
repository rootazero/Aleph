//
//  MultiTurnInputView.swift
//  Aether
//
//  SwiftUI view for multi-turn input window.
//  Updated to use Liquid Glass design language (macOS 26+).
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

    private var coreV2: AetherV2Core? {
        (NSApplication.shared.delegate as? AppDelegate)?.coreV2
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

    func reloadTopics() {
        loadTopics()
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

    // MARK: - Topic Operations

    func deleteTopic(_ topic: Topic) {
        // 1. Delete associated memories from Rust core first
        if let coreV2 = coreV2 {
            do {
                let deletedMemories = try coreV2.deleteMemoriesByTopicId(topicId: topic.id)
                print("[MultiTurnInputViewModel] Deleted \(deletedMemories) memories for topic: \(topic.id)")
            } catch {
                print("[MultiTurnInputViewModel] Failed to delete memories: \(error)")
            }
        }

        // 2. Delete messages from conversation store
        ConversationStore.shared.deleteMessages(topicId: topic.id)

        // 3. Soft-delete the topic
        ConversationStore.shared.deleteTopic(id: topic.id)

        // 4. Reload topic list
        loadTopics()

        print("[MultiTurnInputViewModel] Deleted topic, messages and memories: \(topic.title)")
    }

    func renameTopic(_ topic: Topic, newTitle: String) {
        let trimmedTitle = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedTitle.isEmpty else { return }
        ConversationStore.shared.updateTopicTitle(id: topic.id, title: trimmedTitle)
        loadTopics()
        print("[MultiTurnInputViewModel] Renamed topic: \(topic.id) -> \(trimmedTitle)")
    }

    // MARK: - Command Loading

    private func loadCommands(prefix: String) {
        guard let coreV2 = coreV2 else {
            print("[MultiTurnInputViewModel] V2 Core not available")
            commands = []
            return
        }

        let allCommands = coreV2.getRootCommandsFromRegistry()
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
    /// Uses adaptive glass effect for Liquid Glass on macOS 26+
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
        .adaptiveGlass()  // Liquid Glass on macOS 26+, VisualEffect fallback
        // Smooth animation for background expansion
        .animation(.smooth(duration: 0.3), value: viewModel.showCommandList)
        .animation(.smooth(duration: 0.3), value: viewModel.showTopicList)
    }

    // MARK: - Input Field

    private var inputField: some View {
        HStack(spacing: 12) {
            // Turn indicator (pure glass style - no colored background)
            if viewModel.turnCount > 0 {
                Text("Turn \(viewModel.turnCount + 1)")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(.primary.opacity(0.06))
                    .clipShape(RoundedRectangle(cornerRadius: 4))
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

            // Submit button with glass prominent style
            Button(action: viewModel.submit) {
                Image(systemName: "arrow.up")
            }
            .buttonStyle(.plain)
            .adaptiveGlassProminent()
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
                    .foregroundColor(.primary.opacity(0.6))
                    .padding()
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        // Use VStack to ensure all rows are rendered for scrollTo
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
                    .frame(maxHeight: 300)
                    .onChange(of: viewModel.selectedCommandIndex) { _, newIndex in
                        withAnimation(.easeInOut(duration: 0.15)) {
                            proxy.scrollTo("cmd-\(newIndex)", anchor: nil)
                        }
                    }
                }
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
                    .foregroundColor(.primary.opacity(0.6))
                    .padding()
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        // Use VStack to ensure all rows are rendered for scrollTo
                        VStack(spacing: 0) {
                            ForEach(Array(viewModel.filteredTopics.enumerated()), id: \.element.id) { index, topic in
                                TopicRowView(
                                    topic: topic,
                                    isSelected: index == viewModel.selectedTopicIndex,
                                    onSelect: {
                                        viewModel.selectTopic(topic)
                                    },
                                    onDelete: {
                                        viewModel.deleteTopic(topic)
                                    },
                                    onRename: { newTitle in
                                        viewModel.renameTopic(topic, newTitle: newTitle)
                                    }
                                )
                                .id("topic-\(index)")
                            }
                        }
                    }
                    .frame(maxHeight: 300)
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

// MARK: - CommandRowView

/// Row view for command list with glass style
struct CommandRowView: View {
    let command: CommandNode
    let isSelected: Bool
    let onSelect: () -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: onSelect) {
            HStack(spacing: 12) {
                // Command icon (neutral color for pure glass style)
                Image(systemName: command.icon.isEmpty ? "terminal" : command.icon)
                    .font(.system(size: 14))
                    .foregroundColor(.primary.opacity(0.7))
                    .frame(width: 20)

                // Command info
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text("/\(command.key)")
                            .font(.system(size: 14, weight: .medium))

                        // Source badge (neutral style)
                        Text(sourceTypeName(command.sourceType))
                            .font(.system(size: 9))
                            .padding(.horizontal, 4)
                            .padding(.vertical, 1)
                            .background(.primary.opacity(0.1))
                            .clipShape(RoundedRectangle(cornerRadius: 3))
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
                    .foregroundColor(.primary.opacity(0.4))
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background((isHovering || isSelected) ? .white.opacity(0.1) : .clear)
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHovering = hovering
        }
    }
}

// MARK: - TopicRowView

/// Row view for topic list with glass style
struct TopicRowView: View {
    let topic: Topic
    let isSelected: Bool
    let onSelect: () -> Void
    let onDelete: () -> Void
    let onRename: (String) -> Void

    @State private var isHovering = false
    @State private var isEditing = false
    @State private var isConfirmingDelete = false
    @State private var editingTitle: String = ""
    @FocusState private var isTextFieldFocused: Bool

    var body: some View {
        HStack {
            if isConfirmingDelete {
                // Delete confirmation mode
                deleteConfirmView
            } else if isEditing {
                // Editing mode: inline text field
                editingView
            } else {
                // Normal mode: display topic info
                normalView
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background((isHovering || isSelected || isConfirmingDelete) ? .white.opacity(0.1) : .clear)
        .onHover { hovering in
            isHovering = hovering
        }
    }

    // MARK: - Normal View

    private var normalView: some View {
        HStack {
            // Topic info - clickable to select
            Button(action: onSelect) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(topic.title)
                        .font(.system(size: 14))
                        .lineLimit(1)

                    Text(formatDate(topic.updatedAt))
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .buttonStyle(.plain)

            Spacer()

            // Action buttons - visible on hover
            if isHovering {
                HStack(spacing: 8) {
                    // Rename button
                    Button {
                        editingTitle = topic.title
                        isEditing = true
                        isTextFieldFocused = true
                    } label: {
                        Image(systemName: "pencil")
                            .font(.system(size: 12))
                            .foregroundColor(.primary.opacity(0.6))
                    }
                    .buttonStyle(.plain)
                    .help("Rename")

                    // Delete button
                    Button {
                        withAnimation(.easeInOut(duration: 0.15)) {
                            isConfirmingDelete = true
                        }
                    } label: {
                        Image(systemName: "trash")
                            .font(.system(size: 12))
                            .foregroundColor(.red.opacity(0.7))
                    }
                    .buttonStyle(.plain)
                    .help("Delete")
                }
                .transition(.opacity.combined(with: .scale(scale: 0.8)))
            } else {
                // Chevron when not hovering
                Image(systemName: "chevron.right")
                    .font(.caption)
                    .foregroundColor(.primary.opacity(0.4))
            }
        }
        .animation(.easeInOut(duration: 0.15), value: isHovering)
    }

    // MARK: - Delete Confirmation View

    private var deleteConfirmView: some View {
        HStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 14))
                .foregroundColor(.orange)

            Text("确认删除「\(topic.title)」？")
                .font(.system(size: 13))
                .lineLimit(1)

            Spacer()

            // Cancel button
            Button {
                withAnimation(.easeInOut(duration: 0.15)) {
                    isConfirmingDelete = false
                }
            } label: {
                Text("取消")
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.plain)

            // Confirm delete button
            Button {
                onDelete()
                isConfirmingDelete = false
            } label: {
                Text("删除")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.red)
            }
            .buttonStyle(.plain)
        }
    }

    // MARK: - Editing View

    private var editingView: some View {
        HStack(spacing: 8) {
            TextField("Topic title", text: $editingTitle)
                .textFieldStyle(.plain)
                .font(.system(size: 14))
                .focused($isTextFieldFocused)
                .onSubmit {
                    commitRename()
                }
                .onExitCommand {
                    cancelEditing()
                }

            // Confirm button
            Button {
                commitRename()
            } label: {
                Image(systemName: "checkmark")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.green)
            }
            .buttonStyle(.plain)
            .help("Confirm")

            // Cancel button
            Button {
                cancelEditing()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.plain)
            .help("Cancel")
        }
    }

    // MARK: - Actions

    private func commitRename() {
        let newTitle = editingTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        if !newTitle.isEmpty && newTitle != topic.title {
            onRename(newTitle)
        }
        isEditing = false
    }

    private func cancelEditing() {
        isEditing = false
        editingTitle = topic.title
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
