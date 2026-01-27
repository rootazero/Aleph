# Multi-Turn Conversation Mode Separation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Completely separate single-turn and multi-turn conversation modes, with multi-turn having independent floating windows, SQLite persistence, and topic management.

**Architecture:** New `MultiTurnCoordinator` manages the multi-turn flow independently. `ConversationStore` (Swift) handles SQLite persistence. Two new windows: `MultiTurnInputWindow` (centered input) and `ConversationDisplayWindow` (top-right floating).

**Tech Stack:** Swift/SwiftUI, SQLite (via GRDB.swift), Rust core (for AI calls + title generation)

**Design Doc:** `docs/plans/2026-01-12-multi-turn-separation-design.md`

---

## Phase 1: Data Layer (Swift SQLite)

### Task 1: Add GRDB.swift Dependency

**Files:**
- Modify: `project.yml`

**Step 1: Add GRDB package to project.yml**

In `project.yml`, add to `packages` section:

```yaml
packages:
  GRDB:
    url: https://github.com/groue/GRDB.swift.git
    from: "6.24.0"
```

And add to target dependencies:

```yaml
targets:
  Aether:
    dependencies:
      - package: GRDB
```

**Step 2: Regenerate Xcode project**

Run: `xcodegen generate`
Expected: Project regenerated with GRDB dependency

**Step 3: Commit**

```bash
git add project.yml
git commit -m "feat: add GRDB.swift dependency for conversation persistence"
```

---

### Task 2: Create ConversationStore - Database Setup

**Files:**
- Create: `Aether/Sources/Store/ConversationStore.swift`
- Create: `Aether/Sources/Store/ConversationModels.swift`

**Step 1: Create ConversationModels.swift**

```swift
//
//  ConversationModels.swift
//  Aether
//
//  Data models for conversation persistence.
//

import Foundation
import GRDB

// MARK: - Topic

/// A conversation topic (session)
struct Topic: Identifiable, Codable, FetchableRecord, PersistableRecord {
    var id: String
    var title: String
    var createdAt: Date
    var updatedAt: Date
    var isDeleted: Bool

    static let databaseTableName = "topics"

    init(id: String = UUID().uuidString, title: String = "New Conversation") {
        self.id = id
        self.title = title
        self.createdAt = Date()
        self.updatedAt = Date()
        self.isDeleted = false
    }
}

// MARK: - Message

/// A single message in a conversation
struct ConversationMessage: Identifiable, Codable, FetchableRecord, PersistableRecord {
    var id: String
    var topicId: String
    var role: MessageRole
    var content: String
    var createdAt: Date

    static let databaseTableName = "messages"

    init(id: String = UUID().uuidString, topicId: String, role: MessageRole, content: String) {
        self.id = id
        self.topicId = topicId
        self.role = role
        self.content = content
        self.createdAt = Date()
    }
}

// MARK: - MessageRole

enum MessageRole: String, Codable, DatabaseValueConvertible {
    case user
    case assistant
}
```

**Step 2: Create ConversationStore.swift with database setup**

```swift
//
//  ConversationStore.swift
//  Aether
//
//  SQLite persistence for multi-turn conversations.
//

import Foundation
import GRDB

// MARK: - ConversationStore

/// Manages SQLite persistence for conversations
final class ConversationStore {

    // MARK: - Singleton

    static let shared = ConversationStore()

    // MARK: - Properties

    private var dbQueue: DatabaseQueue?

    // MARK: - Initialization

    private init() {
        setupDatabase()
    }

    // MARK: - Database Setup

    private func setupDatabase() {
        do {
            let dbPath = getDBPath()
            dbQueue = try DatabaseQueue(path: dbPath)
            try createTables()
            print("[ConversationStore] Database initialized at: \(dbPath)")
        } catch {
            print("[ConversationStore] Failed to setup database: \(error)")
        }
    }

    private func getDBPath() -> String {
        let configDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".aether")

        // Create directory if needed
        try? FileManager.default.createDirectory(
            at: configDir,
            withIntermediateDirectories: true
        )

        return configDir.appendingPathComponent("conversations.db").path
    }

    private func createTables() throws {
        try dbQueue?.write { db in
            // Topics table
            try db.create(table: "topics", ifNotExists: true) { t in
                t.column("id", .text).primaryKey()
                t.column("title", .text).notNull()
                t.column("createdAt", .datetime).notNull()
                t.column("updatedAt", .datetime).notNull()
                t.column("isDeleted", .boolean).notNull().defaults(to: false)
            }

            // Messages table
            try db.create(table: "messages", ifNotExists: true) { t in
                t.column("id", .text).primaryKey()
                t.column("topicId", .text).notNull().references("topics", onDelete: .cascade)
                t.column("role", .text).notNull()
                t.column("content", .text).notNull()
                t.column("createdAt", .datetime).notNull()
            }

            // Indexes
            try db.create(index: "idx_messages_topic", on: "messages", columns: ["topicId"], ifNotExists: true)
            try db.create(index: "idx_topics_updated", on: "topics", columns: ["updatedAt"], ifNotExists: true)
        }
    }
}
```

