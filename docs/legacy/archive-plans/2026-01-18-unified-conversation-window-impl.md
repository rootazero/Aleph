# Unified Conversation Window Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor multi-turn conversation mode from two-window system to single unified window with manual attachment handling.

**Architecture:** Replace `MultiTurnInputWindow` + `ConversationDisplayWindow` with a single `UnifiedConversationWindow`. The new window displays conversation history, command lists, and attachment previews above the input area. Window is positioned with input bottom at 70% screen height.

**Tech Stack:** Swift, SwiftUI, AppKit (NSWindow, NSOpenPanel), existing Aleph patterns (IMETextField, adaptiveGlass, etc.)

---

## Phase 1: Data Models & Attachment Manager

### Task 1.1: Create PendingAttachment Model

**Files:**
- Create: `Aether/Sources/MultiTurn/Models/PendingAttachment.swift`

**Step 1: Create the model file**

```swift
//
//  PendingAttachment.swift
//  Aleph
//
//  Data model for pending attachments in multi-turn conversation.
//

import AppKit
import Foundation

// MARK: - FileType

/// Type of attached file
enum AttachmentFileType {
    case image      // Show thumbnail preview
    case document   // Show file icon + name
    case other      // Generic file icon

    var iconName: String {
        switch self {
        case .image: return "photo"
        case .document: return "doc.text"
        case .other: return "doc"
        }
    }
}

// MARK: - PendingAttachment

/// Pending attachment waiting to be sent with message
struct PendingAttachment: Identifiable, Equatable {
    let id: UUID
    let url: URL
    let fileName: String
    let fileType: AttachmentFileType
    let thumbnail: NSImage?
    let data: Data

    init(url: URL) throws {
        self.id = UUID()
        self.url = url
        self.fileName = url.lastPathComponent
        self.data = try Data(contentsOf: url)
        self.fileType = Self.detectFileType(url: url)
        self.thumbnail = Self.generateThumbnail(url: url, fileType: self.fileType)
    }

    // MARK: - File Type Detection

    private static func detectFileType(url: URL) -> AttachmentFileType {
        let ext = url.pathExtension.lowercased()
        let imageExtensions = ["png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tiff"]
        let documentExtensions = ["pdf", "doc", "docx", "txt", "rtf", "md", "pages"]

        if imageExtensions.contains(ext) {
            return .image
        } else if documentExtensions.contains(ext) {
            return .document
        } else {
            return .other
        }
    }

    // MARK: - Thumbnail Generation

    private static func generateThumbnail(url: URL, fileType: AttachmentFileType) -> NSImage? {
        switch fileType {
        case .image:
            guard let image = NSImage(contentsOf: url) else { return nil }
            // Resize to thumbnail size (64x64)
            let targetSize = NSSize(width: 64, height: 64)
            let newImage = NSImage(size: targetSize)
            newImage.lockFocus()
            image.draw(
                in: NSRect(origin: .zero, size: targetSize),
                from: NSRect(origin: .zero, size: image.size),
                operation: .copy,
                fraction: 1.0
            )
            newImage.unlockFocus()
            return newImage
        case .document, .other:
            return NSWorkspace.shared.icon(forFile: url.path)
        }
    }

    // MARK: - Conversion to MediaAttachment

    /// Convert to MediaAttachment for sending to Rust core
    func toMediaAttachment() -> MediaAttachment {
        let mimeType: String
        switch fileType {
        case .image:
            let ext = url.pathExtension.lowercased()
            switch ext {
            case "png": mimeType = "image/png"
            case "jpg", "jpeg": mimeType = "image/jpeg"
            case "gif": mimeType = "image/gif"
            case "webp": mimeType = "image/webp"
            default: mimeType = "image/png"
            }
        case .document:
            let ext = url.pathExtension.lowercased()
            switch ext {
            case "pdf": mimeType = "application/pdf"
            case "txt", "md": mimeType = "text/plain"
            default: mimeType = "application/octet-stream"
            }
        case .other:
            mimeType = "application/octet-stream"
        }

        return MediaAttachment(mimeType: mimeType, data: data, filename: fileName)
    }

    // MARK: - Equatable

    static func == (lhs: PendingAttachment, rhs: PendingAttachment) -> Bool {
        lhs.id == rhs.id
    }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/Models/PendingAttachment.swift
git commit -m "feat(multi-turn): add PendingAttachment model for manual file attachments"
```

