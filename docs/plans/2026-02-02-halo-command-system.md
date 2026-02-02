# Halo Command System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix broken `MultiTurnCoordinator` references and implement `/` (command list) and `//` (history list) commands in the Halo-Only architecture.

**Architecture:** Create a lightweight `HaloInputCoordinator` to replace the deleted `MultiTurnCoordinator`. This coordinator handles hotkey activation, input processing, and command prefix detection (`/` and `//`). Add a new `commandList` state to `HaloState` and create `HaloCommandListView` for the `/` command UI.

**Tech Stack:** Swift, SwiftUI, AppKit (NSEvent monitoring)

---

## Overview

The previous `MultiTurnCoordinator` and related `MultiTurn/` directory were deleted, leaving broken references in:
- `AppDelegate.swift` (lines 300-302, 600-601)
- `HotkeyService.swift` (line 89)
- `DependencyContainer.swift` (line 304)
- `GatewayMultiTurnAdapter.swift` (line 83)

This plan:
1. Creates `HaloInputCoordinator` as a minimal replacement
2. Adds `commandList` state to `HaloState` (8th state)
3. Creates `HaloCommandListView` for `/` command
4. Implements input prefix detection for `/` and `//`
5. Cleans up all broken references

---

### Task 1: Create HaloInputCoordinator

**Files:**
- Create: `platforms/macos/Aether/Sources/Coordinators/HaloInputCoordinator.swift`

**Step 1: Create the coordinator file**