**Step 3: Verify compilation**

Run: `xcodegen generate && xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | tail -5`
Expected: BUILD SUCCEEDED

**Step 4: Commit**

```bash
git add Aether/Sources/Store/
git commit -m "feat: add ConversationStore with SQLite database setup"
```

---

### Task 3: ConversationStore - Topic CRUD

**Files:**
- Modify: `Aether/Sources/Store/ConversationStore.swift`

**Step 1: Add Topic CRUD methods**

Add to `ConversationStore`:

```swift
    // MARK: - Topic Operations

    /// Create a new topic
    func createTopic(title: String = "New Conversation") -> Topic? {
        let topic = Topic(title: title)
        do {
            try dbQueue?.write { db in
                try topic.insert(db)
            }
            print("[ConversationStore] Created topic: \(topic.id)")
            return topic
        } catch {
            print("[ConversationStore] Failed to create topic: \(error)")
            return nil
        }
    }

    /// Get all non-deleted topics, sorted by updatedAt DESC
    func getAllTopics() -> [Topic] {
        do {
            return try dbQueue?.read { db in
                try Topic
                    .filter(Column("isDeleted") == false)
                    .order(Column("updatedAt").desc)
                    .fetchAll(db)
            } ?? []
        } catch {
            print("[ConversationStore] Failed to fetch topics: \(error)")
            return []
        }
    }

    /// Get a topic by ID
    func getTopic(id: String) -> Topic? {
        do {
            return try dbQueue?.read { db in
                try Topic.fetchOne(db, key: id)
            }
        } catch {
            print("[ConversationStore] Failed to fetch topic: \(error)")
            return nil
        }
    }

    /// Update topic title
    func updateTopicTitle(id: String, title: String) {
        do {
            try dbQueue?.write { db in
                try db.execute(
                    sql: "UPDATE topics SET title = ?, updatedAt = ? WHERE id = ?",
                    arguments: [title, Date(), id]
                )
            }
            print("[ConversationStore] Updated topic title: \(id) -> \(title)")
        } catch {
            print("[ConversationStore] Failed to update topic title: \(error)")
        }
    }

    /// Soft delete a topic
    func deleteTopic(id: String) {
        do {
            try dbQueue?.write { db in
                try db.execute(
                    sql: "UPDATE topics SET isDeleted = 1 WHERE id = ?",
                    arguments: [id]
                )
            }
            print("[ConversationStore] Deleted topic: \(id)")
        } catch {
            print("[ConversationStore] Failed to delete topic: \(error)")
        }
    }

    /// Update topic's updatedAt timestamp
    func touchTopic(id: String) {
        do {
            try dbQueue?.write { db in
                try db.execute(
                    sql: "UPDATE topics SET updatedAt = ? WHERE id = ?",
                    arguments: [Date(), id]
                )
            }
        } catch {
            print("[ConversationStore] Failed to touch topic: \(error)")
        }
    }
```

**Step 2: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED

**Step 3: Commit**

```bash
git add Aether/Sources/Store/ConversationStore.swift
git commit -m "feat: add Topic CRUD operations to ConversationStore"
```

---

### Task 4: ConversationStore - Message CRUD

**Files:**
- Modify: `Aether/Sources/Store/ConversationStore.swift`

**Step 1: Add Message CRUD methods**

Add to `ConversationStore`:

```swift
    // MARK: - Message Operations

    /// Add a message to a topic
    func addMessage(topicId: String, role: MessageRole, content: String) -> ConversationMessage? {
        let message = ConversationMessage(topicId: topicId, role: role, content: content)
        do {
            try dbQueue?.write { db in
                try message.insert(db)
                // Update topic's updatedAt
                try db.execute(
                    sql: "UPDATE topics SET updatedAt = ? WHERE id = ?",
                    arguments: [Date(), topicId]
                )
            }
            print("[ConversationStore] Added message to topic \(topicId): \(role.rawValue)")
            return message
        } catch {
            print("[ConversationStore] Failed to add message: \(error)")
            return nil
        }
    }

    /// Get all messages for a topic, sorted by createdAt ASC
    func getMessages(topicId: String) -> [ConversationMessage] {
        do {
            return try dbQueue?.read { db in
                try ConversationMessage
                    .filter(Column("topicId") == topicId)
                    .order(Column("createdAt").asc)
                    .fetchAll(db)
            } ?? []
        } catch {
            print("[ConversationStore] Failed to fetch messages: \(error)")
            return []
        }
    }

    /// Get message count for a topic
    func getMessageCount(topicId: String) -> Int {
        do {
            return try dbQueue?.read { db in
                try ConversationMessage
                    .filter(Column("topicId") == topicId)
                    .fetchCount(db)
            } ?? 0
        } catch {
            print("[ConversationStore] Failed to count messages: \(error)")
            return 0
        }
    }

    /// Delete all messages for a topic
    func deleteMessages(topicId: String) {
        do {
            try dbQueue?.write { db in
                try db.execute(
                    sql: "DELETE FROM messages WHERE topicId = ?",
                    arguments: [topicId]
                )
            }
            print("[ConversationStore] Deleted messages for topic: \(topicId)")
        } catch {
            print("[ConversationStore] Failed to delete messages: \(error)")
        }
    }
```