---

### Task 1.2: Create ContentDisplayState Enum

**Files:**
- Create: `Aether/Sources/MultiTurn/Models/ContentDisplayState.swift`

**Step 1: Create the state enum**

```swift
//
//  ContentDisplayState.swift
//  Aleph
//
//  Display state for unified conversation window content area.
//

import Foundation

// MARK: - ContentDisplayState

/// Mutually exclusive display states for the content area above input
enum ContentDisplayState: Equatable {
    /// No conversation, no commands - initial state
    case empty

    /// Showing conversation history
    case conversation

    /// Showing command or topic list
    case commandList(prefix: String)  // "/" for commands, "//" for topics

    /// Check if showing command list
    var isShowingCommandList: Bool {
        if case .commandList = self {
            return true
        }
        return false
    }

    /// Check if showing topic list (// prefix)
    var isShowingTopicList: Bool {
        if case .commandList(let prefix) = self {
            return prefix == "//"
        }
        return false
    }

    /// Check if showing command list (/ prefix, not //)
    var isShowingCommands: Bool {
        if case .commandList(let prefix) = self {
            return prefix == "/"
        }
        return false
    }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/Models/ContentDisplayState.swift
git commit -m "feat(multi-turn): add ContentDisplayState enum for display state machine"
```

---

## Phase 2: Unified ViewModel

### Task 2.1: Create UnifiedConversationViewModel

**Files:**
- Create: `Aether/Sources/MultiTurn/UnifiedConversationViewModel.swift`

**Step 1: Create the view model**

```swift
//
//  UnifiedConversationViewModel.swift
//  Aleph
//
//  View model for unified conversation window.
//  Manages display state, messages, attachments, commands, and input.
//

import AppKit
import Combine
import Foundation
import SwiftUI

// MARK: - UnifiedConversationViewModel

/// Unified view model for conversation window
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

    // MARK: - Core Reference

    private var core: AlephCore? {
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

    /// Whether to show attachment preview
    var shouldShowAttachmentPreview: Bool {
        !pendingAttachments.isEmpty
    }

    // MARK: - Display State Management

    /// Update display state based on input
    private func updateDisplayState() {
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
    func handleEscape() {
        if displayState.isShowingCommandList {
            // Layer 1: Close command/topic list
            inputText = ""
            displayState = hasMessages ? .conversation : .empty
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

    func addUserMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .user,
            content: content
        ) {
            messages.append(message)
            displayState = .conversation
        }
    }

    func addAssistantMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .assistant,
            content: content
        ) {
            messages.append(message)
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
        streamingText = text
        if let messageId = streamingMessageId,
           let index = messages.firstIndex(where: { $0.id == messageId }) {
            messages[index].content = text
        }
    }

    func finishStreamingMessage() {
        if let messageId = streamingMessageId,
           messages.contains(where: { $0.id == messageId }) {
            ConversationStore.shared.updateMessageContent(
                messageId: messageId,
                content: streamingText
            )
        }
        streamingMessageId = nil
        streamingText = ""
        isLoading = false
    }

    func setLoading(_ loading: Bool) {
        isLoading = loading
    }

    func setError(_ message: String?) {
        errorMessage = message
        isLoading = false
    }

    func reportHeightChange(_ height: CGFloat) {
        onHeightChanged?(height)
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
    }

    func clear() {
        reset()
    }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/UnifiedConversationViewModel.swift
git commit -m "feat(multi-turn): add UnifiedConversationViewModel with attachment support"
```

---

## Phase 3: View Components

### Task 3.1: Create AttachmentPreviewView

**Files:**
- Create: `Aether/Sources/MultiTurn/Views/AttachmentPreviewView.swift`

**Step 1: Create the attachment preview component**

