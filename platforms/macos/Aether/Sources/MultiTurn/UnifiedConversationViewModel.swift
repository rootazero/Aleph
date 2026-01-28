//
//  UnifiedConversationViewModel.swift
//  Aether
//
//  View model for unified conversation window.
//  Manages display state, messages, attachments, commands, and input.
//

import AppKit
import Combine
import SwiftUI

// MARK: - UnifiedConversationViewModel

/// Unified view model for conversation window
///
/// Thread Safety:
/// - Marked as @MainActor since it drives SwiftUI views
@MainActor
@Observable
final class UnifiedConversationViewModel {

    // MARK: - Display State

    /// Current display state (empty, conversation, or commandList)
    var displayState: ContentDisplayState = .empty

    // MARK: - Conversation Data

    /// Current topic
    var topic: Topic?

    /// Messages in current conversation
    var messages: [ConversationMessage] = []

    /// Whether AI is currently responding
    var isLoading: Bool = false

    /// Error message if any
    var errorMessage: String?

    /// Streaming message state
    var streamingMessageId: String?
    var streamingText: String = ""

    /// Preloaded attachments grouped by message ID (Phase 5: Performance optimization)
    /// Populated by loadTopic() to avoid N+1 queries
    var messageAttachments: [String: [StoredAttachment]] = [:]

    // MARK: - Progress Tracking (for multi-turn agentic tasks)

    /// Currently executing tool name (nil when not executing)
    /// @deprecated Use activeToolCalls instead (Phase 3: Legacy cleanup)
    var currentToolCall: String?

    /// Task plan steps for multi-step tasks
    /// @deprecated Use activePlanParts instead (Phase 3: Legacy cleanup)
    var planSteps: [PlanStep] = []

    /// Current step index in the plan
    var currentStepIndex: Int = 0

    // MARK: - Status Bar Display (multi-layer intelligent progress)

    /// Status messages - high-level task progress (replaced, not accumulated)
    var statusMessages: [String] = []

    /// Whether status bar is showing a loading/thinking state
    var statusIsLoading: Bool = false

    /// Current thinking/reasoning content from LLM (cleaned)
    /// @deprecated Use activeReasoningParts instead (Phase 3: Legacy cleanup)
    var currentThinking: String?

    /// Current tool operation description (high-level, not file paths)
    var currentToolActivity: String?

    /// Dynamic status bar height based on number of visible layers
    /// Returns 0 when conversation is hidden, otherwise calculates based on line count
    /// Formula: 24px base padding + 24px per line (12pt font + 3pt spacing)
    var dynamicStatusBarHeight: CGFloat {
        guard shouldShowConversation else { return 0 }
        let lineCount = max(1, statusMessages.count)
        return CGFloat(24 + lineCount * 24)  // 24px base + 24px per line
    }

    // MARK: - Message Flow Parts (Claude Code-style rendering)

    /// Active tool call parts (for real-time status display)
    var activeToolCalls: [ToolCallPart] = []

    /// Streaming response parts (for delta-based updates)
    var streamingParts: [String: StreamingResponsePart] = [:]

    /// Active reasoning parts (for real-time reasoning display)
    var activeReasoningParts: [ReasoningPart] = []

    /// Active plan parts (for plan visualization)
    var activePlanParts: [PlanPart] = []

    // MARK: - UI Configuration (Phase 6: User preferences for Part display)

    /// Configuration for Part display behavior
    /// Loaded from UserDefaults, can be modified in Settings
    private struct PartDisplayConfig {
        var showReasoning: Bool
        var showPlan: Bool
        var showToolCalls: Bool
        var maxRecentToolCalls: Int

        static let `default` = PartDisplayConfig(
            showReasoning: false,  // Default OFF - reasoning can be verbose
            showPlan: true,        // Default ON - plans are useful
            showToolCalls: true,   // Default ON - tool execution visibility
            maxRecentToolCalls: 3  // Keep only recent 3 terminal tool calls
        )

        // UserDefaults keys
        private static let showReasoningKey = "PartDisplay.ShowReasoning"
        private static let showPlanKey = "PartDisplay.ShowPlan"
        private static let showToolCallsKey = "PartDisplay.ShowToolCalls"
        private static let maxToolCallsKey = "PartDisplay.MaxRecentToolCalls"

        static func load() -> PartDisplayConfig {
            let defaults = UserDefaults.standard
            return PartDisplayConfig(
                showReasoning: defaults.object(forKey: showReasoningKey) as? Bool ?? `default`.showReasoning,
                showPlan: defaults.object(forKey: showPlanKey) as? Bool ?? `default`.showPlan,
                showToolCalls: defaults.object(forKey: showToolCallsKey) as? Bool ?? `default`.showToolCalls,
                maxRecentToolCalls: defaults.object(forKey: maxToolCallsKey) as? Int ?? `default`.maxRecentToolCalls
            )
        }

        func save() {
            let defaults = UserDefaults.standard
            defaults.set(showReasoning, forKey: Self.showReasoningKey)
            defaults.set(showPlan, forKey: Self.showPlanKey)
            defaults.set(showToolCalls, forKey: Self.showToolCallsKey)
            defaults.set(maxRecentToolCalls, forKey: Self.maxToolCallsKey)
        }
    }

    /// Current Part display configuration
    private var partDisplayConfig = PartDisplayConfig.load()

    /// Reload Part display configuration from UserDefaults (Phase 6)
    /// Call this when settings are changed to apply new config immediately
    func reloadPartDisplayConfig() {
        partDisplayConfig = PartDisplayConfig.load()
        print("[UnifiedViewModel] Part display config reloaded: reasoning=\(partDisplayConfig.showReasoning), plan=\(partDisplayConfig.showPlan), tools=\(partDisplayConfig.showToolCalls), max=\(partDisplayConfig.maxRecentToolCalls)")

        // Apply new config immediately by clearing filtered-out parts
        if !partDisplayConfig.showReasoning {
            activeReasoningParts.removeAll()
        }
        if !partDisplayConfig.showPlan {
            activePlanParts.removeAll()
        }
        if !partDisplayConfig.showToolCalls {
            activeToolCalls.removeAll()
        } else {
            // Re-apply maxRecentToolCalls limit
            pruneTerminalToolCalls()
        }
    }