```swift
//
//  HaloInputCoordinator.swift
//  Aether
//
//  Lightweight coordinator for Halo input handling.
//  Replaces MultiTurnCoordinator with minimal command-focused logic.
//

import AppKit
import SwiftUI

// MARK: - HaloInputCoordinator

/// Coordinator for handling hotkey-triggered input in Halo-Only mode
///
/// Responsibilities:
/// - Handle hotkey activation (Option+Space)
/// - Detect command prefixes (/, //)
/// - Route to appropriate Halo state
/// - Process normal AI input
@MainActor
final class HaloInputCoordinator {

    // MARK: - Singleton

    static let shared = HaloInputCoordinator()

    // MARK: - Dependencies

    private weak var haloWindow: HaloWindow?
    private weak var core: AetherCore?

    // MARK: - State

    /// Whether we're currently waiting for input
    private var isAwaitingInput: Bool = false

    /// Clipboard content at time of hotkey press
    private var capturedClipboard: String?

    // MARK: - Initialization

    private init() {}

    // MARK: - Configuration

    /// Configure coordinator with dependencies
    func configure(haloWindow: HaloWindow?, core: AetherCore?) {
        self.haloWindow = haloWindow
        self.core = core
        print("[HaloInputCoordinator] Configured")
    }

    // MARK: - Hotkey Handling

    /// Handle hotkey press (called from HotkeyService)
    func handleHotkey() {
        guard let haloWindow = haloWindow else {
            print("[HaloInputCoordinator] Error: HaloWindow not configured")
            return
        }

        // If already visible and interactive, dismiss
        if haloWindow.isVisible && haloWindow.viewModel.state.isInteractive {
            haloWindow.hide()
            isAwaitingInput = false
            return
        }

        // Capture current clipboard content
        capturedClipboard = NSPasteboard.general.string(forType: .string)

        // Show listening state
        haloWindow.updateState(.listening)
        haloWindow.showAtCurrentPosition()
        isAwaitingInput = true

        // Process clipboard content after brief delay (allow UI to render)
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 100_000_000) // 100ms
            self?.processInput()
        }
    }

    // MARK: - Input Processing

    /// Process captured clipboard input
    private func processInput() {
        guard isAwaitingInput, let input = capturedClipboard else {
            return
        }

        isAwaitingInput = false

        // Trim whitespace
        let trimmedInput = input.trimmingCharacters(in: .whitespacesAndNewlines)

        // Check for command prefixes
        if trimmedInput.hasPrefix("//") {
            // History command
            showHistoryList()
        } else if trimmedInput.hasPrefix("/") {
            // Command list
            let query = String(trimmedInput.dropFirst())
            showCommandList(query: query)
        } else {
            // Normal AI input
            processAIInput(trimmedInput)
        }
    }

    // MARK: - Command Handlers

    /// Show history list (// command)
    private func showHistoryList() {
        guard let haloWindow = haloWindow else { return }

        // TODO: Fetch actual history from Gateway/Core
        let topics: [HistoryTopic] = [] // Will be populated by Gateway RPC

        let context = HistoryListContext(topics: topics)
        haloWindow.updateState(.historyList(context))

        // Set callback for topic selection
        haloWindow.viewModel.callbacks.onHistorySelect = { [weak self] topic in
            self?.handleHistorySelect(topic)
        }
        haloWindow.viewModel.callbacks.onDismiss = { [weak haloWindow] in
            haloWindow?.hide()
        }

        haloWindow.showCentered()
    }

    /// Show command list (/ command)
    private func showCommandList(query: String) {
        guard let haloWindow = haloWindow else { return }

        // Fetch skills from Gateway
        Task { @MainActor [weak self] in
            guard let self = self else { return }

            var commands: [CommandItem] = []

            // Try Gateway first
            if GatewayManager.shared.isReady {
                do {
                    let skills = try await GatewayManager.shared.client.skillsList()
                    commands = skills.map { skill in
                        CommandItem(
                            id: skill.id,
                            name: skill.name,
                            description: skill.description ?? "",
                            icon: "command.circle.fill"
                        )
                    }
                } catch {
                    print("[HaloInputCoordinator] Failed to fetch skills: \(error)")
                }
            }

            // Show command list
            let context = CommandListContext(commands: commands, searchQuery: query)
            haloWindow.updateState(.commandList(context))

            // Set callbacks
            haloWindow.viewModel.callbacks.onCommandSelect = { [weak self] command in
                self?.handleCommandSelect(command)
            }
            haloWindow.viewModel.callbacks.onDismiss = { [weak haloWindow] in
                haloWindow?.hide()
            }

            haloWindow.showCentered()
        }
    }

    /// Process normal AI input
    private func processAIInput(_ input: String) {
        guard let haloWindow = haloWindow, let core = core else { return }

        guard !input.isEmpty else {
            haloWindow.hide()
            return
        }

        // Show streaming state
        let runId = UUID().uuidString
        let context = StreamingContext(runId: runId, phase: .thinking)
        haloWindow.updateState(.streaming(context))

        // Process via Gateway or FFI
        if GatewayManager.shared.isReady {
            Task {
                do {
                    let (_, stream) = try await GatewayManager.shared.client.agentRun(
                        input: input,
                        sessionKey: "halo-\(UUID().uuidString)"
                    )
                    // Stream events will be handled by EventHandler
                    for try await _ in stream {
                        // Events handled via EventHandler
                    }
                } catch {
                    print("[HaloInputCoordinator] Gateway run failed: \(error)")
                    haloWindow.showError(
                        ErrorContext(type: .unknown, message: error.localizedDescription),
                        onDismiss: { haloWindow.hide() }
                    )
                }
            }
        } else {
            // FFI fallback
            do {
                let options = ProcessOptions(
                    appContext: nil,
                    windowTitle: nil,
                    topicId: nil,
                    stream: true,
                    attachments: nil,
                    preferredLanguage: LocalizationManager.shared.currentLanguage
                )
                try core.process(input: input, options: options)
            } catch {
                print("[HaloInputCoordinator] FFI process failed: \(error)")
                haloWindow.showError(
                    ErrorContext(type: .unknown, message: error.localizedDescription),
                    onDismiss: { haloWindow.hide() }
                )
            }
        }
    }

    // MARK: - Selection Handlers

    private func handleHistorySelect(_ topic: HistoryTopic) {
        print("[HaloInputCoordinator] Selected history topic: \(topic.title)")
        // TODO: Load conversation and show in Halo streaming mode
        haloWindow?.hide()
    }

    private func handleCommandSelect(_ command: CommandItem) {
        print("[HaloInputCoordinator] Selected command: \(command.name)")

        // Execute the skill/command
        let input = "/\(command.name)"
        processAIInput(input)
    }

    // MARK: - Public API

    /// Show or hide based on current state
    func showOrBringToFront() {
        handleHotkey()
    }

    /// Cancel current operation
    func cancel() {
        isAwaitingInput = false
        capturedClipboard = nil
        haloWindow?.hide()
    }
}
```

**Step 2: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/Coordinators/HaloInputCoordinator.swift`
Expected: Syntax valid

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Coordinators/HaloInputCoordinator.swift
git commit -m "$(cat <<'EOF'
feat(halo): add HaloInputCoordinator for command handling

Replaces deleted MultiTurnCoordinator with minimal command-focused logic.
Handles hotkey activation, command prefix detection (/ and //), and
routes to appropriate Halo states.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Add commandList State to HaloState

**Files:**
- Modify: `platforms/macos/Aether/Sources/HaloState.swift`
- Modify: `platforms/macos/Aether/Sources/Components/HaloStreamingTypes.swift`

**Step 1: Add CommandListContext to HaloStreamingTypes.swift**

Add after `HistoryListContext` (around line 354):

```swift
// MARK: - Command List Types