```swift
//
//  AttachmentPreviewView.swift
//  Aleph
//
//  Attachment preview component for unified conversation window.
//

import SwiftUI

// MARK: - AttachmentPreviewView

/// Horizontal scrollable list of pending attachments
struct AttachmentPreviewView: View {
    let attachments: [PendingAttachment]
    let onRemove: (PendingAttachment) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 12) {
                ForEach(attachments) { attachment in
                    AttachmentThumbnailView(
                        attachment: attachment,
                        onRemove: { onRemove(attachment) }
                    )
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
    }
}

// MARK: - AttachmentThumbnailView

/// Individual attachment thumbnail with remove button
struct AttachmentThumbnailView: View {
    let attachment: PendingAttachment
    let onRemove: () -> Void

    @State private var isHovering = false

    private let thumbnailSize: CGFloat = 64

    var body: some View {
        ZStack(alignment: .topTrailing) {
            // Thumbnail content
            VStack(spacing: 4) {
                thumbnailImage
                    .frame(width: thumbnailSize, height: thumbnailSize)
                    .clipShape(RoundedRectangle(cornerRadius: 8))

                Text(attachment.fileName)
                    .font(.caption2)
                    .foregroundColor(.primary.opacity(0.7))
                    .lineLimit(1)
                    .frame(maxWidth: thumbnailSize + 16)
            }
            .padding(4)
            .background(
                RoundedRectangle(cornerRadius: 10)
                    .fill(.primary.opacity(isHovering ? 0.1 : 0.05))
            )

            // Remove button
            if isHovering {
                Button(action: onRemove) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 18))
                        .foregroundColor(.primary.opacity(0.7))
                        .background(Circle().fill(.background))
                }
                .buttonStyle(.plain)
                .offset(x: 6, y: -6)
                .transition(.scale.combined(with: .opacity))
            }
        }
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
    }

    @ViewBuilder
    private var thumbnailImage: some View {
        if let thumbnail = attachment.thumbnail {
            Image(nsImage: thumbnail)
                .resizable()
                .aspectRatio(contentMode: .fill)
        } else {
            Image(systemName: attachment.fileType.iconName)
                .font(.system(size: 28))
                .foregroundColor(.primary.opacity(0.6))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(.primary.opacity(0.05))
        }
    }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/Views/AttachmentPreviewView.swift
git commit -m "feat(multi-turn): add AttachmentPreviewView component"
```

---

### Task 3.2: Create InputAreaView

**Files:**
- Create: `Aether/Sources/MultiTurn/Views/InputAreaView.swift`

**Step 1: Create the input area component**

```swift
//
//  InputAreaView.swift
//  Aleph
//
//  Input area component with text field, attachment button, and send button.
//

import SwiftUI
import UniformTypeIdentifiers

// MARK: - InputAreaView

/// Input area with text field, attachment button, and send button
struct InputAreaView: View {
    @Bindable var viewModel: UnifiedConversationViewModel

    var body: some View {
        HStack(spacing: 12) {
            // Turn indicator
            if viewModel.turnCount > 0 {
                Text("Turn \(viewModel.turnCount + 1)")
                    .font(.caption)
                    .foregroundColor(.primary.opacity(0.7))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(.primary.opacity(0.1))
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            }

            // Text field
            IMETextField(
                text: $viewModel.inputText,
                placeholder: NSLocalizedString("multiturn.input.placeholder", comment: ""),
                font: .systemFont(ofSize: 16),
                textColor: .labelColor,
                placeholderColor: NSColor.secondaryLabelColor,
                backgroundColor: .clear,
                autoFocus: true,
                onSubmit: { viewModel.submit() },
                onEscape: { viewModel.handleEscape() },
                onTextChange: { _ in viewModel.refreshDisplayState() },
                onArrowUp: { viewModel.moveSelectionUp() },
                onArrowDown: { viewModel.moveSelectionDown() },
                onTab: { viewModel.handleTab() }
            )
            .frame(height: 24)

            // Attachment button
            AttachmentButton(onFilesSelected: viewModel.addAttachments)

            // Submit button
            Button(action: viewModel.submit) {
                Image(systemName: "arrow.up")
            }
            .buttonStyle(GlassProminentButtonStyle())
            .disabled(
                viewModel.inputText.trimmingCharacters(in: .whitespaces).isEmpty &&
                viewModel.pendingAttachments.isEmpty
            )
        }
        .padding(16)
    }
}

// MARK: - AttachmentButton

/// Button to add attachments via file picker or drag-drop
struct AttachmentButton: View {
    let onFilesSelected: ([URL]) -> Void

    @State private var isHovering = false
    @State private var isTargeted = false

    var body: some View {
        Button(action: openFilePicker) {
            Image(systemName: "plus")
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(.primary.opacity(isHovering ? 1.0 : 0.7))
        }
        .buttonStyle(.plain)
        .frame(width: 28, height: 28)
        .background(
            RoundedRectangle(cornerRadius: 6)
                .fill(.primary.opacity(isTargeted ? 0.2 : (isHovering ? 0.1 : 0.05)))
        )
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.1)) {
                isHovering = hovering
            }
        }
        .onDrop(of: [.fileURL], isTargeted: $isTargeted) { providers in
            handleDrop(providers: providers)
        }
        .help(NSLocalizedString("multiturn.attachment.add", comment: ""))
    }

    private func openFilePicker() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.allowedContentTypes = [
            .image, .pdf, .plainText, .rtf,
            UTType(filenameExtension: "md") ?? .plainText
        ]

        if panel.runModal() == .OK {
            onFilesSelected(panel.urls)
        }
    }

    private func handleDrop(providers: [NSItemProvider]) -> Bool {
        var urls: [URL] = []
        let group = DispatchGroup()

        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                group.enter()
                provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { item, _ in
                    if let data = item as? Data,
                       let url = URL(dataRepresentation: data, relativeTo: nil) {
                        urls.append(url)
                    }
                    group.leave()
                }
            }
        }

        group.notify(queue: .main) {
            if !urls.isEmpty {
                onFilesSelected(urls)
            }
        }

        return true
    }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/Views/InputAreaView.swift
git commit -m "feat(multi-turn): add InputAreaView with attachment button"
```