    /// Get current Part display configuration (Phase 6: for Settings UI)
    func getPartDisplayConfig() -> (showReasoning: Bool, showPlan: Bool, showToolCalls: Bool, maxRecentToolCalls: Int) {
        return (
            showReasoning: partDisplayConfig.showReasoning,
            showPlan: partDisplayConfig.showPlan,
            showToolCalls: partDisplayConfig.showToolCalls,
            maxRecentToolCalls: partDisplayConfig.maxRecentToolCalls
        )
    }

    /// Update Part display configuration (Phase 6: for Settings UI)
    func updatePartDisplayConfig(showReasoning: Bool? = nil, showPlan: Bool? = nil, showToolCalls: Bool? = nil, maxRecentToolCalls: Int? = nil) {
        var config = partDisplayConfig

        if let showReasoning = showReasoning {
            config.showReasoning = showReasoning
        }
        if let showPlan = showPlan {
            config.showPlan = showPlan
        }
        if let showToolCalls = showToolCalls {
            config.showToolCalls = showToolCalls
        }
        if let maxRecentToolCalls = maxRecentToolCalls {
            config.maxRecentToolCalls = maxRecentToolCalls
        }

        config.save()
        reloadPartDisplayConfig()
    }

    // MARK: - DAG Plan Confirmation (inline in conversation)

    /// Pending plan confirmation (shown inline in conversation area)
    var pendingPlanConfirmation: PendingPlanConfirmation?

    /// Core reference for plan confirmation callback (set by notification handler)
    var planConfirmationCore: AetherCore?

    // MARK: - User Input Request (inline in conversation)

    /// Pending user input request (shown inline in conversation area)
    var pendingUserInputRequest: PendingUserInputRequest?

    /// Core reference for user input callback (set by notification handler)
    var userInputCore: AetherCore?

    // MARK: - Attachment Data

    /// Pending attachments to send with next message
    var pendingAttachments: [PendingAttachment] = []

    // MARK: - Input State

    /// Current input text
    var inputText: String = "" {
        didSet {
            updateDisplayState()
        }
    }

    /// Turn count for display
    var turnCount: Int = 0

    // MARK: - Token Usage (Gateway integration)

    /// Total tokens used in current/last run
    var totalTokens: UInt64 = 0

    /// Number of tool calls in current/last run
    var toolCallCount: UInt32 = 0

    /// Number of agent loops in current/last run
    var loopCount: UInt32 = 0

    /// Whether to show token usage in UI
    var showTokenUsage: Bool = true

    /// Formatted token usage string for display
    var tokenUsageDisplay: String {
        guard totalTokens > 0 else { return "" }

        var parts: [String] = []

        // Format tokens with K suffix for large numbers
        if totalTokens >= 1000 {
            let kTokens = Double(totalTokens) / 1000.0
            parts.append(String(format: "%.1fK tokens", kTokens))
        } else {
            parts.append("\(totalTokens) tokens")
        }

        if toolCallCount > 0 {
            parts.append("\(toolCallCount) tools")
        }

        if loopCount > 1 {
            parts.append("\(loopCount) loops")
        }

        return parts.joined(separator: " · ")
    }

    // MARK: - Gateway AskUser Integration

    /// Pending Gateway user question (nil when no question pending)
    var gatewayPendingQuestion: AskUserEvent?

    /// GatewayMultiTurnAdapter reference for answering questions
    weak var gatewayAdapter: GatewayMultiTurnAdapter?

    /// Check if there's any pending question (from either source)
    var hasAnyPendingQuestion: Bool {
        gatewayPendingQuestion != nil || pendingUserInputRequest != nil
    }

    // MARK: - Command/Topic Data

    /// Available commands
    var commands: [CommandNode] = []

    /// All topics
    var topics: [Topic] = []

    /// Filtered topics based on search
    var filteredTopics: [Topic] = []

    /// Selected command index
    var selectedCommandIndex: Int = 0

    /// Selected topic index
    var selectedTopicIndex: Int = 0

    // MARK: - Callbacks

    /// Called when message should be submitted
    var onSubmit: ((String, [PendingAttachment]) -> Void)?

    /// Called when window should close
    var onCancel: (() -> Void)?

    /// Called when topic is selected
    var onTopicSelected: ((Topic) -> Void)?

    /// Called when window height should change
    var onHeightChanged: ((CGFloat) -> Void)?

    /// Last reported content height (without status bar)
    private var lastContentHeight: CGFloat = 0

    // MARK: - Core Reference

    private var core: AetherCore? {
        (NSApplication.shared.delegate as? AppDelegate)?.core
    }

    // MARK: - Computed Properties

    /// Whether there are any messages
    var hasMessages: Bool {
        !messages.isEmpty
    }

    /// Topic title for display
    var displayTitle: String {
        topic?.title ?? "New Conversation"
    }

    /// Whether to show conversation area
    var shouldShowConversation: Bool {
        displayState == .conversation && hasMessages
    }

    /// Whether to show status bar (when processing even without messages)
    var shouldShowStatus: Bool {
        // Show status when:
        // 1. There's active loading/processing
        // 2. There are status messages to display
        // 3. There are active tool calls
        statusIsLoading || !statusMessages.isEmpty || !activeToolCalls.isEmpty
    }

    /// Whether to show command list
    var shouldShowCommandList: Bool {
        displayState.isShowingCommands
    }

    /// Whether to show topic list
    var shouldShowTopicList: Bool {
        displayState.isShowingTopicList
    }

    // MARK: - Display State Management

