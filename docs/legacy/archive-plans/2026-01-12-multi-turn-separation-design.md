# Multi-Turn Conversation Mode Separation Design

Date: 2026-01-12

## Overview

Completely separate single-turn and multi-turn conversation modes in Aleph, making multi-turn mode an independent conversation experience without relying on cursor focus detection.

## Design Goals

- Single-turn mode: Unchanged (double-tap Shift, process selected text, replace in place)
- Multi-turn mode: Fully independent conversation experience
  - Launch from anywhere without cursor focus
  - Output displayed only in floating window
  - Persistent conversation history with topic management

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Hotkey Layer                           │
├──────────────────────┬──────────────────────────────────────┤
│  Double-tap Shift    │  Cmd+Opt+/                           │
│  ↓                   │  ↓                                   │
│  InputCoordinator    │  MultiTurnCoordinator (new)          │
│  (single-turn)       │  (multi-turn, new design)            │
├──────────────────────┴──────────────────────────────────────┤
│                      UI Layer                               │
├──────────────────────┬──────────────────────────────────────┤
│  HaloWindow          │  MultiTurnInputWindow (new)          │
│  (spinner/toast)     │  + ConversationDisplayWindow (new)   │
├──────────────────────┴──────────────────────────────────────┤
│                      Data Layer                             │
├──────────────────────┬──────────────────────────────────────┤
│  (no persistence)    │  ConversationStore (new)             │
│                      │  → SQLite persistence                │
├──────────────────────┴──────────────────────────────────────┤
│                      Core Layer (Rust)                      │
│  Existing conversation API + new title generation           │
└─────────────────────────────────────────────────────────────┘
```

## New Swift Components

| Component | Responsibility |
|-----------|----------------|
| **MultiTurnCoordinator** | Main coordinator for multi-turn mode, manages hotkey, lifecycle, Rust interaction |
| **MultiTurnInputWindow** | Input window, centered, supports `//` command for topic list |
| **ConversationDisplayWindow** | Conversation floating window, top-right, displays conversation history |
| **ConversationDisplayView** | SwiftUI view for floating window, message list + copy buttons |
| **ConversationStore** | Persistence manager, wraps SQLite operations |
| **TopicListView** | Topic list view in SubPanel |

### Component Relationships

```
MultiTurnCoordinator
    ├── owns → MultiTurnInputWindow
    ├── owns → ConversationDisplayWindow
    ├── uses → ConversationStore (persistence)
    └── calls → AlephCore (Rust)
```

## Data Model

### SQLite Schema

```sql
-- Topics table
CREATE TABLE topics (
    id          TEXT PRIMARY KEY,     -- UUID
    title       TEXT NOT NULL,        -- AI-generated title
    created_at  INTEGER NOT NULL,     -- Unix timestamp
    updated_at  INTEGER NOT NULL,     -- Last activity time
    is_deleted  INTEGER DEFAULT 0     -- Soft delete flag
);

-- Messages table
CREATE TABLE messages (
    id          TEXT PRIMARY KEY,     -- UUID
    topic_id    TEXT NOT NULL,        -- Associated topic
    role        TEXT NOT NULL,        -- "user" | "assistant"
    content     TEXT NOT NULL,        -- Message content
    created_at  INTEGER NOT NULL,     -- Unix timestamp
    FOREIGN KEY (topic_id) REFERENCES topics(id)
);

-- Indexes
CREATE INDEX idx_messages_topic ON messages(topic_id);
CREATE INDEX idx_topics_updated ON topics(updated_at DESC);
```

### Swift Data Structures

```swift
struct Topic: Identifiable {
    let id: String
    var title: String
    let createdAt: Date
    var updatedAt: Date
}

struct Message: Identifiable {
    let id: String
    let topicId: String
    let role: MessageRole  // .user | .assistant
    let content: String
    let createdAt: Date
}
```

**Storage Location**: `~/.aleph/conversations.db`

## Interaction Flow

### Start New Conversation

```
User presses Cmd+Opt+/
    ↓
MultiTurnCoordinator.handleHotkey()
    ↓
┌─ Create new Topic (id = UUID, title = "New Conversation")
├─ Show MultiTurnInputWindow (centered)
└─ Show ConversationDisplayWindow (top-right, empty state)
    ↓
User types text, presses Enter
    ↓
┌─ Save user message to SQLite
├─ Call AlephCore.processInput()
├─ ConversationDisplayWindow shows user message + loading
    ↓
AI response returns
    ↓
┌─ Save assistant message to SQLite
├─ ConversationDisplayWindow shows AI reply
├─ Async call AI to generate title (after first turn)
└─ Update Topic.title
```

### Continue Conversation / Switch Topic

```
User types "//"
    ↓
SubPanel shows topic list (sorted by updated_at DESC)
    ↓
User selects a topic
    ↓
┌─ Load all messages for that topic from SQLite
├─ ConversationDisplayWindow shows conversation history
└─ User can continue asking questions
```

### Exit