---

### Task 3.3: Create ConversationAreaView

**Files:**
- Create: `Aether/Sources/MultiTurn/Views/ConversationAreaView.swift`

**Step 1: Create the conversation area component**

```swift
//
//  ConversationAreaView.swift
//  Aleph
//
//  Conversation history display area for unified window.
//

import SwiftUI

// MARK: - ConversationAreaView

/// Scrollable conversation history with title bar
struct ConversationAreaView: View {
    @Bindable var viewModel: UnifiedConversationViewModel
    let maxHeight: CGFloat

    @State private var contentHeight: CGFloat = 0

    private let titleBarHeight: CGFloat = 44

    var body: some View {
        VStack(spacing: 0) {
            // Title bar
            titleBar

            Divider()
                .opacity(0.3)

            // Messages list
            if viewModel.hasMessages {
                messagesList
            } else {
                emptyState
            }

            // Loading indicator
            if viewModel.isLoading {
                loadingIndicator
            }

            // Error banner
            if let error = viewModel.errorMessage {
                errorBanner(error)
            }
        }
        .frame(maxHeight: maxHeight)
    }

    // MARK: - Title Bar

    private var titleBar: some View {
        HStack {
            Text(viewModel.displayTitle)
                .font(.headline)
                .foregroundColor(.primary)
                .lineLimit(1)

            Spacer()

            Button(action: viewModel.copyAllMessages) {
                Image(systemName: "doc.on.doc")
                    .font(.system(size: 12))
                    .foregroundColor(.primary.opacity(0.7))
            }
            .buttonStyle(.plain)
            .adaptiveGlassButton()
            .help(NSLocalizedString("conversation.copy.all", comment: ""))
            .disabled(!viewModel.hasMessages)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }

    // MARK: - Messages List

    private var messagesList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 12) {
                    ForEach(viewModel.messages) { message in
                        MessageBubbleView(
                            message: message,
                            onCopy: { viewModel.copyMessage(message) }
                        )
                        .id(message.id)
                    }
                }
                .padding(12)
                .background(
                    GeometryReader { geometry in
                        Color.clear
                            .onChange(of: geometry.size.height) { _, newHeight in
                                contentHeight = newHeight
                                let total = titleBarHeight + 1 + newHeight +
                                    (viewModel.isLoading ? 30 : 0)
                                viewModel.reportHeightChange(total)
                            }
                    }
                )
            }
            .onChange(of: viewModel.messages.count) {
                if let lastId = viewModel.messages.last?.id {
                    withAnimation {
                        proxy.scrollTo(lastId, anchor: .bottom)
                    }
                }
            }
        }
    }

    // MARK: - Empty State

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 32))
                .foregroundColor(.primary.opacity(0.6))

            Text(NSLocalizedString("conversation.empty", comment: ""))
                .font(.subheadline)
                .foregroundColor(.primary.opacity(0.7))
        }
        .frame(maxWidth: .infinity)
        .frame(height: 100)
        .padding()
    }

    // MARK: - Loading Indicator

    private var loadingIndicator: some View {
        HStack(spacing: 6) {
            ForEach(0..<3, id: \.self) { _ in
                Circle()
                    .fill(.primary.opacity(0.5))
                    .frame(width: 6, height: 6)
            }
        }
        .padding(.vertical, 10)
    }

    // MARK: - Error Banner

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle")
            Text(message)
                .font(.caption)
        }
        .foregroundColor(.red)
        .padding(10)
        .background(.red.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
        .padding(12)
    }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/Views/ConversationAreaView.swift
git commit -m "feat(multi-turn): add ConversationAreaView component"
```