**Step 2: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED

**Step 3: Commit**

```bash
git add Aether/Sources/Store/ConversationStore.swift
git commit -m "feat: add Message CRUD operations to ConversationStore"
```

---

### Task 5: Rust - Add generate_topic_title API

**Files:**
- Modify: `Aether/core/src/aether.udl`
- Modify: `Aether/core/src/lib.rs`
- Create: `Aether/core/src/title_generator.rs`

**Step 1: Create title_generator.rs**

```rust
//! Title generation for conversation topics.
//!
//! Uses a lightweight AI call to generate concise titles from conversation content.

use crate::providers::AiProvider;
use crate::error::AetherError;

const TITLE_PROMPT: &str = r#"Based on the following conversation, generate a very short title (maximum 15 Chinese characters or 30 English characters). Return ONLY the title, nothing else.

User: {user_input}
Assistant: {ai_response}

Title:"#;

/// Generate a concise title for a conversation topic.
///
/// # Arguments
/// * `provider` - The AI provider to use
/// * `user_input` - The user's first message (will be truncated to 200 chars)
/// * `ai_response` - The AI's first response (will be truncated to 200 chars)
///
/// # Returns
/// A short title string, or a default title on failure
pub async fn generate_title(
    provider: &dyn AiProvider,
    user_input: &str,
    ai_response: &str,
) -> String {
    let truncated_user: String = user_input.chars().take(200).collect();
    let truncated_response: String = ai_response.chars().take(200).collect();

    let prompt = TITLE_PROMPT
        .replace("{user_input}", &truncated_user)
        .replace("{ai_response}", &truncated_response);

    match provider.complete_simple(&prompt).await {
        Ok(title) => {
            let cleaned = title.trim().trim_matches('"').to_string();
            if cleaned.is_empty() {
                default_title(user_input)
            } else {
                cleaned
            }
        }
        Err(e) => {
            log::warn!("Failed to generate title: {}", e);
            default_title(user_input)
        }
    }
}

/// Generate a default title from user input
fn default_title(user_input: &str) -> String {
    let truncated: String = user_input.chars().take(20).collect();
    if truncated.len() < user_input.len() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_title_short() {
        let result = default_title("Hello");
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_default_title_long() {
        let result = default_title("This is a very long message that should be truncated");
        assert_eq!(result, "This is a very long ...");
    }
}
```

**Step 2: Add module to lib.rs**

In `Aether/core/src/lib.rs`, add:

```rust
mod title_generator;
```

**Step 3: Add UDL interface**

In `Aether/core/src/aether.udl`, add to interface:

```
    /// Generate a title for a conversation topic
    [Async]
    string generate_topic_title(string user_input, string ai_response);
```

**Step 4: Implement in AetherCore**

In `Aether/core/src/lib.rs`, add to `AetherCore` impl:

```rust
    /// Generate a title for a conversation topic
    pub async fn generate_topic_title(&self, user_input: String, ai_response: String) -> Result<String, AetherError> {
        let provider = self.get_provider()?;
        Ok(title_generator::generate_title(provider.as_ref(), &user_input, &ai_response).await)
    }
```

**Step 5: Run tests**

Run: `cd Aether/core && cargo test title_generator`
Expected: All tests pass

**Step 6: Build and generate bindings**

Run: `cd Aether/core && cargo build --release && cargo run --bin uniffi-bindgen generate src/aether.udl --language swift --out-dir ../Sources/Generated/`
Expected: Build succeeds, bindings generated

**Step 7: Commit**

```bash
git add Aether/core/src/title_generator.rs Aether/core/src/lib.rs Aether/core/src/aether.udl Aether/Sources/Generated/
git commit -m "feat(core): add generate_topic_title API for conversation topics"
```

---

## Phase 2: UI Components

### Task 6: Create ConversationDisplayWindow

