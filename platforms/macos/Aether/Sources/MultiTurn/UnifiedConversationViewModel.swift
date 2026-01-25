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

    // MARK: - Progress Tracking (for multi-turn agentic tasks)

    /// Currently executing tool name (nil when not executing)
    var currentToolCall: String?

    /// Task plan steps for multi-step tasks
    var planSteps: [PlanStep] = []

    /// Current step index in the plan
    var currentStepIndex: Int = 0

    // MARK: - Status Bar Display (minimal single-line)

    /// Current status text - plain language description of what Agent is doing
    var statusText: String = ""

    /// Whether status bar is showing a loading/thinking state
    var statusIsLoading: Bool = false

    // MARK: - Message Flow Parts (Claude Code-style rendering)

    /// Active tool call parts (for real-time status display)
    var activeToolCalls: [ToolCallPart] = []

    /// Streaming response parts (for delta-based updates)
    var streamingParts: [String: StreamingResponsePart] = [:]

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
        self.errorMessage = nil
        self.displayState = messages.isEmpty ? .empty : .conversation
    }

    /// Clear current topic (for new session before first message)
    func clearTopic() {
        self.topic = nil
        self.messages = []
        self.errorMessage = nil
        self.displayState = .empty
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

        // Update status to show completion, keep visible until next message
        // Status will be cleared/overwritten by the next user message or operation
        updateStatus("✓ 完成", isLoading: false)
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
                // For local files (tool-generated), copy to attachments directory
                if let localPath = AttachmentFileManager.shared.saveGeneratedFile(
                    from: url,
                    toolName: "tool",
                    messageId: messageId
                ) {
                    let stored = StoredAttachment.forToolOutput(
                        messageId: messageId,
                        toolName: "tool",
                        sourceURL: url,
                        localPath: localPath
                    )
                    AttachmentStore.shared.save(stored)
                    print("[UnifiedViewModel] Saved local tool output: \(localPath)")
                }
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
    }

    func reportHeightChange(_ height: CGFloat) {
        lastContentHeight = height
        // Status bar height (20px) is included in conversation area layout
        // Add status bar height when conversation is visible
        let statusBarHeight: CGFloat = shouldShowConversation ? 20 : 0
        onHeightChanged?(height + statusBarHeight)
    }

    // MARK: - Status Bar Management

    /// Update status text (for streaming AI response)
    func updateStatus(_ text: String, isLoading: Bool = true) {
        statusText = text
        statusIsLoading = isLoading
        // No height refresh needed - status bar height is fixed
    }

    /// Clear current status
    func clearCurrentStatus() {
        statusText = ""
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

    // MARK: - Part Update Handling (Message Flow)

    /// Handle Part update event from Rust core
    /// Enables Claude Code-style message flow rendering
    func handlePartUpdate(event: PartUpdateEventFfi) {
        let partType = event.partType

        switch partType {
        case "tool_call":
            handleToolCallPartUpdate(event: event)

        case "ai_response", "reasoning":
            handleStreamingPartUpdate(event: event)

        default:
            print("[UnifiedViewModel] Unknown part type: \(partType)")
        }
    }

    /// Handle tool call part updates
    private func handleToolCallPartUpdate(event: PartUpdateEventFfi) {
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

        case .removed:
            // Remove tool call
            activeToolCalls.removeAll { $0.id == event.partId }
            print("[UnifiedViewModel] Tool call removed: \(event.partId)")
        }
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

    /// Clear all active parts (called on reset)
    func clearActiveParts() {
        activeToolCalls.removeAll()
        streamingParts.removeAll()
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