---

### Task 3.4: Create CommandListView

**Files:**
- Create: `Aether/Sources/MultiTurn/Views/CommandListView.swift`

**Step 1: Create the command list component**

```swift
//
//  CommandListView.swift
//  Aleph
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
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/Views/CommandListView.swift
git commit -m "feat(multi-turn): add CommandListView and TopicListView components"
```

---

## Phase 4: Unified Window & Main View

### Task 4.1: Create UnifiedConversationView

**Files:**
- Create: `Aether/Sources/MultiTurn/UnifiedConversationView.swift`

**Step 1: Create the main unified view**

```swift
//
//  UnifiedConversationView.swift
//  Aleph
//
//  Main SwiftUI view for unified conversation window.
//  Displays conversation/commands/topics above input, with attachment preview.
//

import SwiftUI

// MARK: - UnifiedConversationView

/// Main view for unified conversation window
struct UnifiedConversationView: View {
    @Bindable var viewModel: UnifiedConversationViewModel

    /// Maximum height for content area (conversation or command list)
    private let maxContentHeight: CGFloat = 600

    /// Height for attachment preview
    private let attachmentPreviewHeight: CGFloat = 100

    var body: some View {
        VStack(spacing: 0) {
            // Spacer pushes content to bottom
            Spacer(minLength: 0)

            // Main content with glass background
            contentWithBackground
        }
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            handleDrop(providers: providers)
        }
    }

    // MARK: - Content with Background

    private var contentWithBackground: some View {
        VStack(spacing: 0) {
            // Content area (mutually exclusive)
            contentArea

            // Attachment preview (if any)
            if viewModel.shouldShowAttachmentPreview {
                Divider().opacity(0.3)
                AttachmentPreviewView(
                    attachments: viewModel.pendingAttachments,
                    onRemove: viewModel.removeAttachment
                )
            }

            // Divider before input
            if viewModel.shouldShowConversation ||
               viewModel.shouldShowCommandList ||
               viewModel.shouldShowTopicList ||
               viewModel.shouldShowAttachmentPreview {
                Divider().opacity(0.3)
            }

            // Input area (always visible)
            InputAreaView(viewModel: viewModel)
        }
        .frame(width: 800)
        .adaptiveGlass()
        .animation(.smooth(duration: 0.25), value: viewModel.displayState)
        .animation(.smooth(duration: 0.25), value: viewModel.shouldShowAttachmentPreview)
    }

    // MARK: - Content Area (Mutually Exclusive)

    @ViewBuilder
    private var contentArea: some View {
        switch viewModel.displayState {
        case .empty:
            EmptyView()

        case .conversation:
            ConversationAreaView(
                viewModel: viewModel,
                maxHeight: maxContentHeight
            )

        case .commandList(let prefix):
            if prefix == "//" {
                TopicListView(
                    viewModel: viewModel,
                    maxHeight: maxContentHeight
                )
            } else {
                CommandListView(
                    viewModel: viewModel,
                    maxHeight: maxContentHeight
                )
            }
        }
    }

    // MARK: - Drag & Drop

    private func handleDrop(providers: [NSItemProvider]) -> Bool {
        var urls: [URL] = []
        let group = DispatchGroup()

        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier("public.file-url") {
                group.enter()
                provider.loadItem(forTypeIdentifier: "public.file-url", options: nil) { item, _ in
                    if let data = item as? Data,
                       let url = URL(dataRepresentation: data, relativeTo: nil) {
                        urls.append(url)
                    }
                    group.leave()
                }
            }
        }

        group.notify(queue: .main) {
            if !urls.isEmpty {
                viewModel.addAttachments(urls: urls)
            }
        }

        return true
    }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/UnifiedConversationView.swift
git commit -m "feat(multi-turn): add UnifiedConversationView main view"
```