**Files:**
- Create: `Aether/Sources/MultiTurn/ConversationDisplayWindow.swift`

**Step 1: Create the window class**

```swift
//
//  ConversationDisplayWindow.swift
//  Aether
//
//  Floating window for displaying multi-turn conversation history.
//  Positioned at top-right corner, draggable, with fixed width and adaptive height.
//

import Cocoa
import SwiftUI

// MARK: - ConversationDisplayWindow

/// Floating window for conversation display
final class ConversationDisplayWindow: NSWindow {

    // MARK: - Constants

    private enum Layout {
        static let width: CGFloat = 360
        static let minHeight: CGFloat = 200
        static let maxHeight: CGFloat = 600
        static let cornerRadius: CGFloat = 12
        static let screenPadding: CGFloat = 20
    }

    // MARK: - Properties

    /// View model for conversation state
    let viewModel = ConversationDisplayViewModel()

    /// Hosting view for SwiftUI content
    private var hostingView: NSHostingView<ConversationDisplayView>?

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Layout.width, height: Layout.minHeight),
            styleMask: [.borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupHostingView()
        positionAtTopRight()
    }

    // MARK: - Window Setup

    private func setupWindow() {
        // Appearance
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true

        // Behavior
        collectionBehavior = [.canJoinAllSpaces, .stationary]
        hidesOnDeactivate = false
        isMovableByWindowBackground = true

        // Content
        titlebarAppearsTransparent = true
        titleVisibility = .hidden
    }

    private func setupHostingView() {
        let displayView = ConversationDisplayView(viewModel: viewModel)
        hostingView = NSHostingView(rootView: displayView)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]
            contentView = hostingView
        }
    }

    // MARK: - Positioning

    private func positionAtTopRight() {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.visibleFrame
        let origin = NSPoint(
            x: screenFrame.maxX - Layout.width - Layout.screenPadding,
            y: screenFrame.maxY - frame.height - Layout.screenPadding
        )

        setFrameOrigin(origin)
    }

    // MARK: - Height Management

    /// Update window height based on content
    func updateHeight(for contentHeight: CGFloat) {
        let clampedHeight = min(max(contentHeight, Layout.minHeight), Layout.maxHeight)

        var newFrame = frame
        let heightDiff = clampedHeight - newFrame.height
        newFrame.size.height = clampedHeight
        newFrame.origin.y -= heightDiff  // Keep top edge fixed

        setFrame(newFrame, display: true, animate: true)
    }

    // MARK: - Show/Hide

    func show() {
        alphaValue = 0
        orderFrontRegardless()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        }
    }

    func hide() {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            self?.orderOut(nil)
        })
    }

    // MARK: - Focus Prevention

    override var canBecomeKey: Bool { true }  // Allow for copy interactions
    override var canBecomeMain: Bool { false }
}
```

**Step 2: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: Error about missing ConversationDisplayViewModel/View (expected, will create next)

**Step 3: Commit partial progress**

```bash
git add Aether/Sources/MultiTurn/
git commit -m "feat: add ConversationDisplayWindow framework (WIP)"
```

---

### Task 7: Create ConversationDisplayViewModel

**Files:**
- Create: `Aether/Sources/MultiTurn/ConversationDisplayViewModel.swift`

**Step 1: Create the view model**

```swift
//
//  ConversationDisplayViewModel.swift
//  Aether
//
//  View model for conversation display window.
//

import Foundation
import SwiftUI
import Combine

// MARK: - ConversationDisplayViewModel

/// View model for conversation display
final class ConversationDisplayViewModel: ObservableObject {

    // MARK: - Published Properties

    /// Current topic
    @Published var topic: Topic?

    /// Messages in current conversation
    @Published var messages: [ConversationMessage] = []

    /// Whether AI is currently responding
    @Published var isLoading: Bool = false

    /// Error message if any
    @Published var errorMessage: String?

    // MARK: - Computed Properties

    /// Whether there are any messages
    var hasMessages: Bool {
        !messages.isEmpty
    }

    /// Topic title for display
    var displayTitle: String {
        topic?.title ?? "New Conversation"
    }

    // MARK: - Actions

    /// Load messages for a topic
    func loadTopic(_ topic: Topic) {
        self.topic = topic
        self.messages = ConversationStore.shared.getMessages(topicId: topic.id)
        self.errorMessage = nil
    }

    /// Add a user message
    func addUserMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .user,
            content: content
        ) {
            messages.append(message)
        }
    }

    /// Add an assistant message
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

    /// Set loading state
    func setLoading(_ loading: Bool) {
        isLoading = loading
    }

    /// Set error message
    func setError(_ message: String?) {
        errorMessage = message
        isLoading = false
    }

    /// Clear conversation
    func clear() {
        topic = nil
        messages = []
        isLoading = false
        errorMessage = nil
    }

    // MARK: - Copy Actions

    /// Copy a single message to clipboard
    func copyMessage(_ message: ConversationMessage) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(message.content, forType: .string)
    }

    /// Copy all messages to clipboard
    func copyAllMessages() {
        let text = messages.map { msg in
            let prefix = msg.role == .user ? "User" : "Assistant"
            return "[\(prefix)]\n\(msg.content)"
        }.joined(separator: "\n\n")

        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
```

