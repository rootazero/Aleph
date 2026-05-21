# Hierarchical Slash Commands — Step 2: Three-Client Interaction

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Update TUI, Telegram Bot, and WebChat to support hierarchical slash command interaction — both quick mode (`/session new topic`) and guided mode (`/session` → select subcommand → enter params).

**Architecture:** All three clients consume the tree-structured `commands.list` RPC response (from Step 1). Guided mode uses `command.execute` to detect namespaces and get children. Quick mode sends the full command as a chat message. Each client adapts the UX to its platform capabilities.

**Tech Stack:** Rust (TUI: ratatui), Rust/WASM (WebChat: Leptos), Rust (Bot: teloxide)

**Spec:** `docs/reference/2026-03-21-hierarchical-slash-commands-design.md`

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| Modify | `interfaces/tui/src/tui/app.rs` | Change `gateway_commands` from flat to tree |
| Modify | `interfaces/tui/src/tui/mod.rs` | Update `fetch_gateway_commands` to parse tree, handle guided mode |
| Modify | `interfaces/tui/src/tui/slash.rs` | Update command catalog for palette filtering |
| Modify | `interfaces/tui/src/tui/widgets/command_palette.rs` | Show namespaces + children in palette |
| Modify | `src/gateway/interfaces/telegram/mod.rs` | Register only top-level commands to Telegram |
| Modify | `src/gateway/interfaces/telegram/message_ops.rs` | Inline keyboard for namespace guided mode |
| Modify | `interfaces/webchat/src/views/chat/view.rs` | Tree-aware command palette + guided mode |

---

### Task 1: TUI — Tree-Aware Command Palette

The TUI command palette currently stores `Vec<(String, String)>` (flat name→description). Update it to understand namespaces and show hierarchical browsing.

**Files:**
- Modify: `interfaces/tui/src/tui/app.rs`
- Modify: `interfaces/tui/src/tui/mod.rs`
- Modify: `interfaces/tui/src/tui/slash.rs`
- Modify: `interfaces/tui/src/tui/widgets/command_palette.rs`

- [ ] **Step 1: Update `fetch_gateway_commands` to parse tree response**

In `interfaces/tui/src/tui/mod.rs`, the `fetch_gateway_commands` function currently parses flat `commands[*].key`. Update to parse the tree structure from Step 1:

```json
{ "commands": [
  { "name": "session", "is_namespace": true, "children": [...] },
  { "name": "search", "is_namespace": false, "hint": "Web search", "param_hint": "<query>" }
]}
```

Define a struct to hold the tree:
```rust
#[derive(Clone, Debug)]
pub struct CommandEntry {
    pub name: String,
    pub hint: String,
    pub is_namespace: bool,
    pub param_hint: Option<String>,
    pub children: Vec<CommandEntry>,
}
```

Parse the tree into `Vec<CommandEntry>` and store in `AppState`.

- [ ] **Step 2: Update `AppState.gateway_commands` type**

In `app.rs`, change:
```rust
pub gateway_commands: Vec<(String, String)>,
```
To:
```rust
pub gateway_commands: Vec<CommandEntry>,
```

Update `all_commands()` and `filter_commands()` to work with the new type. For the flat palette view, build display entries dynamically:
- Namespace → show as `"/session  → Session management"` (with indicator it has children)
- Independent → show as `"/search <query>  Web search"`
- When filtering inside a namespace → show children: `"/session new [topic]  Start new session"`

- [ ] **Step 3: Update command palette widget**

In `command_palette.rs`, update rendering:
- Show namespace entries with a `▸` indicator to signal they have children
- When user selects a namespace and presses Enter → enter "namespace mode":
  - Filter switches to show only that namespace's children
  - Input prefix changes to `/namespace `
- When user selects a child → fill input with `/namespace child ` and close palette
- Esc → go back one level (or close palette if at root)

The PaletteState needs a new field:
```rust
pub struct PaletteState {
    pub input: String,
    pub filtered: Vec<(String, String)>,
    pub selected: usize,
    pub namespace_stack: Vec<String>,  // NEW: current browsing path
}
```

- [ ] **Step 4: Handle guided mode in TUI event loop**

In `mod.rs`, when user types `/session` and presses Enter:
1. The text is parsed by `slash.rs` → `ParsedInput::Gateway("/session")`
2. Before sending to Gateway, check if it matches a known namespace from `gateway_commands`
3. If namespace → open command palette with that namespace's children pre-filtered
4. If not namespace → send as regular chat message