---

### Task 4.2: Create UnifiedConversationWindow

**Files:**
- Create: `Aether/Sources/MultiTurn/UnifiedConversationWindow.swift`

**Step 1: Create the unified window**

```swift
//
//  UnifiedConversationWindow.swift
//  Aleph
//
//  Unified NSWindow for multi-turn conversation.
//  Replaces separate input and display windows.
//

import Cocoa
import SwiftUI

// MARK: - UnifiedConversationWindow

/// Unified window for multi-turn conversation
final class UnifiedConversationWindow: NSWindow {

    // MARK: - Constants

    private enum Layout {
        static let width: CGFloat = 800
        static let inputAreaHeight: CGFloat = 60
        static let maxContentHeight: CGFloat = 600
        static let attachmentPreviewHeight: CGFloat = 100
    }

    // MARK: - Properties

    /// View model
    let viewModel = UnifiedConversationViewModel()

    /// Hosting view
    private var hostingView: NSHostingView<UnifiedConversationView>?

    /// ESC key monitor
    private var escapeMonitor: Any?

    /// Callbacks
    var onSubmit: ((String, [PendingAttachment]) -> Void)?
    var onCancel: (() -> Void)?
    var onTopicSelected: ((Topic) -> Void)?

    // MARK: - Initialization

    init() {
        // Start with minimal height (just input area)
        let initialHeight = Layout.inputAreaHeight + 32  // padding

        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Layout.width, height: initialHeight),
            styleMask: [.borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupHostingView()
        setupCallbacks()
        setupEscapeHandler()
    }

    deinit {
        if let monitor = escapeMonitor {
            NSEvent.removeMonitor(monitor)
        }
    }

    // MARK: - Window Setup

    private func setupWindow() {
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        alphaValue = 0  // Start hidden

        collectionBehavior = [.canJoinAllSpaces, .stationary]
        hidesOnDeactivate = false
        isMovableByWindowBackground = true

        titlebarAppearsTransparent = true
        titleVisibility = .hidden
    }

    private func setupHostingView() {
        let view = UnifiedConversationView(viewModel: viewModel)
        hostingView = NSHostingView(rootView: view)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]
            contentView = hostingView
        }

        // Height change callback
        viewModel.onHeightChanged = { [weak self] height in
            DispatchQueue.main.async {
                self?.updateWindowHeight(contentHeight: height)
            }
        }
    }

    private func setupCallbacks() {
        viewModel.onSubmit = { [weak self] text, attachments in
            self?.onSubmit?(text, attachments)
        }
        viewModel.onCancel = { [weak self] in
            self?.onCancel?()
        }
        viewModel.onTopicSelected = { [weak self] topic in
            self?.onTopicSelected?(topic)
        }
    }

    private func setupEscapeHandler() {
        escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if event.keyCode == 53 && self?.isVisible == true {
                self?.viewModel.handleEscape()
                return nil
            }
            return event
        }
    }

    // MARK: - Positioning

    /// Show window centered with input bottom at 70% screen height
    func showPositioned() {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.frame

        // Input bottom at 70% from top (30% from bottom)
        let anchorY = screenFrame.height * 0.30

        // Calculate initial window height
        let windowHeight = calculateWindowHeight()

        // Position window
        let origin = NSPoint(
            x: screenFrame.midX - Layout.width / 2,
            y: anchorY  // Window bottom at anchor
        )

        setFrame(NSRect(origin: origin, size: NSSize(width: Layout.width, height: windowHeight)), display: true)
        alphaValue = 0

        // Activate and show
        NSApp.activate(ignoringOtherApps: true)
        makeKeyAndOrderFront(nil)

        // Fade in
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }
    }

    /// Calculate window height based on content
    private func calculateWindowHeight() -> CGFloat {
        var height = Layout.inputAreaHeight + 32  // Base + padding

        // Add content area height
        if viewModel.shouldShowConversation ||
           viewModel.displayState.isShowingCommandList {
            height += min(viewModel.messages.count > 0 ? 200 : 0, Layout.maxContentHeight)
        }

        // Add attachment preview
        if viewModel.shouldShowAttachmentPreview {
            height += Layout.attachmentPreviewHeight
        }

        return height
    }

    /// Update window height and keep bottom anchored
    private func updateWindowHeight(contentHeight: CGFloat) {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.frame
        let anchorY = screenFrame.height * 0.30

        // Calculate new height
        var newHeight = Layout.inputAreaHeight + 32

        // Add content height (clamped)
        newHeight += min(contentHeight, Layout.maxContentHeight)

        // Add attachment preview if needed
        if viewModel.shouldShowAttachmentPreview {
            newHeight += Layout.attachmentPreviewHeight
        }

        // Update frame keeping bottom at anchor
        let newFrame = NSRect(
            x: frame.origin.x,
            y: anchorY,  // Keep bottom at anchor
            width: Layout.width,
            height: newHeight
        )

        setFrame(newFrame, display: true, animate: true)
    }

    // MARK: - Hide

    func hide() {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            self?.orderOut(nil)
            self?.viewModel.reset()
        })
    }

    // MARK: - State

    func updateTurnCount(_ count: Int) {
        viewModel.turnCount = count
    }

    // MARK: - Focus

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/UnifiedConversationWindow.swift
git commit -m "feat(multi-turn): add UnifiedConversationWindow"
```