/// A command/skill item in the command list
struct CommandItem: Identifiable, Equatable {
    /// Unique identifier for this command
    let id: String
    /// Display name of the command
    let name: String
    /// Description of what the command does
    let description: String
    /// SF Symbol icon name
    let icon: String

    init(id: String, name: String, description: String, icon: String = "command.circle.fill") {
        self.id = id
        self.name = name
        self.description = description
        self.icon = icon
    }
}

/// Context for command list (/ command)
struct CommandListContext: Equatable {
    /// All available commands
    let commands: [CommandItem]
    /// Current search query
    var searchQuery: String
    /// Currently selected command index
    var selectedIndex: Int?

    /// Commands filtered by search query
    var filteredCommands: [CommandItem] {
        if searchQuery.isEmpty {
            return commands
        }
        let query = searchQuery.lowercased()
        return commands.filter { command in
            command.name.lowercased().contains(query) ||
            command.description.lowercased().contains(query)
        }
    }

    init(commands: [CommandItem], searchQuery: String = "", selectedIndex: Int? = nil) {
        self.commands = commands
        self.searchQuery = searchQuery
        self.selectedIndex = selectedIndex
    }
}
```

**Step 2: Add commandList case to HaloState enum**

In `HaloState.swift`, add new case after `historyList` (line 35):

```swift
    /// Command list view (/ command)
    case commandList(CommandListContext)
```

**Step 3: Add isCommandList helper**

Add after `isHistoryList` computed property (around line 81):

```swift
    /// Check if state is command list
    var isCommandList: Bool {
        if case .commandList = self { return true }
        return false
    }
```

**Step 4: Update isInteractive to include commandList**

Modify the `isInteractive` computed property (around line 84):

```swift
    /// Check if state requires user interaction
    var isInteractive: Bool {
        switch self {
        case .confirmation, .error, .historyList, .commandList:
            return true
        default:
            return false
        }
    }
```

**Step 5: Add windowSize for commandList**

In the `windowSize` computed property (around line 119), add:

```swift
        case .commandList:
            return NSSize(width: 380, height: 420)
```

**Step 6: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/HaloState.swift`
Expected: Syntax valid

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/Components/HaloStreamingTypes.swift`
Expected: Syntax valid

**Step 7: Commit**

```bash
git add platforms/macos/Aether/Sources/HaloState.swift platforms/macos/Aether/Sources/Components/HaloStreamingTypes.swift
git commit -m "$(cat <<'EOF'
feat(halo): add commandList state for / command

Adds CommandItem, CommandListContext types and commandList case to
HaloState enum. Updates isInteractive and windowSize helpers.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Create HaloCommandListView

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/HaloCommandListView.swift`

**Step 1: Create the view file**

```swift
//
//  HaloCommandListView.swift
//  Aether
//
//  Command list panel view for the / command.
//  Displays available skills/commands with search filtering.
//

import SwiftUI

/// Command list panel for navigating available commands
struct HaloCommandListView: View {
    @Binding var context: CommandListContext
    let onSelect: (CommandItem) -> Void
    let onDismiss: () -> Void

    @State private var isAppearing = false

    var body: some View {
        VStack(spacing: 0) {
            headerView
            searchFieldView
            commandListView
        }
        .frame(width: 360, height: 380)
        .background(.ultraThinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .scaleEffect(isAppearing ? 1.0 : 0.95)
        .opacity(isAppearing ? 1.0 : 0.0)
        .onAppear {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                isAppearing = true
            }
        }
    }

    // MARK: - Header

    private var headerView: some View {
        HStack(spacing: 10) {
            Image(systemName: "command.circle.fill")
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(.blue)

            Text(L("commands.title"))
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)

            Spacer()

            Button(action: dismissWithAnimation) {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 18))
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    // MARK: - Search Field

