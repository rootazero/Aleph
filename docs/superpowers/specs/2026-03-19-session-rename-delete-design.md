# Session Rename & Delete — Design Spec

**Date**: 2026-03-19
**Scope**: Session rename & delete — UI buttons + LLM tool for natural language rename

## Problem

The session list in the webchat sidebar is read-only. Users cannot rename session topics or delete sessions with their history. This is basic session management that users expect (cf. ChatGPT). Additionally, per R9 (Everything is a Tool), session topic renaming should also be available as a builtin tool so users can rename sessions through natural language in the chat.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Trigger mechanism | Hover → ⋯ button → dropdown menu | ChatGPT pattern, balances clean UI with discoverability |
| Rename UX | Inline edit (input replaces title) | Lightest-weight, no modal needed |
| Delete confirmation | Inline confirm (row turns red, confirm/cancel buttons) | Non-modal, less disruptive than a dialog |

## Backend: New `sessions.set_topic` RPC

A single new RPC endpoint. No schema changes, no new SessionManager methods.

**Method**: `sessions.set_topic`

**Params**:
```json
{ "session_key": "agent:main:main:s0", "topic": "New title" }
```

**Response**:
```json
{ "session_key": "agent:main:main:s0", "updated": true }
```

**Validation**:
- Backend enforces max 100 characters for topic (truncate or reject). Boundary defense per P7.

**Implementation**:
- New handler `handle_set_topic_db()` in `src/gateway/handlers/session/db_handlers.rs`
- Re-export from `src/gateway/handlers/session/mod.rs` (add to `pub use db_handlers::{...}` block)
- Calls existing `SessionManager::set_topic()`
- Register in `src/bin/aleph/commands/start/builder/handlers.rs` as `"sessions.set_topic"`

Existing endpoints used:
- `sessions.delete` — already implemented and registered

## Builtin Tool: `session_set_topic`

A new builtin tool following the `AlephTool` trait pattern (cf. `session_new` in `new_tool.rs`). Enables natural language session renaming: user says "把这次对话改名叫项目架构讨论", LLM calls this tool.

**Tool name**: `session_set_topic`

**Description**: "Rename the current session's topic. Use when the user asks to change, rename, or set the conversation title/topic."

**Args** (`SessionSetTopicArgs`):
```rust
pub struct SessionSetTopicArgs {
    /// The new topic/title for the session.
    pub topic: String,

    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,
}
```

**Output** (`SessionSetTopicOutput`):
```rust
pub struct SessionSetTopicOutput {
    pub session_key: String,
    pub topic: String,
    pub message: String,  // e.g. "会话主题已更新为: 项目架构讨论"
}
```

**Behavior**:
- Trim topic, reject if empty or > 100 chars
- Call `SessionManager::set_topic()` with the injected `__session_key`
- Return human-readable confirmation message

**File**: `src/builtin_tools/sessions/set_topic_tool.rs`

**Registration**: Add to `src/builtin_tools/sessions/mod.rs` exports, register in tool registry alongside `session_new`.

**Examples**:
```
session_set_topic(topic="项目架构讨论")
session_set_topic(topic="Debug WASM compilation issues")
```

## Frontend: `chat_sidebar.rs` Modifications

All changes contained within `apps/panel/src/components/chat_sidebar.rs`. No new files.

### New State Signals

```rust
let editing_key: RwSignal<Option<String>>  // session key being renamed
let deleting_key: RwSignal<Option<String>> // session key in delete-confirm state
let edit_text: RwSignal<String>            // temporary text in edit input
let menu_open_key: RwSignal<Option<String>> // session key with open ⋯ menu
```

### Session Item Rendering — Three Modes

**1. Normal mode** (default):
- Current layout: title + subtitle (message count, date)
- Wrap in a `group` container so hover reveals the ⋯ button on the right
- Click ⋯ → toggle dropdown with "Rename" and "Delete"

**2. Edit mode** (`editing_key == this session's key`):
- Title replaced by `<input>` element
- Pre-filled with current topic, auto-focused, text fully selected
- Enter → trim, if non-empty call `sessions.set_topic` then reload; if empty, cancel. Input disabled during RPC to prevent double-submit.
- Esc → cancel (restore normal mode)
- Max length: 100 characters (frontend + backend enforced)

**3. Delete-confirm mode** (`deleting_key == this session's key`):
- Row background turns red/danger color
- Text changes to "Confirm delete?"
- Two small buttons: "Confirm" and "Cancel"
- Confirm → call `sessions.delete`, reload sessions, if deleting active session call `chat.clear_session()`
- Esc → cancel confirm mode
- Cancel button or 5-second timeout → exit confirm mode
- Confirm button disabled during RPC call to prevent double-click

### ⋯ Dropdown Menu

- Controlled by `menu_open_key` signal
- Click ⋯ toggles; click outside (document-level click listener) or select action closes. Clicking another session row closes menu AND selects that session.
- Menu items: "Rename" (sets editing_key), "Delete" (sets deleting_key)
- Mutual exclusion: only one session can have menu/edit/confirm active at a time

### API Calls

- **Rename**: `rpc_call("sessions.set_topic", { session_key, topic })` → on success, reload session list
- **Delete**: `rpc_call("sessions.delete", { session_key })` → on success, reload session list; if deleted session was active, `chat.clear_session()`

## Edge Cases

- Empty topic after trim → cancel edit (do not save)
- Delete active session → clear chat area, do not auto-navigate
- 5-second auto-dismiss on delete confirm → prevents forgotten confirm state
- Esc cancels delete confirm mode
- All states (menu, edit, confirm) are mutually exclusive across all session items

## Out of Scope

- Batch delete
- Undo / trash / recycle bin
- Drag-to-reorder sessions
- Session search functionality (existing placeholder remains as-is)
- Topic auto-generation changes

## Files Changed

| File | Change |
|------|--------|
| `src/gateway/handlers/session/db_handlers.rs` | Add `handle_set_topic_db()` |
| `src/gateway/handlers/session/mod.rs` | Re-export `handle_set_topic_db` |
| `src/bin/aleph/commands/start/builder/handlers.rs` | Register `sessions.set_topic` |
| `src/builtin_tools/sessions/set_topic_tool.rs` | New `SessionSetTopicTool` (AlephTool impl) |
| `src/builtin_tools/sessions/mod.rs` | Export `set_topic_tool` module and types |
| Tool registry registration site | Register `session_set_topic` alongside `session_new` |
| `apps/panel/src/components/chat_sidebar.rs` | Add rename/delete UI with three-mode rendering |