---

## Phase 5: Coordinator Refactoring

### Task 5.1: Refactor MultiTurnCoordinator

**Files:**
- Modify: `Aether/Sources/MultiTurn/MultiTurnCoordinator.swift`

**Step 1: Replace window references and update input handling**

Replace the entire file with updated version that:
1. Uses `UnifiedConversationWindow` instead of separate windows
2. Removes clipboard auto-reading
3. Handles attachments from the new system

See the detailed changes in the implementation - key modifications:

```swift
// Replace:
private lazy var inputWindow: MultiTurnInputWindow = { ... }
private lazy var displayWindow: ConversationDisplayWindow = { ... }

// With:
private lazy var unifiedWindow: UnifiedConversationWindow = {
    let window = UnifiedConversationWindow()
    window.onSubmit = { [weak self] text, attachments in
        self?.handleInput(text, attachments: attachments)
    }
    window.onCancel = { [weak self] in
        self?.exit()
    }
    window.onTopicSelected = { [weak self] topic in
        self?.loadTopic(topic)
    }
    return window
}()
```

```swift
// Update handleInput to accept attachments parameter:
private func handleInput(_ text: String, attachments: [PendingAttachment]) {
    // Remove clipboard reading logic
    // Convert PendingAttachment to MediaAttachment
    let mediaAttachments = attachments.map { $0.toMediaAttachment() }
    // ... rest of processing
}
```