    private var searchFieldView: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 12))
                .foregroundColor(.secondary)

            TextField(L("commands.search"), text: $context.searchQuery)
                .textFieldStyle(.plain)
                .font(.system(size: 13))

            if !context.searchQuery.isEmpty {
                Button(action: { context.searchQuery = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(Color.primary.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
    }

    // MARK: - Command List

    private var commandListView: some View {
        Group {
            if context.filteredCommands.isEmpty {
                emptyStateView
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(context.filteredCommands.enumerated()), id: \.element.id) { index, command in
                            CommandRow(
                                command: command,
                                isSelected: context.selectedIndex == index
                            )
                            .onTapGesture {
                                onSelect(command)
                            }
                        }
                    }
                }
            }
        }
    }

    private var emptyStateView: some View {
        VStack(spacing: 12) {
            Spacer()
            Image(systemName: "command.circle")
                .font(.system(size: 32))
                .foregroundColor(.secondary.opacity(0.5))
            Text(L("commands.empty"))
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: - Actions

    private func dismissWithAnimation() {
        withAnimation(.easeOut(duration: 0.2)) {
            isAppearing = false
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            onDismiss()
        }
    }
}

// MARK: - Command Row

private struct CommandRow: View {
    let command: CommandItem
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            // Icon
            Image(systemName: command.icon)
                .font(.system(size: 16))
                .foregroundColor(.blue)
                .frame(width: 24)

            // Name and description
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text("/")
                        .font(.system(size: 13, weight: .medium, design: .monospaced))
                        .foregroundColor(.secondary)
                    Text(command.name)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(.primary)
                }

                if !command.description.isEmpty {
                    Text(command.description)
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                }
            }

            Spacer()

            // Enter hint
            Text("↵")
                .font(.system(size: 11, weight: .medium))
                .foregroundColor(.secondary.opacity(0.5))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(
            isSelected ? Color.accentColor.opacity(0.1) : Color.clear
        )
        .contentShape(Rectangle())
    }
}

// MARK: - Preview