- [ ] **Step 5: Verify TUI compiles**

Run: `cargo check -p aleph-tui`

- [ ] **Step 6: Commit**

```bash
git add interfaces/tui/
git commit -m "tui: hierarchical command palette with namespace browsing"
```

---

### Task 2: Telegram Bot — Namespace Inline Keyboard

When Telegram user sends `/session`, the bot should respond with an inline keyboard showing subcommands.

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`
- Modify: `src/gateway/interfaces/telegram/message_ops.rs` (or wherever messages are sent)

- [ ] **Step 1: Read Telegram channel implementation**

Read `src/gateway/interfaces/telegram/mod.rs` and `message_ops.rs` fully to understand:
- How incoming messages are received
- How outgoing messages are sent
- How `slash_commands` are registered with Telegram API
- Where inline keyboard support exists

- [ ] **Step 2: Register only top-level commands**

Update the startup code that calls `setMyCommands`. Instead of registering all builtin tools, register only:
- Namespace names (`session`, `agent`, `plugin`, `skill`, `memory`, `image`, `speech`, `cron`, `vault`)
- Independent commands (`search`, `webfetch`, `switch`, `groupchat`, `snapshot`)

In the subsystems builder (`src/bin/aleph-server/commands/start/builder/subsystems.rs`), update the slash_commands building logic to derive top-level names from the tree.

- [ ] **Step 3: Handle namespace commands with inline keyboard**

When the bot receives `/session` (or any namespace-only command), instead of routing to the agent loop, send an inline keyboard reply:

```rust
// Pseudocode
if command_result.needs_interaction {
    let keyboard = InlineKeyboardMarkup::new(
        command_result.children.iter().map(|child| {
            vec![InlineKeyboardButton::callback(
                format!("{} — {}", child.name, child.hint),
                format!("/session {}", child.name),  // callback data
            )]
        })
    );
    bot.send_message(chat_id, "Session management:")
        .reply_markup(keyboard)
        .await?;
}
```

- [ ] **Step 4: Handle inline keyboard callback**

When user clicks a button, Telegram sends a callback query with the data (e.g., `/session new`). Handle it:
- If the callback data is a complete command (no params needed) → execute it
- If params needed → reply with "Enter parameter:" and wait for next message
- Route the assembled full command through the normal message pipeline

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore --lib`

- [ ] **Step 6: Commit**

```bash
git add src/gateway/interfaces/telegram/ src/bin/aleph-server/
git commit -m "telegram: inline keyboard for hierarchical namespace commands"
```

---

### Task 3: WebChat — Hierarchical Command Palette

**Files:**
- Modify: `interfaces/webchat/src/views/chat/view.rs`

- [ ] **Step 1: Read current command palette code**

Read `interfaces/webchat/src/views/chat/view.rs` lines 200-300 where command palette is implemented.

- [ ] **Step 2: Parse tree response**

Update the `fetch_commands` closure to parse the tree-structured `commands.list` response. Store as a tree structure (not flat).

- [ ] **Step 3: Update palette rendering**

When the palette is shown:
- Top level: show namespaces (with `▸` indicator) and independent commands
- User clicks a namespace → palette shows that namespace's children
- User clicks a child → fill input with `/namespace child ` and close palette
- Back button or Esc → return to top level

- [ ] **Step 4: Handle guided mode**

When user types `/session` and presses Enter:
- Check if it's a namespace
- If yes → show children in a dropdown/popup above the input
- User clicks child → input becomes `/session child |`

- [ ] **Step 5: Verify compilation**

Note: WebChat is a WASM crate, may need `trunk` or specific wasm target.

Run: `cargo check -p aleph-panel` (or whatever the webchat package name is)

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/
git commit -m "webchat: hierarchical command palette with namespace browsing"
```

---

### Task 4: Full Integration Verification

- [ ] **Step 1: Build all crates**

```bash
cargo check -p aleph-tui && cargo check -p alephcore --lib && cargo check -p aleph-cli
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo test -p aleph-tui --lib 2>&1 | tail -5
```

- [ ] **Step 3: Commit any fixes**

---

## Step 2 Complete Checklist

- [ ] TUI command palette shows tree structure with namespace browsing
- [ ] TUI guided mode: `/session` Enter → shows children → select → fill input
- [ ] Telegram registers only top-level commands
- [ ] Telegram namespace commands trigger inline keyboard
- [ ] WebChat command palette shows tree with namespace navigation
- [ ] All crates compile, tests pass