**Step 2: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | head -50`

**Step 3: Commit**

```bash
git add Aleph/Sources/MultiTurn/MultiTurnCoordinator.swift
git commit -m "refactor(multi-turn): update coordinator to use UnifiedConversationWindow"
```

---

## Phase 6: Localization

### Task 6.1: Add Localization Strings

**Files:**
- Modify: `Aether/Resources/en.lproj/Localizable.strings`
- Modify: `Aether/Resources/zh-Hans.lproj/Localizable.strings`

**Step 1: Add English strings**

```
// Multi-turn Unified Window
"multiturn.input.placeholder" = "Type a message... (/ for commands, // for topics)";
"multiturn.attachment.add" = "Add attachment";
"conversation.copy.all" = "Copy all messages";
"conversation.empty" = "Start a conversation";
"commands.empty" = "No commands found";
"topics.empty" = "No topics found";
```

**Step 2: Add Chinese strings**

```
// Multi-turn Unified Window
"multiturn.input.placeholder" = "输入消息... (/ 命令, // 历史)";
"multiturn.attachment.add" = "添加附件";
"conversation.copy.all" = "复制所有消息";
"conversation.empty" = "开始新对话";
"commands.empty" = "未找到命令";
"topics.empty" = "未找到主题";
```

**Step 3: Commit**

```bash
git add Aleph/Resources/*/Localizable.strings
git commit -m "i18n: add localization strings for unified conversation window"
```

---

## Phase 7: Cleanup

### Task 7.1: Mark Deprecated Files

**Files:**
- Modify: `Aether/Sources/MultiTurn/MultiTurnInputWindow.swift`
- Modify: `Aether/Sources/MultiTurn/ConversationDisplayWindow.swift`
- Modify: `Aether/Sources/MultiTurn/ConversationDisplayView.swift`

**Step 1: Add deprecation notices**

Add `@available(*, deprecated, message: "Use UnifiedConversationWindow instead")` to classes.

**Step 2: Commit**

```bash
git add Aleph/Sources/MultiTurn/*.swift
git commit -m "refactor(multi-turn): mark deprecated window files"
```

---

### Task 7.2: Update project.yml

**Files:**
- Modify: `project.yml`

**Step 1: Add new source files to project if needed**

Ensure all new files are included in the sources section.

**Step 2: Regenerate project**

Run: `xcodegen generate`

**Step 3: Commit**

```bash
git add project.yml
git commit -m "chore: update project.yml for unified conversation window"
```

---

## Phase 8: Testing

### Task 8.1: Manual Testing Checklist

1. **Window Display**
   - [ ] Window appears at correct position (input bottom at 70% height)
   - [ ] Window is 800px wide
   - [ ] Window is horizontally centered

2. **Input Area**
   - [ ] Text input works with IME
   - [ ] Attachment button opens file picker
   - [ ] Send button is disabled when empty
   - [ ] Send button enabled with text OR attachments

3. **Attachments**
   - [ ] Click + to add files
   - [ ] Drag files to window
   - [ ] Preview shows thumbnails for images
   - [ ] Preview shows icons for documents
   - [ ] Click X to remove attachment
   - [ ] Multiple attachments supported

4. **Conversation Area**
   - [ ] Hidden initially
   - [ ] Appears after first message
   - [ ] Scrolls to latest message
   - [ ] Max height 600px
   - [ ] Copy button works

5. **Command List**
   - [ ] Type / to show commands
   - [ ] Filters as you type
   - [ ] Arrow keys navigate
   - [ ] Tab/Enter selects
   - [ ] Replaces conversation area

6. **Topic List**
   - [ ] Type // to show topics
   - [ ] Filters as you type
   - [ ] Can rename/delete topics
   - [ ] Selecting loads topic

7. **ESC Behavior**
   - [ ] ESC closes command list first
   - [ ] ESC then closes window
   - [ ] ESC from conversation closes window

---

## Summary

**Files Created:**
- `Aether/Sources/MultiTurn/Models/PendingAttachment.swift`
- `Aether/Sources/MultiTurn/Models/ContentDisplayState.swift`
- `Aether/Sources/MultiTurn/UnifiedConversationViewModel.swift`
- `Aether/Sources/MultiTurn/UnifiedConversationView.swift`
- `Aether/Sources/MultiTurn/UnifiedConversationWindow.swift`
- `Aether/Sources/MultiTurn/Views/AttachmentPreviewView.swift`
- `Aether/Sources/MultiTurn/Views/InputAreaView.swift`
- `Aether/Sources/MultiTurn/Views/ConversationAreaView.swift`
- `Aether/Sources/MultiTurn/Views/CommandListView.swift`

**Files Modified:**
- `Aether/Sources/MultiTurn/MultiTurnCoordinator.swift`
- `Aether/Resources/en.lproj/Localizable.strings`
- `Aether/Resources/zh-Hans.lproj/Localizable.strings`

**Files Deprecated:**
- `Aether/Sources/MultiTurn/MultiTurnInputWindow.swift`
- `Aether/Sources/MultiTurn/ConversationDisplayWindow.swift`
- `Aether/Sources/MultiTurn/ConversationDisplayView.swift`