```
User presses ESC
    ↓
MultiTurnCoordinator.exit()
    ↓
┌─ Hide MultiTurnInputWindow
└─ Hide ConversationDisplayWindow
    ↓
(Data already persisted, no extra save needed)
```

## UI Design

### ConversationDisplayWindow (Floating Window)

```
┌─────────────────────────────────────┐  ← Fixed width 360pt
│  ● Python Sorting Discussion  [Copy]│  ← Title bar + copy all button
├─────────────────────────────────────┤
│  ┌─────────────────────────────┐    │
│  │ 👤 Help me write quicksort  │[Copy]│  ← User message (copy button on right)
│  └─────────────────────────────┘    │
│                                     │
│  ┌─────────────────────────────┐    │
│  │ 🤖 Here's the implementation│[Copy]│  ← AI message
│  │ ```python                   │    │
│  │ def quicksort(arr):         │    │
│  │     ...                     │    │
│  │ ```                         │    │
│  └─────────────────────────────┘    │
│                                     │
│         ● ● ●  (loading)            │  ← AI thinking
│                                     │
└─────────────────────────────────────┘
         ↑ Height adaptive
         Min 200pt, Max 600pt
         Scrollable when overflow
```

### Visual Style

| Property | Value |
|----------|-------|
| Background | Semi-transparent blur (NSVisualEffectView) |
| Corner radius | 12pt |
| Shadow | Light drop shadow |
| User message | Right-aligned, light purple background |
| AI message | Left-aligned, gray background |
| Code blocks | Monospace font, dark background |

### Window Behavior

- Initial position: Top-right corner of screen
- Fixed width, height adaptive (min 200pt, max 600pt)
- Draggable to move position
- ESC closes both input window and display window

## Rust Core Changes

### New API

```rust
// aleph.udl addition

/// Generate title based on conversation content
fn generate_topic_title(user_input: string, ai_response: string) -> string;
```

### Implementation

```rust
const TITLE_PROMPT: &str = r#"
Generate a short Chinese title (max 15 characters) based on this conversation:

User: {user_input}
Assistant: {ai_response}

Return only the title, nothing else.
"#;

pub fn generate_topic_title(user_input: &str, ai_response: &str) -> Result<String> {
    let prompt = TITLE_PROMPT
        .replace("{user_input}", &user_input.chars().take(200).collect::<String>())
        .replace("{ai_response}", &ai_response.chars().take(200).collect::<String>());

    // Use lightweight model for fast, low-cost title generation
    let title = self.provider.complete_simple(&prompt)?;
    Ok(title.trim().to_string())
}
```

## Code Migration

### Components to Simplify

| Component | Change |
|-----------|--------|
| **UnifiedInputCoordinator** | Remove multi-turn logic, keep only single-turn command processing |
| **ConversationCoordinator** | Deprecate, migrate logic to `MultiTurnCoordinator` |
| **ConversationManager** | Deprecate, state management moves to `ConversationStore` |
| **ConversationInputView** | Deprecate, replaced by `MultiTurnInputWindow` |

### Components to Keep

| Component | Reason |
|-----------|--------|
| **InputCoordinator** | Single-turn selected text processing |
| **HaloWindow** | spinner/toast display for single-turn |
| **OutputCoordinator** | Single-turn output to target app |
| **FocusDetector** | Needed for single-turn |

## Implementation Steps

### Phase 1: Infrastructure (Rust + Data Layer)

- [ ] 1.1 Create `conversations.db` SQLite database initialization
- [ ] 1.2 Implement `ConversationStore` basic CRUD operations
- [ ] 1.3 Rust add `generate_topic_title` API
- [ ] 1.4 UniFFI binding update

### Phase 2: UI Components

- [ ] 2.1 Create `ConversationDisplayWindow` window framework
- [ ] 2.2 Create `ConversationDisplayView` message list view
- [ ] 2.3 Implement message bubble components (user/AI styles)
- [ ] 2.4 Implement copy functionality (single + all)
- [ ] 2.5 Create `MultiTurnInputWindow` (based on existing UnifiedInputWindow)

### Phase 3: Coordinator

- [ ] 3.1 Create `MultiTurnCoordinator` framework
- [ ] 3.2 Implement hotkey handling (reuse existing Cmd+Opt+/ listener)
- [ ] 3.3 Implement new conversation flow
- [ ] 3.4 Implement `//` command + topic list
- [ ] 3.5 Implement topic switching and history loading
- [ ] 3.6 Implement ESC unified exit

### Phase 4: Cleanup

- [ ] 4.1 Simplify `UnifiedInputCoordinator`
- [ ] 4.2 Remove deprecated components
- [ ] 4.3 Update AppDelegate dependency injection

### Phase 5: Testing & Polish

- [ ] 5.1 Unit tests (ConversationStore)
- [ ] 5.2 Integration tests (full conversation flow)
- [ ] 5.3 UI detail polish

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| SQLite for persistence | Already have rusqlite dependency for Memory module |
| AI-generated titles | Better UX than truncated first message |
| Permanent retention | Users can manually delete topics |
| SubPanel for topic list | Consistent with existing command completion UX |
| Separate coordinator | Clean separation, independent code paths |