**Step 2: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: Error about missing ConversationDisplayView (expected, will create next)

**Step 3: Commit**

```bash
git add Aether/Sources/MultiTurn/ConversationDisplayViewModel.swift
git commit -m "feat: add ConversationDisplayViewModel"
```

---

### Task 8: Create ConversationDisplayView

**Files:**
- Create: `Aether/Sources/MultiTurn/ConversationDisplayView.swift`

**Step 1: Create the main view**

```swift
//
//  ConversationDisplayView.swift
//  Aether
//
//  SwiftUI view for displaying conversation history.
//

import SwiftUI

// MARK: - ConversationDisplayView

/// Main view for conversation display window
struct ConversationDisplayView: View {
    @ObservedObject var viewModel: ConversationDisplayViewModel

    var body: some View {
        VStack(spacing: 0) {
            // Title bar
            titleBar

            Divider()

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

            // Error message
            if let error = viewModel.errorMessage {
                errorBanner(error)
            }
        }
        .frame(width: 360)
        .background(VisualEffectView(material: .hudWindow, blendingMode: .behindWindow))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    // MARK: - Title Bar

    private var titleBar: some View {
        HStack {
            Circle()
                .fill(Color.purple)
                .frame(width: 8, height: 8)

            Text(viewModel.displayTitle)
                .font(.headline)
                .lineLimit(1)

            Spacer()

            Button(action: viewModel.copyAllMessages) {
                Image(systemName: "doc.on.doc")
                    .font(.system(size: 12))
            }
            .buttonStyle(.plain)
            .help("Copy all messages")
            .disabled(!viewModel.hasMessages)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    // MARK: - Messages List

    private var messagesList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 12) {
                    ForEach(viewModel.messages) { message in
                        MessageBubbleView(
                            message: message,
                            onCopy: { viewModel.copyMessage(message) }
                        )
                        .id(message.id)
                    }
                }
                .padding(12)
            }
            .onChange(of: viewModel.messages.count) { _ in
                // Scroll to bottom when new message added
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
                .foregroundColor(.secondary)

            Text("Start a conversation")
                .font(.subheadline)
                .foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding()
    }

    // MARK: - Loading Indicator

    private var loadingIndicator: some View {
        HStack(spacing: 4) {
            ForEach(0..<3) { i in
                Circle()
                    .fill(Color.purple.opacity(0.6))
                    .frame(width: 6, height: 6)
            }
        }
        .padding(.vertical, 8)
    }

    // MARK: - Error Banner

    private func errorBanner(_ message: String) -> some View {
        HStack {
            Image(systemName: "exclamationmark.triangle")
            Text(message)
                .font(.caption)
        }
        .foregroundColor(.red)
        .padding(8)
        .background(Color.red.opacity(0.1))
        .cornerRadius(8)
        .padding(12)
    }
}

// MARK: - MessageBubbleView

/// Individual message bubble
struct MessageBubbleView: View {
    let message: ConversationMessage
    let onCopy: () -> Void

    @State private var isHovering = false

    private var isUser: Bool {
        message.role == .user
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            if isUser { Spacer(minLength: 40) }

            VStack(alignment: isUser ? .trailing : .leading, spacing: 4) {
                // Message content
                Text(message.content)
                    .font(.system(size: 13))
                    .textSelection(.enabled)
                    .padding(10)
                    .background(bubbleBackground)
                    .clipShape(RoundedRectangle(cornerRadius: 12))

                // Copy button (on hover)
                if isHovering {
                    Button(action: onCopy) {
                        HStack(spacing: 2) {
                            Image(systemName: "doc.on.doc")
                            Text("Copy")
                        }
                        .font(.caption2)
                        .foregroundColor(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }

            if !isUser { Spacer(minLength: 40) }
        }
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
    }

    private var bubbleBackground: Color {
        isUser ? Color.purple.opacity(0.2) : Color.gray.opacity(0.15)
    }
}

// MARK: - VisualEffectView

/// NSVisualEffectView wrapper for SwiftUI
struct VisualEffectView: NSViewRepresentable {
    let material: NSVisualEffectView.Material
    let blendingMode: NSVisualEffectView.BlendingMode

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = blendingMode
        view.state = .active
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = material
        nsView.blendingMode = blendingMode
    }
}
```