    /// Update display state based on input
    ///
    /// Priority (high → low):
    /// 1. `//` prefix → Topic list
    /// 2. `/` prefix → Command list
    /// 3. Has messages → Conversation
    /// 4. No messages → Empty
    private func updateDisplayState() {
        let previousState = displayState

        if inputText.hasPrefix("//") {
            displayState = .commandList(prefix: "//")
            loadTopics()
            filterTopics(query: String(inputText.dropFirst(2)))
        } else if inputText.hasPrefix("/") {
            displayState = .commandList(prefix: "/")
            loadCommands(prefix: String(inputText.dropFirst()))
        } else if hasMessages {
            displayState = .conversation
        } else {
            displayState = .empty
        }

        // When transitioning to empty state, report zero content height
        // to collapse window to input-only
        if displayState == .empty && previousState != .empty {
            onHeightChanged?(0)
        }
    }

    /// Force update display state (for external calls)
    func refreshDisplayState() {
        updateDisplayState()
    }

    // MARK: - Input Actions

    /// Submit current input
    func submit() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !pendingAttachments.isEmpty else { return }
        guard !text.hasPrefix("//") else { return }

        // If command list showing, complete selected command
        if shouldShowCommandList, selectedCommandIndex < commands.count {
            let command = commands[selectedCommandIndex]
            inputText = "/\(command.key) "
            return
        }

        // Submit with attachments
        onSubmit?(text, pendingAttachments)

