# Unified Conversation Window Design

> Multi-turn conversation UI refactoring: merge input window and conversation display into a single unified window.

**Date**: 2026-01-18
**Status**: Draft

---

## Overview

This design refactors the multi-turn conversation mode from a two-window system to a single unified window, with several UX improvements:

1. **Attachment button** replaces automatic clipboard reading
2. **Conversation area** merged with input window (displayed above input)
3. **Slash commands** displayed above input (mutually exclusive with conversation)
4. **Window positioning** anchored at 70% screen height (from top)

---

## Current vs New Architecture

| Aspect | Current | New |
|--------|---------|-----|
| **Windows** | 2 separate windows | 1 unified window |
| **Window width** | 600px | 800px |
| **Window position** | Screen center + 150px up | Input bottom at 70% screen height |
| **Conversation area** | Top-right corner, independent | Above input (merged) |
| **Conversation height** | 200-600px | Dynamic, max 600px |
| **Attachment handling** | Auto-read clipboard | Manual button / drag-drop |
| **Attachment preview** | None | Expandable above input |
| **Command list** | Below input | Above input, mutually exclusive with conversation |
| **ESC behavior** | Close directly | Layered exit |

---

## Window Layout & Positioning

### Positioning Calculation

```
Screen Top ════════════════════════════════════════════ 100%
    │
    │   (Reserved space: 30% of screen height)
    │
    ▼
    ┌────────────────────────────────────────────┐
    │         Conversation Area (dynamic)        │
    │            Max height 600px                │
    │         (Hidden when no messages)          │
    ├────────────────────────────────────────────┤
    │   ┌──────┐ ┌──────┐                        │  ← Attachment preview
    │   │ 📷 × │ │ 📄 × │   (expandable)         │    (on demand)
    │   └──────┘ └──────┘                        │
    ├────────────────────────────────────────────┤
    │  [Input field....................] [＋][▶]  │  ← Input area
    └────────────────────────────────────────────┘
                                                 ↑
                                          Input bottom
    │                                    anchored here
    │
Screen Bottom ════════════════════════════════════════════ 0%
             ← 30% screen height →
```

### Core Positioning Logic

```swift
let screenHeight = screen.frame.height
let inputAreaHeight: CGFloat = 60
let anchorY = screenHeight * 0.30  // 30% from bottom

// Window bottom edge = 30% screen height
let windowOriginY = anchorY

// Horizontal center
let windowOriginX = (screen.frame.width - 800) / 2
```

### Dynamic Window Height

| State | Window Height |
|-------|---------------|
| Initial (input only) | ~60px |
| With attachment preview | +80px |
| With conversation (few messages) | +content height |
| Conversation at max | +600px (limit) |

---

## Input Area Design

### Layout

```
┌──────────────────────────────────────────────────────────────────┐
│                                                                  │
│  ┌────────────────────────────────────────────────┐ ┌───┐ ┌───┐ │
│  │  Input content...                              │ │ ＋ │ │ ▶ │ │
│  │                                                │ │   │ │   │ │
│  └────────────────────────────────────────────────┘ └───┘ └───┘ │
│   ↑                                                  ↑     ↑    │
│   IMETextField (multi-line expandable)          Attach  Send   │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### Attachment Interaction

| User Action | System Response |
|-------------|-----------------|
| Click [＋] button | Open NSOpenPanel (multi-select) |
| Drag file to window | Detect file type, add to list |
| Add successful | Expand preview area above input |

### Attachment Preview

```
┌──────────────────────────────────────────────────────────────────┐
│  ┌─────────┐ ┌─────────┐ ┌─────────┐                            │
│  │  📷     │ │  📄     │ │  📁     │                            │
│  │ img.png │ │ doc.pdf │ │ data... │                            │
│  │    ✕    │ │    ✕    │ │    ✕    │    ← Click ✕ to remove     │
│  └─────────┘ └─────────┘ └─────────┘                            │
├──────────────────────────────────────────────────────────────────┤
│  [Describe these files...                 ] [＋] [▶]            │
└──────────────────────────────────────────────────────────────────┘
```

---

## Display State Machine

### State Diagram

```
                    ┌─────────────┐
                    │   Empty     │  ← Initial state (input only)
                    └──────┬──────┘
                           │ Send first message
                           ▼
                    ┌─────────────┐
         ┌─────────│ Conversation│←─────────┐
         │         └──────┬──────┘          │
         │                │                 │
    Type / or //    Cancel/Select done   Type / or //
         │                │                 │
         ▼                │                 │
    ┌─────────────┐       │                 │
    │ CommandList │───────┘                 │
    └─────────────┘                         │
         │                                  │
         └──────────────────────────────────┘