**Step 2: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED

**Step 3: Commit**

```bash
git add Aether/Sources/MultiTurn/ConversationDisplayView.swift
git commit -m "feat: add ConversationDisplayView with message bubbles"
```

---

### Task 9: Create MultiTurnInputWindow

**Files:**
- Create: `Aether/Sources/MultiTurn/MultiTurnInputWindow.swift`
- Create: `Aether/Sources/MultiTurn/MultiTurnInputView.swift`

**Step 1: Create MultiTurnInputWindow.swift**

```swift
//
//  MultiTurnInputWindow.swift
//  Aether
//
//  Input window for multi-turn conversation mode.
//  Centered on screen, supports text input and // command for topic list.
//

import Cocoa
import SwiftUI

// MARK: - MultiTurnInputWindow

/// Input window for multi-turn conversations
final class MultiTurnInputWindow: NSWindow {

    // MARK: - Properties

    /// View model for input state
    let viewModel = MultiTurnInputViewModel()

    /// Hosting view for SwiftUI content
    private var hostingView: NSHostingView<MultiTurnInputView>?

    /// Callbacks
    var onSubmit: ((String) -> Void)?
    var onCancel: (() -> Void)?
    var onTopicSelected: ((Topic) -> Void)?

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 600, height: 60),
            styleMask: [.borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupHostingView()
        setupCallbacks()
    }

    // MARK: - Window Setup

    private func setupWindow() {
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true

        collectionBehavior = [.canJoinAllSpaces, .stationary]
        hidesOnDeactivate = false

        titlebarAppearsTransparent = true
        titleVisibility = .hidden
    }

    private func setupHostingView() {
        let inputView = MultiTurnInputView(viewModel: viewModel)
        hostingView = NSHostingView(rootView: inputView)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]
            contentView = hostingView
        }
    }

    private func setupCallbacks() {
        viewModel.onSubmit = { [weak self] text in
            self?.onSubmit?(text)
        }
        viewModel.onCancel = { [weak self] in
            self?.onCancel?()
        }
        viewModel.onTopicSelected = { [weak self] topic in
            self?.onTopicSelected?(topic)
        }
    }

    // MARK: - Show/Hide

    func showCentered() {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.frame
        let origin = NSPoint(
            x: screenFrame.midX - frame.width / 2,
            y: screenFrame.midY + 100  // Slightly above center
        )

        setFrameOrigin(origin)
        alphaValue = 0
        orderFrontRegardless()
        makeKey()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }

        // Focus the text field
        viewModel.focusInput()
    }

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

**Step 2: Create MultiTurnInputView.swift**

```swift
//
//  MultiTurnInputView.swift
//  Aether
//
//  SwiftUI view for multi-turn input window.
//

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
        .background(VisualEffectView(material: .hudWindow, blendingMode: .behindWindow))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .onChange(of: viewModel.shouldFocusInput) { shouldFocus in
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
                .onChange(of: viewModel.inputText) { newValue in
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
```

**Step 3: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED

**Step 4: Commit**

```bash
git add Aether/Sources/MultiTurn/MultiTurnInputWindow.swift Aether/Sources/MultiTurn/MultiTurnInputView.swift
git commit -m "feat: add MultiTurnInputWindow with topic list support"
```

---

## Phase 3: Coordinator

### Task 10: Create MultiTurnCoordinator - Framework

**Files:**
- Create: `Aether/Sources/MultiTurn/MultiTurnCoordinator.swift`

**Step 1: Create the coordinator framework**

```swift
//
//  MultiTurnCoordinator.swift
//  Aether
//
//  Coordinator for multi-turn conversation mode.
//  Manages input window, display window, persistence, and AI interaction.
//

import AppKit
import SwiftUI

// MARK: - MultiTurnCoordinator

/// Coordinator for multi-turn conversation mode
final class MultiTurnCoordinator {

    // MARK: - Singleton

    static let shared = MultiTurnCoordinator()

    // MARK: - Dependencies

    private weak var core: AetherCore?

    // MARK: - Windows

    private lazy var inputWindow: MultiTurnInputWindow = {
        let window = MultiTurnInputWindow()
        window.onSubmit = { [weak self] text in
            self?.handleInput(text)
        }
        window.onCancel = { [weak self] in
            self?.exit()
        }
        window.onTopicSelected = { [weak self] topic in
            self?.loadTopic(topic)
        }
        return window
    }()

    private lazy var displayWindow: ConversationDisplayWindow = {
        ConversationDisplayWindow()
    }()

    // MARK: - State

    private var currentTopic: Topic?
    private var isActive: Bool = false

    // MARK: - Initialization

    private init() {}

    // MARK: - Configuration

    /// Configure with dependencies
    func configure(core: AetherCore) {
        self.core = core
    }

    // MARK: - Hotkey Handling

    /// Handle hotkey press (Cmd+Opt+/)
    func handleHotkey() {
        print("[MultiTurnCoordinator] Hotkey pressed")

        if isActive {
            // Toggle off if already active
            exit()
        } else {
            // Start new session
            start()
        }
    }

    // MARK: - Session Management

    /// Start a new multi-turn session
    private func start() {
        print("[MultiTurnCoordinator] Starting new session")
        isActive = true

        // Create new topic
        currentTopic = ConversationStore.shared.createTopic()

        // Show windows
        displayWindow.viewModel.clear()
        if let topic = currentTopic {
            displayWindow.viewModel.loadTopic(topic)
        }
        displayWindow.show()

        inputWindow.updateTurnCount(0)
        inputWindow.showCentered()
    }

    /// Exit multi-turn mode
    func exit() {
        print("[MultiTurnCoordinator] Exiting")
        isActive = false

        inputWindow.hide()
        displayWindow.hide()
        currentTopic = nil
    }

    // MARK: - Topic Management

    /// Load an existing topic
    private func loadTopic(_ topic: Topic) {
        print("[MultiTurnCoordinator] Loading topic: \(topic.title)")
        currentTopic = topic

        displayWindow.viewModel.loadTopic(topic)

        let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
        inputWindow.updateTurnCount(messageCount / 2)  // User + Assistant = 1 turn
    }

    // MARK: - Input Handling

    /// Handle user input
    private func handleInput(_ text: String) {
        guard let topic = currentTopic, let core = core else {
            print("[MultiTurnCoordinator] No active topic or core")
            return
        }

        print("[MultiTurnCoordinator] Processing input: \(text.prefix(50))...")

        // Add user message
        displayWindow.viewModel.addUserMessage(text)
        displayWindow.viewModel.setLoading(true)

        // Get conversation history for context
        let messages = displayWindow.viewModel.messages
        let conversationHistory = messages.map { msg in
            ConversationTurn(
                turnId: 0,
                userInput: msg.role == .user ? msg.content : "",
                aiResponse: msg.role == .assistant ? msg.content : ""
            )
        }

        // Process in background
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.processWithAI(text: text, topic: topic, history: conversationHistory)
        }
    }

    /// Process input with AI
    private func processWithAI(text: String, topic: Topic, history: [ConversationTurn]) {
        guard let core = core else { return }

        do {
            // Create context
            let context = CapturedContext(
                appBundleId: "com.aether.multi-turn",
                windowTitle: nil,
                attachments: nil
            )

            // Call AI
            let response = try core.processInput(userInput: text, context: context)

            DispatchQueue.main.async { [weak self] in
                self?.handleAIResponse(response, topic: topic, userInput: text)
            }

        } catch {
            DispatchQueue.main.async { [weak self] in
                self?.displayWindow.viewModel.setError(error.localizedDescription)
            }
        }
    }

    /// Handle AI response
    private func handleAIResponse(_ response: String, topic: Topic, userInput: String) {
        // Add assistant message
        displayWindow.viewModel.addAssistantMessage(response)

        // Update turn count
        let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
        inputWindow.updateTurnCount(messageCount / 2)

        // Generate title if this is the first turn
        if messageCount == 2 {  // First user + first assistant
            generateTitle(topic: topic, userInput: userInput, aiResponse: response)
        }
    }

    /// Generate title for topic
    private func generateTitle(topic: Topic, userInput: String, aiResponse: String) {
        guard let core = core else { return }

        DispatchQueue.global(qos: .background).async {
            do {
                let title = try core.generateTopicTitle(userInput: userInput, aiResponse: aiResponse)

                ConversationStore.shared.updateTopicTitle(id: topic.id, title: title)

                DispatchQueue.main.async { [weak self] in
                    self?.displayWindow.viewModel.topic?.title = title
                }
            } catch {
                print("[MultiTurnCoordinator] Failed to generate title: \(error)")
            }
        }
    }
}
```

**Step 2: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED (or errors about ConversationTurn which we'll fix)

**Step 3: Commit**

```bash
git add Aether/Sources/MultiTurn/MultiTurnCoordinator.swift
git commit -m "feat: add MultiTurnCoordinator framework"
```

---

### Task 11: Integrate MultiTurnCoordinator into AppDelegate

**Files:**
- Modify: `Aether/Sources/AppDelegate.swift`

**Step 1: Add MultiTurnCoordinator initialization**

In `AppDelegate.swift`, add to initialization section:

```swift
// In setupCore() or similar initialization method, add:
MultiTurnCoordinator.shared.configure(core: core)
```

**Step 2: Update hotkey handling**

Find the existing Cmd+Opt+/ hotkey handling and update to call MultiTurnCoordinator:

```swift
// Replace existing unified hotkey handling with:
MultiTurnCoordinator.shared.handleHotkey()
```

**Step 3: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED

**Step 4: Commit**

```bash
git add Aether/Sources/AppDelegate.swift
git commit -m "feat: integrate MultiTurnCoordinator into AppDelegate"
```

---

## Phase 4: Cleanup

### Task 12: Simplify UnifiedInputCoordinator

**Files:**
- Modify: `Aether/Sources/Coordinator/UnifiedInputCoordinator.swift`

**Step 1: Remove multi-turn conversation logic**

Remove or comment out:
- `currentSessionId` property
- `currentTurnCount` property
- `handleConversationInput()` method
- `onConversationTurnCompleted` notification observer
- Any conversation-related code paths

Keep:
- Single-turn command processing (`/command` handling)
- CLI output mode
- Focus detection (for single-turn use cases)

**Step 2: Verify compilation**

Run: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Debug build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED

**Step 3: Commit**

```bash
git add Aether/Sources/Coordinator/UnifiedInputCoordinator.swift
git commit -m "refactor: simplify UnifiedInputCoordinator, remove multi-turn logic"
```

---

### Task 13: Mark Deprecated Components

**Files:**
- Modify: `Aether/Sources/Coordinator/ConversationCoordinator.swift`
- Modify: `Aether/Sources/Utils/ConversationManager.swift`

**Step 1: Add deprecation markers**

Add to top of each file:

```swift
// DEPRECATED: This file is deprecated and will be removed.
// Multi-turn conversation logic has been moved to MultiTurnCoordinator.
// Keeping for backward compatibility during transition.
```

**Step 2: Commit**

```bash
git add Aether/Sources/Coordinator/ConversationCoordinator.swift Aether/Sources/Utils/ConversationManager.swift
git commit -m "refactor: mark ConversationCoordinator and ConversationManager as deprecated"
```

---

## Phase 5: Testing & Polish

### Task 14: Manual Testing Checklist

Create a manual testing checklist:

**New Multi-Turn Mode:**
- [ ] Cmd+Opt+/ opens input window (centered) and display window (top-right)
- [ ] Typing and pressing Enter sends message
- [ ] User message appears in display window
- [ ] AI response appears in display window
- [ ] Title is generated after first exchange
- [ ] ESC closes both windows
- [ ] `//` shows topic list
- [ ] Selecting topic loads conversation history
- [ ] Can continue existing conversation
- [ ] Copy single message works
- [ ] Copy all messages works
- [ ] Display window is draggable
- [ ] Display window height adapts to content

**Single-Turn Mode (Regression):**
- [ ] Double-tap Shift still works
- [ ] Selection-based flow unchanged
- [ ] Output to target app works

**Persistence:**
- [ ] Conversations survive app restart
- [ ] Topics appear in `//` list after restart
- [ ] Messages are preserved

---

### Task 15: Final Commit and PR Preparation

**Step 1: Run full test suite**

Run: `cd Aether/core && cargo test`
Expected: All tests pass

**Step 2: Generate final bindings**

Run: `cd Aether/core && cargo build --release && cargo run --bin uniffi-bindgen generate src/aether.udl --language swift --out-dir ../Sources/Generated/ && cp target/release/libaethecore.dylib ../Frameworks/`

**Step 3: Final build verification**

Run: `xcodegen generate && xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build 2>&1 | grep -E "(BUILD|error:)"`
Expected: BUILD SUCCEEDED

**Step 4: Create summary commit**

```bash
git add -A
git commit -m "feat: complete multi-turn conversation mode separation

- Add ConversationStore for SQLite persistence
- Add ConversationDisplayWindow (floating, top-right)
- Add MultiTurnInputWindow with // command support
- Add MultiTurnCoordinator for independent multi-turn flow
- Add generate_topic_title API in Rust core
- Simplify UnifiedInputCoordinator (single-turn only)
- Deprecate ConversationCoordinator/ConversationManager

Closes: multi-turn-separation design doc"
```

---

## Summary

| Phase | Tasks | Estimated Steps |
|-------|-------|-----------------|
| Phase 1: Data Layer | 5 tasks | ~25 steps |
| Phase 2: UI Components | 4 tasks | ~20 steps |
| Phase 3: Coordinator | 2 tasks | ~10 steps |
| Phase 4: Cleanup | 2 tasks | ~8 steps |
| Phase 5: Testing | 2 tasks | ~10 steps |
| **Total** | **15 tasks** | **~73 steps** |