#if DEBUG
struct HaloCommandListView_Previews: PreviewProvider {
    static var previews: some View {
        let sampleCommands = [
            CommandItem(id: "1", name: "translate", description: "Translate text to another language", icon: "globe"),
            CommandItem(id: "2", name: "summarize", description: "Summarize long text", icon: "doc.text"),
            CommandItem(id: "3", name: "code", description: "Generate or explain code", icon: "chevron.left.forwardslash.chevron.right"),
            CommandItem(id: "4", name: "search", description: "Search the web", icon: "magnifyingglass"),
            CommandItem(id: "5", name: "remember", description: "Save information to memory", icon: "brain.head.profile"),
        ]

        Group {
            // Normal state
            HaloCommandListView(
                context: .constant(CommandListContext(commands: sampleCommands)),
                onSelect: { print("Selected: \($0.name)") },
                onDismiss: { print("Dismissed") }
            )
            .previewDisplayName("With Commands")

            // Empty state
            HaloCommandListView(
                context: .constant(CommandListContext(commands: [])),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("Empty State")

            // With search
            HaloCommandListView(
                context: .constant(CommandListContext(commands: sampleCommands, searchQuery: "trans")),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("With Search")

            // With selection
            HaloCommandListView(
                context: .constant(CommandListContext(commands: sampleCommands, selectedIndex: 2)),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("With Selection")
        }
        .padding(40)
        .background(Color.gray.opacity(0.2))
    }
}
#endif
```

**Step 2: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/Components/HaloCommandListView.swift`
Expected: Syntax valid

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/HaloCommandListView.swift
git commit -m "$(cat <<'EOF'
feat(halo): add HaloCommandListView for / command UI

Displays available skills/commands with search filtering.
Includes CommandRow component and preview support.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Update HaloViewV2 for commandList State

**Files:**
- Modify: `platforms/macos/Aether/Sources/Components/HaloViewV2.swift`

**Step 1: Add onCommandSelect callback to HaloCallbacksV2**

In `HaloCallbacksV2` class (around line 151), add:

```swift
    /// Called when user selects a command from command list
    var onCommandSelect: ((CommandItem) -> Void)?
```

**Step 2: Update reset() method**

In the `reset()` method, add:

```swift
        onCommandSelect = nil
```

**Step 3: Add commandList case to HaloViewV2 body**

In `HaloViewV2` body switch statement, add after `historyList` case (around line 92):

```swift
            case .commandList(let context):
                HaloCommandListView(
                    context: Binding(
                        get: {
                            if case .commandList(let ctx) = viewModel.state {
                                return ctx
                            }
                            return context
                        },
                        set: { newContext in
                            viewModel.updateCommandContext(newContext)
                        }
                    ),
                    onSelect: { command in
                        viewModel.callbacks.onCommandSelect?(command)
                    },
                    onDismiss: {
                        viewModel.callbacks.onDismiss?()
                    }
                )
```

**Step 4: Add updateCommandContext method to HaloViewModelV2**

In `HaloViewModelV2` class, add after `updateHistoryContext`:

```swift
    /// Update command context (for search query and selection changes)
    func updateCommandContext(_ context: CommandListContext) {
        if case .commandList = state {
            state = .commandList(context)
        }
    }
```

**Step 5: Update stateIdentifier**

In the `stateIdentifier` computed property, add:

```swift
        case .commandList:
            return "commandList"
```

**Step 6: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/Components/HaloViewV2.swift`
Expected: Syntax valid

**Step 7: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/HaloViewV2.swift
git commit -m "$(cat <<'EOF'
feat(halo): integrate HaloCommandListView in HaloViewV2

Adds commandList case handling, onCommandSelect callback, and
updateCommandContext method for command list state management.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Fix HotkeyService Reference

**Files:**
- Modify: `platforms/macos/Aether/Sources/Services/HotkeyService.swift`

**Step 1: Replace MultiTurnCoordinator with HaloInputCoordinator**

Change line 89 from:

```swift
                    MultiTurnCoordinator.shared.handleHotkey()
```

to:

```swift
                    HaloInputCoordinator.shared.handleHotkey()
```

**Step 2: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/Services/HotkeyService.swift`
Expected: Syntax valid

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Services/HotkeyService.swift
git commit -m "$(cat <<'EOF'
fix(hotkey): replace MultiTurnCoordinator with HaloInputCoordinator

Updates hotkey handler to use new HaloInputCoordinator.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Fix AppDelegate References

**Files:**
- Modify: `platforms/macos/Aether/Sources/AppDelegate.swift`

**Step 1: Replace showConversation implementation**

Change the `showConversation()` method (around line 300-302) from:

```swift
    @objc private func showConversation() {
        MultiTurnCoordinator.shared.showOrBringToFront()
    }
```

to:

```swift
    @objc private func showConversation() {
        HaloInputCoordinator.shared.showOrBringToFront()
    }
```

**Step 2: Replace configure call**

Change line 600-601 from:

```swift
            // Configure MultiTurnCoordinator with dependencies
            MultiTurnCoordinator.shared.configure(core: core)
```

to:

```swift
            // Configure HaloInputCoordinator with dependencies
            HaloInputCoordinator.shared.configure(haloWindow: haloWindow, core: core)
```

**Step 3: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/AppDelegate.swift`
Expected: Syntax valid

**Step 4: Commit**

```bash
git add platforms/macos/Aether/Sources/AppDelegate.swift
git commit -m "$(cat <<'EOF'
fix(app): replace MultiTurnCoordinator with HaloInputCoordinator

Updates showConversation and configure calls to use new coordinator.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Fix DependencyContainer Reference

**Files:**
- Modify: `platforms/macos/Aether/Sources/DI/DependencyContainer.swift`

**Step 1: Remove MultiTurnCoordinator comment**

Change line 304 from:

```swift
        // Single-turn coordinators removed - use MultiTurnCoordinator for conversations
```

to:

```swift
        // Single-turn coordinators removed - use HaloInputCoordinator for conversations
```

**Step 2: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/DI/DependencyContainer.swift`
Expected: Syntax valid

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/DI/DependencyContainer.swift
git commit -m "$(cat <<'EOF'
fix(di): update comment to reference HaloInputCoordinator

Removes stale MultiTurnCoordinator reference in comment.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Fix GatewayMultiTurnAdapter References

**Files:**
- Modify: `platforms/macos/Aether/Sources/Gateway/GatewayMultiTurnAdapter.swift`

**Step 1: Remove coordinator reference**

The `GatewayMultiTurnAdapter` has a `weak var coordinator: MultiTurnCoordinator?` reference that's used for callbacks. Since we're moving to a simpler event-based model, we'll remove this dependency.

Change line 83 from:

```swift
    weak var coordinator: MultiTurnCoordinator?
```

to:

```swift
    // Coordinator reference removed - using event-based callbacks instead
```

**Step 2: Remove configure method**

Change lines 113-117 from:

```swift
    /// Configure with coordinator reference
    func configure(coordinator: MultiTurnCoordinator) {
        self.coordinator = coordinator
        print("[GatewayMultiTurnAdapter] Configured with coordinator")
    }
```

to:

```swift
    /// Configure adapter (coordinator dependency removed)
    func configure() {
        print("[GatewayMultiTurnAdapter] Configured")
    }
```

**Step 3: Replace coordinator callbacks with empty implementations**

Replace all `coordinator?.handle*` calls with comments or remove them:

- Line 179: `coordinator?.handleThinking()` → `// Thinking state handled via events`
- Line 209: `coordinator?.handleToolStart(toolName: event.toolName)` → `// Tool start handled via events`
- Line 249: `coordinator?.handleToolResult(toolName: "", result: resultString)` → `// Tool result handled via events`
- Line 264: `coordinator?.handleStreamChunk(text: accumulatedText)` → `// Stream chunk handled via events`
- Line 290: `coordinator?.handleCompletion(response: response)` → `// Completion handled via events`
- Line 306: `coordinator?.handleError(message: errorMessage)` → `// Error handled via events`

**Step 4: Verify Swift syntax**

Run: `~/.uv/python3/bin/python Scripts/verify_swift_syntax.py platforms/macos/Aether/Sources/Gateway/GatewayMultiTurnAdapter.swift`
Expected: Syntax valid

**Step 5: Commit**

```bash
git add platforms/macos/Aether/Sources/Gateway/GatewayMultiTurnAdapter.swift
git commit -m "$(cat <<'EOF'
fix(gateway): remove MultiTurnCoordinator dependency from adapter

Removes coordinator reference and callbacks. Events are now handled
directly through the adapter's published properties and callbacks.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Add Localization Keys

**Files:**
- Modify: `platforms/macos/Aether/Resources/en.lproj/Localizable.strings`
- Modify: `platforms/macos/Aether/Resources/zh-Hans.lproj/Localizable.strings`

**Step 1: Add English localization keys**

Add to `en.lproj/Localizable.strings`:

```
// Commands
"commands.title" = "Commands";
"commands.search" = "Search commands...";
"commands.empty" = "No commands available";
```

**Step 2: Add Chinese localization keys**

Add to `zh-Hans.lproj/Localizable.strings`:

```
// Commands
"commands.title" = "命令";
"commands.search" = "搜索命令...";
"commands.empty" = "暂无可用命令";
```

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Resources/en.lproj/Localizable.strings platforms/macos/Aether/Resources/zh-Hans.lproj/Localizable.strings
git commit -m "$(cat <<'EOF'
i18n: add localization keys for command list

Adds commands.title, commands.search, commands.empty keys in
English and Chinese.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Add XcodeGen Entry

**Files:**
- Modify: `platforms/macos/project.yml`

**Step 1: Verify files are in Sources directory**

The new files are in:
- `Sources/Coordinators/HaloInputCoordinator.swift`
- `Sources/Components/HaloCommandListView.swift`

XcodeGen uses glob patterns, so if `Sources/**/*.swift` is already configured, no changes needed.

**Step 2: Regenerate Xcode project**

Run: `cd platforms/macos && xcodegen generate`
Expected: Project regenerated successfully

**Step 3: Commit (if project.yml changed)**

```bash
# Only if project.yml was modified
git add platforms/macos/project.yml
git commit -m "$(cat <<'EOF'
build: update XcodeGen for new coordinator files

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Build Verification

**Step 1: Build the project**

Run: `cd platforms/macos && xcodebuild -scheme Aether -configuration Debug build 2>&1 | head -100`
Expected: Build succeeds (or only warnings, no errors)

**Step 2: Fix any compilation errors**

If errors occur, analyze and fix them.

**Step 3: Final commit (if any fixes needed)**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix: resolve compilation errors for command system

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Summary

After completing all tasks:

| Component | Status |
|-----------|--------|
| `HaloInputCoordinator` | ✅ Created - handles hotkey and command detection |
| `HaloState.commandList` | ✅ Added - 8th state for command list UI |
| `HaloCommandListView` | ✅ Created - UI for / command |
| `HotkeyService` | ✅ Fixed - uses HaloInputCoordinator |
| `AppDelegate` | ✅ Fixed - uses HaloInputCoordinator |
| `DependencyContainer` | ✅ Fixed - comment updated |
| `GatewayMultiTurnAdapter` | ✅ Fixed - coordinator dependency removed |
| Localization | ✅ Added - English and Chinese keys |
| Build | ✅ Verified - no compilation errors |

**Resulting Behavior:**
- Press `Option+Space` → Halo shows listening state
- Clipboard starts with `//` → Shows history list
- Clipboard starts with `/` → Shows command list
- Clipboard is normal text → Processes as AI input