```

### Three Display States

**State 1: Empty (Initial)**
```
┌────────────────────────────────────────────┐
│  [Enter message...                ] [＋][▶]  │
└────────────────────────────────────────────┘
```

**State 2: Conversation (Active)**
```
┌────────────────────────────────────────────┐
│  👤 User: Help me analyze this problem     │
│  🤖 Assistant: Sure, let me take a look... │
│  👤 User: Any other options?               │
├────────────────────────────────────────────┤
│  [Continue asking...              ] [＋][▶]  │
└────────────────────────────────────────────┘
```

**State 3: CommandList (Selecting)**
```
┌────────────────────────────────────────────┐
│  /search    - Search web                   │
│  /youtube   - Search videos                │
│  /memory    - Memory retrieval  ← keyboard │
│  /webfetch  - Fetch webpage                │
├────────────────────────────────────────────┤
│  [/sea                          ] [＋][▶]  │
└────────────────────────────────────────────┘
```

### State Management

```swift
enum ContentDisplayState {
    case empty                        // No conversation, no commands
    case conversation                 // Show conversation history
    case commandList(prefix: String)  // "/" or "//"
}

func updateDisplayState(input: String) {
    if input.hasPrefix("//") {
        state = .commandList(prefix: "//")  // Topic list
    } else if input.hasPrefix("/") {
        state = .commandList(prefix: "/")   // Command list
    } else if hasMessages {
        state = .conversation
    } else {
        state = .empty
    }
}
```

---

## ESC Layered Exit

### Logic

```swift
func handleEscapeKey() {
    switch currentState {
    case .commandList:
        // Layer 1: Close command list, restore previous state
        clearCommandInput()
        state = hasMessages ? .conversation : .empty

    case .conversation, .empty:
        // Layer 2: Close entire window
        if hasAttachments {
            clearAttachments()
        }
        closeWindow()
    }
}
```

### Keyboard Interaction Table

| Key | State | Behavior |
|-----|-------|----------|
| `ESC` | Command list showing | Close list, return to conversation/empty |
| `ESC` | Conversation/empty | Close window, exit multi-turn mode |
| `↑` `↓` | Command list showing | Navigate selection |
| `Tab` / `Enter` | Command list showing | Confirm selection, fill command |
| `Enter` | Input has content | Send message |
| `Cmd+V` | Any state | Paste text (no auto-attach) |
| `Cmd+Opt+/` | Window open | Focus window |

---

## Data Models

### PendingAttachment

```swift
struct PendingAttachment: Identifiable {
    let id: UUID
    let url: URL              // Local file path
    let fileName: String      // Display name
    let fileType: FileType    // image/document/other
    let thumbnail: NSImage?   // Preview thumbnail
    let data: Data            // Binary data (for sending)
}

enum FileType {
    case image      // Show thumbnail preview
    case document   // Show file icon + name
    case other      // Generic file icon
}
```

### ContentDisplayState

```swift
enum ContentDisplayState {
    case empty
    case conversation
    case commandList(prefix: String)
}
```

### UnifiedConversationViewModel

```swift
@Observable
class UnifiedConversationViewModel {
    // Display state
    var displayState: ContentDisplayState = .empty

    // Conversation data
    var messages: [ConversationMessage] = []
    var currentTopicId: String?

    // Attachment data
    var pendingAttachments: [PendingAttachment] = []

    // Input state
    var inputText: String = ""
    var isProcessing: Bool = false

    // Computed properties
    var shouldShowConversation: Bool { ... }
    var shouldShowCommandList: Bool { ... }
    var shouldShowAttachmentPreview: Bool { ... }
    var windowHeight: CGFloat { ... }  // Dynamic calculation
}
```

---

## File Changes

| Action | File | Description |
|--------|------|-------------|
| **Create** | `UnifiedConversationWindow.swift` | Replace two windows |
| **Create** | `UnifiedConversationView.swift` | Main view (conversation+attachments+input) |
| **Create** | `AttachmentPreviewView.swift` | Attachment preview component |
| **Create** | `AttachmentManager.swift` | Attachment state management |
| **Refactor** | `MultiTurnCoordinator.swift` | Adapt to new window |
| **Modify** | `MultiTurnInputView.swift` | Simplify, remove command list logic |
| **Deprecate** | `MultiTurnInputWindow.swift` | Merged into unified window |
| **Deprecate** | `ConversationDisplayWindow.swift` | Merged into unified window |
| **Deprecate** | `ConversationDisplayView.swift` | Merged into unified window |
| **Modify** | `ClipboardMonitor.swift` | Remove auto-read trigger |

### New View Hierarchy

```
UnifiedConversationWindow (NSWindow)
└── UnifiedConversationView (SwiftUI)
    ├── ConversationAreaView        // Conversation history (conditional)
    │   └── MessageBubbleView × N
    ├── CommandListView             // Command list (conditional)
    │   └── CommandRowView × N
    ├── AttachmentPreviewView       // Attachment preview (conditional)
    │   └── AttachmentThumbnail × N
    └── InputAreaView               // Input area (always visible)
        ├── IMETextField
        ├── AttachmentButton [＋]
        └── SendButton [▶]
```

---

## UX Improvements

1. **Single focus** - One window for all interactions, no eye switching
2. **Clear intent** - Attachments require manual action, no clipboard misreads
3. **Visual consistency** - Command list, conversation, attachments all above input
4. **Natural positioning** - Window positioned lower, comfortable for typing