        // Clear state
        inputText = ""
        pendingAttachments = []
    }

    /// Cancel / close window
    func cancel() {
        onCancel?()
    }

    /// Handle ESC key with layered exit
    ///
    /// Exit priority (high → low):
    /// 1. Close command/topic list → restore to conversation or empty
    /// 2. Close window
    func handleEscape() {
        if displayState.isShowingCommandList {
            // Layer 1: Close command/topic list
            // Setting inputText = "" triggers updateDisplayState() via didSet,
            // which automatically restores to .conversation or .empty
            inputText = ""
        } else {
            // Layer 2: Close window
            cancel()
        }
    }

    // MARK: - Navigation

    func moveSelectionUp() {
        if shouldShowCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex - 1 + commands.count) % commands.count
        } else if shouldShowTopicList && !filteredTopics.isEmpty {
            selectedTopicIndex = (selectedTopicIndex - 1 + filteredTopics.count) % filteredTopics.count
        }
    }

    func moveSelectionDown() {
        if shouldShowCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex + 1) % commands.count
        } else if shouldShowTopicList && !filteredTopics.isEmpty {
            selectedTopicIndex = (selectedTopicIndex + 1) % filteredTopics.count
        }
    }

    func handleTab() {
        if shouldShowCommandList, selectedCommandIndex < commands.count {
            selectCommand(commands[selectedCommandIndex])
        } else if shouldShowTopicList, selectedTopicIndex < filteredTopics.count {
            selectTopic(filteredTopics[selectedTopicIndex])
        }
    }

    // MARK: - Topic Management

    func loadTopics() {
        topics = ConversationStore.shared.getAllTopics()
        filteredTopics = topics
        selectedTopicIndex = 0
    }

    private func filterTopics(query: String) {
        if query.isEmpty {
            filteredTopics = topics
        } else {
            filteredTopics = topics.filter {
                $0.title.localizedCaseInsensitiveContains(query)
            }
        }
        selectedTopicIndex = 0
    }

    func selectTopic(_ topic: Topic) {
        onTopicSelected?(topic)
        inputText = ""
    }

    func deleteTopic(_ topic: Topic) {
        if let core = core {
            do {
                let deletedMemories = try core.deleteMemoriesByTopicId(topicId: topic.id)
                print("[UnifiedViewModel] Deleted \(deletedMemories) memories")
            } catch {
                print("[UnifiedViewModel] Failed to delete memories: \(error)")
            }
        }
        ConversationStore.shared.deleteMessages(topicId: topic.id)
        ConversationStore.shared.deleteTopic(id: topic.id)
        loadTopics()
    }

    func renameTopic(_ topic: Topic, newTitle: String) {
        let trimmed = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        ConversationStore.shared.updateTopicTitle(id: topic.id, title: trimmed)
        loadTopics()
    }

    // MARK: - Command Management

    private func loadCommands(prefix: String) {
        guard let core = core else {
            commands = []
            return
        }

        let allCommands = core.getRootCommandsFromRegistry()
        if prefix.isEmpty {
            commands = allCommands
        } else {
            let lower = prefix.lowercased()
            commands = allCommands.filter {
                $0.key.lowercased().hasPrefix(lower) ||
                $0.description.lowercased().contains(lower)
            }
        }
        selectedCommandIndex = 0
    }

    func selectCommand(_ command: CommandNode) {
        inputText = "/\(command.key) "
    }

    // MARK: - Attachment Management

    /// Add attachment from URL
    func addAttachment(url: URL) {
        do {
            let attachment = try PendingAttachment(url: url)
            pendingAttachments.append(attachment)
            print("[UnifiedViewModel] Added attachment: \(attachment.fileName)")
        } catch {
            print("[UnifiedViewModel] Failed to add attachment: \(error)")
            errorMessage = "Failed to add file: \(error.localizedDescription)"
        }
    }

    /// Add attachments from URLs
    func addAttachments(urls: [URL]) {
        for url in urls {
            addAttachment(url: url)
        }
    }

    /// Remove attachment
    func removeAttachment(_ attachment: PendingAttachment) {
        pendingAttachments.removeAll { $0.id == attachment.id }
    }

    /// Clear all attachments
    func clearAttachments() {
        pendingAttachments = []
    }

    // MARK: - Message Management

    func loadTopic(_ topic: Topic) {
        self.topic = topic
        self.messages = ConversationStore.shared.getMessages(topicId: topic.id)

        // Batch preload attachments (Phase 5: Performance optimization)
        // Single JOIN query instead of N+1 individual queries
        self.messageAttachments = AttachmentStore.shared.getAttachmentsByTopic(topicId: topic.id)

        self.errorMessage = nil
        self.displayState = messages.isEmpty ? .empty : .conversation

        print("[UnifiedViewModel] Loaded \(messages.count) messages with \(messageAttachments.values.flatMap { $0 }.count) attachments")
    }

    /// Clear current topic (for new session before first message)
    func clearTopic() {
        self.topic = nil
        self.messages = []
        self.errorMessage = nil
        self.displayState = .empty

        // Clear preloaded attachments (Phase 5)
        self.messageAttachments = [:]
    }

    /// Get attachments for a message (Phase 5: from preloaded cache)
    /// - Parameter messageId: The message ID
    /// - Returns: Array of attachments, empty if none found
    func getAttachments(forMessage messageId: String) -> [StoredAttachment] {
        return messageAttachments[messageId] ?? []
    }

    func addUserMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .user,
            content: content
        ) {
            messages.append(message)
            displayState = .conversation

            // Save pending attachments to this message
            saveAttachments(for: message.id)
        }
    }

    /// Save pending attachments to storage
    /// - Parameter messageId: The message ID to link attachments to
    private func saveAttachments(for messageId: String) {
        guard !pendingAttachments.isEmpty else { return }

        for pending in pendingAttachments {
            // Save file to disk
            guard let localPath = AttachmentFileManager.shared.saveUserUpload(
                from: pending,
                messageId: messageId
            ) else {
                print("[UnifiedViewModel] Failed to save attachment file: \(pending.fileName)")
                continue
            }

            // Create and save database record
            let stored = StoredAttachment(
                from: pending,
                messageId: messageId,
                localPath: localPath
            )
            AttachmentStore.shared.save(stored)
        }

        print("[UnifiedViewModel] Saved \(pendingAttachments.count) attachments for message: \(messageId)")
    }

    func addAssistantMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        // Extract and save attachments BEFORE stripping the block
        // Strip the [GENERATED_FILES] block from content for display
        let displayContent = stripGeneratedFilesBlock(from: content)

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .assistant,
            content: displayContent
        ) {
            messages.append(message)

            // Extract and save image URLs from original content (with file block)
            extractAndSaveImageURLs(from: content, messageId: message.id)
        }
        isLoading = false
    }

    func startStreamingMessage() -> String? {
        guard let topicId = topic?.id else { return nil }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .assistant,
            content: ""
        ) {
            messages.append(message)
            streamingMessageId = message.id
            streamingText = ""
            return message.id
        }
        return nil
    }

    func updateStreamingText(_ text: String) {
        // Only update streamingText - do NOT update messages array during streaming
        // This avoids triggering SwiftUI's expensive array diff and re-render
        streamingText = text

        // Update status bar with the latest streaming text (truncated for display)
        let displayText = text.count > 80 ? "..." + String(text.suffix(80)) : text
        let singleLine = displayText.replacingOccurrences(of: "\n", with: " ")
        updateStatus(singleLine, isLoading: true)
    }

    func finishStreamingMessage() {
        print("[UnifiedViewModel] finishStreamingMessage: messageId=\(streamingMessageId ?? "nil"), streamingTextLen=\(streamingText.count)")

        if let messageId = streamingMessageId,
           let index = messages.firstIndex(where: { $0.id == messageId }) {
            // Extract and save attachments BEFORE stripping the block
            extractAndSaveImageURLs(from: streamingText, messageId: messageId)

            // Strip the [GENERATED_FILES] block from content for display
            let displayContent = stripGeneratedFilesBlock(from: streamingText)

            print("[UnifiedViewModel] Updating message content: index=\(index), contentLen=\(displayContent.count)")

            // Update the message content in the array ONLY when streaming finishes
            // This triggers a single re-render with the final content
            messages[index].content = displayContent

            // Persist to store
            ConversationStore.shared.updateMessageContent(
                messageId: messageId,
                content: displayContent
            )

            print("[UnifiedViewModel] Message updated and persisted")
        } else {
            print("[UnifiedViewModel] Warning: No streaming message to finish")
        }

        streamingMessageId = nil
        streamingText = ""
        isLoading = false

        // Clear status messages to hide the status bar when output is complete
        // Status bar will auto-disappear based on shouldShowStatus
        clearCurrentStatus()
    }

    /// Strip [GENERATED_FILES] block from content for display
    /// - Parameter content: The full content with potential file block
    /// - Returns: Content without the generated files metadata block
    private func stripGeneratedFilesBlock(from content: String) -> String {
        // Remove [GENERATED_FILES]...[/GENERATED_FILES] block from content
        guard let startRange = content.range(of: "\n\n[GENERATED_FILES]"),
              let endRange = content.range(of: "[/GENERATED_FILES]") else {
            // Try without leading newlines
            guard let startRange = content.range(of: "[GENERATED_FILES]"),
                  let endRange = content.range(of: "[/GENERATED_FILES]") else {
                return content
            }
            // Remove the block
            var result = content
            result.removeSubrange(startRange.lowerBound..<endRange.upperBound)
            return result.trimmingCharacters(in: .whitespacesAndNewlines)
        }

        // Remove the block including leading newlines
        var result = content
        result.removeSubrange(startRange.lowerBound..<endRange.upperBound)
        return result.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Extract image URLs and generated files from content and save as attachments
    /// - Parameters:
    ///   - content: The message content to parse
    ///   - messageId: The message ID to link attachments to
    private func extractAndSaveImageURLs(from content: String, messageId: String) {
        // First, extract generated files from [GENERATED_FILES] block
        let generatedFiles = extractGeneratedFiles(from: content)

        // Then parse content for image URLs
        let segments = ContentParser.parse(content)
        var imageURLs: [String] = []

        for segment in segments {
            if case .image(let url) = segment {
                imageURLs.append(url)
            }
        }

        // Combine: generated files + inline image URLs (deduplicate)
        var allURLs = generatedFiles
        for url in imageURLs {
            if !allURLs.contains(url) {
                allURLs.append(url)
            }
        }

        guard !allURLs.isEmpty else { return }

        // Save each URL as an attachment (local file or remote URL)
        for urlString in allURLs {
            guard let url = URL(string: urlString) else { continue }

            // Determine if it's a local file (from output directory) or remote URL
            let isLocalFile = url.isFileURL || urlString.hasPrefix("file://")

            if isLocalFile {
                // For local files (tool-generated), save direct reference to output directory
                // No copying needed - files stay in ~/.aether/output/{topic_id}/
                let stored = StoredAttachment.forToolOutput(
                    messageId: messageId,
                    toolName: "ai_generated",
                    sourceURL: url,
                    localPath: url.path // Direct path to file in output directory
                )
                AttachmentStore.shared.save(stored)
                print("[UnifiedViewModel] Saved reference to AI-generated file: \(url.path)")
            } else {
                // For remote URLs, create attachment record (download in background)
                let stored = StoredAttachment.forToolOutput(
                    messageId: messageId,
                    toolName: "remote",
                    sourceURL: url,
                    localPath: nil
                )
                AttachmentStore.shared.save(stored)
                print("[UnifiedViewModel] Saved remote URL attachment: \(urlString)")

                // Optionally cache remote images in background
                Task {
                    if let relativePath = await AttachmentFileManager.shared.downloadAndCache(
                        url: url,
                        messageId: messageId
                    ) {
                        AttachmentStore.shared.updateLocalPath(id: stored.id, localPath: relativePath)
                        print("[UnifiedViewModel] Cached remote image: \(relativePath)")
                    }
                }
            }
        }

        print("[UnifiedViewModel] Extracted \(allURLs.count) URLs from assistant response (images: \(imageURLs.count), generated files: \(generatedFiles.count))")
    }

    /// Extract file URLs from [GENERATED_FILES] block in response
    /// - Parameter content: The full response content
    /// - Returns: Array of file URL strings
    private func extractGeneratedFiles(from content: String) -> [String] {
        // Look for [GENERATED_FILES]...[/GENERATED_FILES] block
        guard let startRange = content.range(of: "[GENERATED_FILES]"),
              let endRange = content.range(of: "[/GENERATED_FILES]"),
              startRange.upperBound < endRange.lowerBound else {
            return []
        }

        let filesBlock = String(content[startRange.upperBound..<endRange.lowerBound])
        let fileURLs = filesBlock
            .split(separator: "\n")
            .map { String($0).trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }

        print("[UnifiedViewModel] Found \(fileURLs.count) generated files in response")
        return fileURLs
    }

    func setLoading(_ loading: Bool) {
        isLoading = loading
    }

    func setError(_ message: String?) {
        errorMessage = message
        isLoading = false
        // Clear status messages when error occurs (status bar will hide)
        clearCurrentStatus()
    }

    func reportHeightChange(_ height: CGFloat) {
        lastContentHeight = height

        // Dynamic status bar height calculation
        // Base: 24px (padding) + 24px per line (12pt font + 3pt spacing)
        onHeightChanged?(height + dynamicStatusBarHeight)
    }

    // MARK: - Status Bar Management

    /// Add status message - intelligently display multi-layer progress
    func updateStatus(_ text: String, isLoading: Bool = true) {
        statusIsLoading = isLoading

        // Extract and classify incoming content
        if text.hasPrefix("💭") {
            // Extract thinking content (remove emoji, clean)
            let thinking = String(text.dropFirst(2)).trimmingCharacters(in: .whitespaces)
            currentThinking = cleanThinkingContent(thinking)
        } else if text.hasPrefix("⚡") {
            // Extract tool activity (remove emoji, high-level only)
            let activity = String(text.dropFirst(2)).trimmingCharacters(in: .whitespaces)
            currentToolActivity = extractHighLevelActivity(activity)
        } else if text.hasPrefix("✓") {
            // Tool completed - clear tool activity
            currentToolActivity = nil
        } else {
            // Other status messages - use as-is
            statusMessages = [text]
            return
        }

        // Rebuild status messages from multiple layers
        rebuildStatusDisplay()
    }

    /// Rebuild status display by intelligently combining multiple information sources
    private func rebuildStatusDisplay() {
        var layers: [String] = []

        // Layer 1: Plan progress (highest priority if exists)
        if !planSteps.isEmpty && currentStepIndex < planSteps.count {
            let currentStep = planSteps[currentStepIndex]
            let progress = "🔧 步骤 \(currentStepIndex + 1)/\(planSteps.count): \(currentStep.description)"
            layers.append(progress)
        }

        // Layer 2: Current thinking/reasoning (if exists)
        if let thinking = currentThinking, !thinking.isEmpty {
            layers.append("💭 \(thinking)")
        }

        // Layer 3: Current tool activity (if exists and meaningful)
        if let activity = currentToolActivity, !activity.isEmpty {
            layers.append("⚙️ \(activity)")
        }

        // Fallback: Generic processing message
        if layers.isEmpty {
            layers.append("⚙️ 处理中...")
        }

        // Use withAnimation for smooth layer changes
        withAnimation(.smooth(duration: 0.25)) {
            statusMessages = layers
        }
    }

    /// Clean thinking content - extract key decision points, remove noise
    private func cleanThinkingContent(_ raw: String) -> String {
        // Remove common noise patterns
        var cleaned = raw
            .replacingOccurrences(of: "I should ", with: "")
            .replacingOccurrences(of: "I will ", with: "")
            .replacingOccurrences(of: "Let me ", with: "")
            .replacingOccurrences(of: "I need to ", with: "")

        // Truncate long thinking to fit status bar (max ~80 chars)
        if cleaned.count > 80 {
            let endIndex = cleaned.index(cleaned.startIndex, offsetBy: 77)
            cleaned = String(cleaned[..<endIndex]) + "..."
        }

        // Remove newlines
        cleaned = cleaned.replacingOccurrences(of: "\n", with: " ")

        return cleaned
    }

    /// Extract high-level activity from tool call description
    private func extractHighLevelActivity(_ raw: String) -> String {
        // Map low-level tool operations to high-level activities
        if raw.contains("创建目录") || raw.contains("写入文件") || raw.contains("移动文件") {
            return "管理文件"
        } else if raw.contains("读取文件") {
            return "读取数据"
        } else if raw.contains("搜索") {
            return "搜索信息"
        } else if raw.contains("获取网页") {
            return "访问网络"
        } else if raw.contains("生成图像") {
            return "生成图像"
        } else if raw.contains("执行代码") || raw.contains("运行") {
            return "执行操作"
        }

        // For unknown operations, truncate the raw description
        if raw.count > 40 {
            let endIndex = raw.index(raw.startIndex, offsetBy: 37)
            return String(raw[..<endIndex]) + "..."
        }

        return raw
    }

    /// Clear current status
    func clearCurrentStatus() {
        statusMessages.removeAll()
        currentThinking = nil
        currentToolActivity = nil
        statusIsLoading = false
    }

    /// Clear all status (alias for clearCurrentStatus)
    func clearAllStatus() {
        clearCurrentStatus()
    }

    /// Natural language status for end users
    private func friendlyToolName(_ toolName: String) -> String {
        let baseName = toolName.components(separatedBy: ":").first ?? toolName

        switch baseName.lowercased() {
        case "file_ops", "file_operations", "read_file", "write_file":
            return "正在处理文件..."
        case "web_search", "search":
            return "正在帮你搜索..."
        case "code_exec", "execute_code", "shell", "bash", "terminal":
            return "正在执行..."
        case "generate_image", "image_gen":
            return "正在生成图片..."
        case "browser", "web_browse":
            return "正在浏览网页..."
        case "thinking", "analyze":
            return "思考中..."
        default:
            return "思考中..."
        }
    }

    // MARK: - Copy Actions

    func copyMessage(_ message: ConversationMessage) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(message.content, forType: .string)
    }

    func copyAllMessages() {
        let text = messages.map { msg in
            let prefix = msg.role == .user ? "User" : "Assistant"
            return "[\(prefix)]\n\(msg.content)"
        }.joined(separator: "\n\n")

        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    // MARK: - Reset

    func reset() {
        topic = nil
        messages = []
        inputText = ""
        turnCount = 0
        pendingAttachments = []
        commands = []
        topics = []
        filteredTopics = []
        selectedCommandIndex = 0
        selectedTopicIndex = 0
        isLoading = false
        errorMessage = nil
        displayState = .empty
        resetProgress()
        clearPendingPlanConfirmation()
        clearActiveParts()
        clearGatewayPendingQuestion()
        resetTokenUsage()
    }

    func clear() {
        reset()
    }

    /// Reset progress tracking state
    func resetProgress() {
        currentToolCall = nil
        planSteps = []
        currentStepIndex = 0
        clearAllStatus()
    }

    /// Update tool call status
    func setToolCallStarted(_ toolName: String) {
        print("[UnifiedViewModel] setToolCallStarted: \(toolName)")
        currentToolCall = toolName
        // Update current step status to running
        if currentStepIndex < planSteps.count {
            planSteps[currentStepIndex].status = .running
        }
        print("[UnifiedViewModel] currentToolCall is now: \(currentToolCall ?? "nil")")

        // Update status bar with friendly tool name
        let friendlyName = friendlyToolName(toolName)
        updateStatus(friendlyName, isLoading: true)
    }

    /// Mark tool call as completed
    func setToolCallCompleted() {
        // Update current step status to completed
        if currentStepIndex < planSteps.count {
            planSteps[currentStepIndex].status = .completed
        }
        currentToolCall = nil
        currentStepIndex += 1

        // Keep status showing completion state - don't clear immediately
        // The streaming text will take over if there's AI response, or
        // status will persist until next user message
        updateStatus("✓ 完成", isLoading: false)
    }

    /// Mark tool call as failed
    func setToolCallFailed(error: String? = nil) {
        let failedToolName = currentToolCall ?? "tool"

        // Update current step status to failed
        if currentStepIndex < planSteps.count {
            planSteps[currentStepIndex].status = .failed
        }
        currentToolCall = nil

        // Show error briefly
        let friendlyName = friendlyToolName(failedToolName)
        let errorText = error != nil ? "\(friendlyName) 失败" : "\(friendlyName) 失败"
        updateStatus(errorText, isLoading: false)
    }

    /// Set plan steps from notification
    func setPlanSteps(_ steps: [String]) {
        planSteps = steps.enumerated().map { index, description in
            PlanStep(id: "step_\(index)", description: description)
        }
        currentStepIndex = 0

        // Show first step in status bar
        if let firstStep = steps.first {
            updateStatus(firstStep, isLoading: true)
        }
    }

    // MARK: - Plan Confirmation Methods

    /// Set pending plan confirmation (shown inline in conversation)
    func setPendingPlanConfirmation(_ confirmation: PendingPlanConfirmation, core: AetherCore) {
        self.pendingPlanConfirmation = confirmation
        self.planConfirmationCore = core
        // Ensure we're showing the conversation to display the confirmation
        if displayState == .empty {
            displayState = .conversation
        }
    }

    /// Confirm the pending plan
    func confirmPendingPlan() {
        guard let confirmation = pendingPlanConfirmation,
              let core = planConfirmationCore else { return }

        let success = core.confirmTaskPlan(planId: confirmation.planId, confirmed: true)
        if !success {
            print("[UnifiedViewModel] Warning: Plan confirmation may have expired: \(confirmation.planId)")
        }

        // Clear the pending confirmation
        pendingPlanConfirmation = nil
        planConfirmationCore = nil
    }

    /// Cancel the pending plan
    func cancelPendingPlan() {
        guard let confirmation = pendingPlanConfirmation,
              let core = planConfirmationCore else { return }

        let success = core.confirmTaskPlan(planId: confirmation.planId, confirmed: false)
        if !success {
            print("[UnifiedViewModel] Warning: Plan confirmation may have expired: \(confirmation.planId)")
        }

        // Clear the pending confirmation
        pendingPlanConfirmation = nil
        planConfirmationCore = nil
    }

    /// Clear pending plan confirmation without sending decision (for reset)
    func clearPendingPlanConfirmation() {
        pendingPlanConfirmation = nil
        planConfirmationCore = nil
    }

    // MARK: - User Input Request Methods

    /// Set pending user input request (shown inline in conversation)
    func setPendingUserInputRequest(requestId: String, question: String, options: [String], core: AetherCore) {
        self.pendingUserInputRequest = PendingUserInputRequest(
            requestId: requestId,
            question: question,
            options: options
        )
        self.userInputCore = core
        // Ensure we're showing the conversation to display the input request
        if displayState == .empty {
            displayState = .conversation
        }
    }

    /// Respond to the pending user input request
    func respondToUserInput(response: String) {
        guard let request = pendingUserInputRequest,
              let core = userInputCore else { return }

        let success = core.respondToUserInput(requestId: request.requestId, response: response)
        if !success {
            print("[UnifiedViewModel] Warning: User input request may have expired: \(request.requestId)")
        }

        // Clear the pending request
        pendingUserInputRequest = nil
        userInputCore = nil
    }

    /// Cancel the pending user input request (respond with empty string)
    func cancelUserInputRequest() {
        guard let request = pendingUserInputRequest,
              let core = userInputCore else { return }

        let success = core.respondToUserInput(requestId: request.requestId, response: "")
        if !success {
            print("[UnifiedViewModel] Warning: User input request may have expired: \(request.requestId)")
        }

        // Clear the pending request
        pendingUserInputRequest = nil
        userInputCore = nil
    }

    /// Clear pending user input request without sending response (for reset)
    func clearPendingUserInputRequest() {
        pendingUserInputRequest = nil
        userInputCore = nil
    }

    // MARK: - Gateway User Question Methods

    /// Set pending Gateway user question (from AskUser event)
    func setGatewayPendingQuestion(_ event: AskUserEvent, adapter: GatewayMultiTurnAdapter) {
        self.gatewayPendingQuestion = event
        self.gatewayAdapter = adapter
        // Ensure we're showing the conversation to display the question
        if displayState == .empty {
            displayState = .conversation
        }
    }

    /// Submit answer for Gateway user question
    func submitGatewayAnswer(answers: [String: String]) async {
        guard let event = gatewayPendingQuestion,
              let adapter = gatewayAdapter else { return }

        do {
            try await adapter.submitAnswer(questionId: event.questionId, answers: answers)
            // Clear the pending question
            gatewayPendingQuestion = nil
            gatewayAdapter = nil
        } catch {
            print("[UnifiedViewModel] Failed to submit Gateway answer: \(error)")
            errorMessage = "Failed to submit answer: \(error.localizedDescription)"
        }
    }

    /// Cancel Gateway user question
    func cancelGatewayQuestion() {
        gatewayPendingQuestion = nil
        gatewayAdapter?.cancelQuestion()
        gatewayAdapter = nil
    }

    /// Clear Gateway pending question without action (for reset)
    func clearGatewayPendingQuestion() {
        gatewayPendingQuestion = nil
        gatewayAdapter = nil
    }

    // MARK: - Token Usage Methods

    /// Update token usage from Gateway RunSummary
    func updateTokenUsage(from summary: RunSummary) {
        totalTokens = summary.totalTokens
        toolCallCount = summary.toolCalls
        loopCount = summary.loops

        print("[UnifiedViewModel] Token usage updated: \(tokenUsageDisplay)")
    }

    /// Reset token usage
    func resetTokenUsage() {
        totalTokens = 0
        toolCallCount = 0
        loopCount = 0
    }

    /// Toggle token usage display
    func toggleTokenUsageDisplay() {
        showTokenUsage.toggle()
    }

    // MARK: - Part Update Handling (Message Flow)

    /// Handle Part update event from Rust core
    /// Enables Claude Code-style message flow rendering
    func handlePartUpdate(event: PartUpdateEventFfi) {
        let partType = event.partType
        let eventType = event.eventType

        print("[UnifiedViewModel] 🔔 Part Update received: type=\(partType), event=\(eventType), sessionId=\(event.sessionId)")
        print("[UnifiedViewModel] 🔔 Part Update JSON: \(event.partJson.prefix(200))...")
        print("[UnifiedViewModel] 🔔 Current state: statusMessages=\(statusMessages.count) items, statusIsLoading=\(statusIsLoading), activeToolCalls=\(activeToolCalls.count)")

        switch partType {
        case "tool_call":
            handleToolCallPartUpdate(event: event)

        case "reasoning":
            handleReasoningPartUpdate(event: event)

        case "plan", "plan_created":
            handlePlanPartUpdate(event: event)

        case "ai_response":
            handleStreamingPartUpdate(event: event)

        default:
            print("[UnifiedViewModel] ⚠️ Unknown part type: \(partType)")
        }

        print("[UnifiedViewModel] 🔔 After update: statusMessages=\(statusMessages.count) items, statusIsLoading=\(statusIsLoading), activeToolCalls=\(activeToolCalls.count)")
    }

    /// Handle tool call part updates
    private func handleToolCallPartUpdate(event: PartUpdateEventFfi) {
        // Phase 6: Apply user configuration filter
        guard partDisplayConfig.showToolCalls else {
            return  // User has disabled tool call display
        }

        switch event.eventType {
        case .added:
            // Parse and add new tool call
            if let data = event.partJson.data(using: .utf8),
               let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let part = ToolCallPart.fromJSON(json) {
                activeToolCalls.append(part)
                print("[UnifiedViewModel] Tool call added: \(part.toolName) (status: \(part.status))")

                // Update status bar
                updateStatus("Running: \(part.displayDescription)", isLoading: true)
            }

        case .updated:
            // Update existing tool call
            if let data = event.partJson.data(using: .utf8),
               let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let updatedPart = ToolCallPart.fromJSON(json),
               let index = activeToolCalls.firstIndex(where: { $0.id == updatedPart.id }) {
                activeToolCalls[index] = updatedPart
                print("[UnifiedViewModel] Tool call updated: \(updatedPart.toolName) (status: \(updatedPart.status))")

                // Update status bar based on status
                switch updatedPart.status {
                case .completed:
                    if let duration = updatedPart.durationMs {
                        updateStatus("\(updatedPart.toolName) completed (\(duration)ms)", isLoading: false)
                    } else {
                        updateStatus("\(updatedPart.toolName) completed", isLoading: false)
                    }
                case .failed:
                    updateStatus("\(updatedPart.toolName) failed", isLoading: false)
                default:
                    break
                }
            }

            // Prune terminal tool calls after update (using configured max)
            pruneTerminalToolCalls()

        case .removed:
            // Remove tool call
            activeToolCalls.removeAll { $0.id == event.partId }
            print("[UnifiedViewModel] Tool call removed: \(event.partId)")
        }
    }

    /// Prune terminal (completed/failed/aborted) tool calls (Phase 6: use configured max)
    private func pruneTerminalToolCalls() {
        let maxDisplayed = partDisplayConfig.maxRecentToolCalls

        // Separate running and terminal calls
        let running = activeToolCalls.filter { $0.status == .running }
        let terminal = activeToolCalls.filter {
            $0.status == .completed || $0.status == .failed || $0.status == .aborted
        }

        // Keep running + most recent N terminal (N from config)
        let recentTerminal = Array(terminal.suffix(maxDisplayed))
        activeToolCalls = running + recentTerminal
    }

    /// Handle streaming response part updates
    private func handleStreamingPartUpdate(event: PartUpdateEventFfi) {
        let partId = event.partId

        switch event.eventType {
        case .added:
            // Create new streaming part
            let part = StreamingResponsePart(
                id: partId,
                content: "",
                isComplete: false,
                startedAt: event.timestamp,
                completedAt: nil
            )
            streamingParts[partId] = part
            print("[UnifiedViewModel] Streaming part added: \(partId)")

        case .updated:
            // Handle delta update
            if let delta = event.delta, !delta.isEmpty {
                if var part = streamingParts[partId] {
                    part.appendDelta(delta)
                    streamingParts[partId] = part

                    // Update streaming text for display
                    updateStreamingText(part.content)
                }
            }

        case .removed:
            // Mark as complete and remove
            if var part = streamingParts[partId] {
                part.complete()
                streamingParts[partId] = nil
                print("[UnifiedViewModel] Streaming part completed: \(partId)")
            }
        }
    }

    /// Handle reasoning part updates (Phase 1: Part-driven UI)
    private func handleReasoningPartUpdate(event: PartUpdateEventFfi) {
        // Phase 6: Apply user configuration filter
        guard partDisplayConfig.showReasoning else {
            return  // User has disabled reasoning display
        }

        guard let data = event.partJson.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let part = ReasoningPart.fromJSON(json) else {
            print("[UnifiedViewModel] ⚠️ Failed to parse ReasoningPart from JSON")
            return
        }

        switch event.eventType {
        case .added:
            activeReasoningParts.append(part)
            print("[UnifiedViewModel] ReasoningPart added: step=\(part.step), content=\(part.content.prefix(50))...")

        case .updated:
            // Replace or append reasoning part based on step
            if let index = activeReasoningParts.firstIndex(where: { $0.step == part.step }) {
                activeReasoningParts[index] = part
            } else {
                activeReasoningParts.append(part)
            }
            print("[UnifiedViewModel] ReasoningPart updated: step=\(part.step)")

        case .removed:
            activeReasoningParts.removeAll { $0.id == part.id }
            print("[UnifiedViewModel] ReasoningPart removed: step=\(part.step)")
        }
    }

    /// Handle plan part updates (Phase 1: Part-driven UI)
    private func handlePlanPartUpdate(event: PartUpdateEventFfi) {
        // Phase 6: Apply user configuration filter
        guard partDisplayConfig.showPlan else {
            return  // User has disabled plan display
        }

        guard let data = event.partJson.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let part = PlanPart.fromJSON(json) else {
            print("[UnifiedViewModel] ⚠️ Failed to parse PlanPart from JSON")
            return
        }

        switch event.eventType {
        case .added:
            activePlanParts.append(part)
            print("[UnifiedViewModel] PlanPart added: id=\(part.id), steps=\(part.steps.count)")

        case .updated:
            if let index = activePlanParts.firstIndex(where: { $0.id == part.id }) {
                activePlanParts[index] = part
            } else {
                activePlanParts.append(part)
            }
            print("[UnifiedViewModel] PlanPart updated: id=\(part.id)")

        case .removed:
            activePlanParts.removeAll { $0.id == part.id }
            print("[UnifiedViewModel] PlanPart removed: id=\(part.id)")
        }
    }

    /// Clear all active parts (called on reset)
    func clearActiveParts() {
        activeToolCalls.removeAll()
        streamingParts.removeAll()
        activeReasoningParts.removeAll()
        activePlanParts.removeAll()
    }
}

// MARK: - User Input Request Model

/// Pending user input request from agent loop
struct PendingUserInputRequest: Sendable {
    let requestId: String
    let question: String
    let options: [String]

    var hasOptions: Bool {
        !options.isEmpty
    }
}
