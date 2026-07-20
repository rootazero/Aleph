# Chat Sidebar Redesign — Two-Level Agent/Session Hierarchy

## Summary

Redesign the panel ChatSidebar from a flat session list to a two-level collapsible
hierarchy: Level 1 = agents, Level 2 = sessions grouped under each agent. Add
automatic session topic generation on first message, an explicit `agent_id` signal
in ChatState, and backend extensions to expose topic and agent_id in `sessions.list`.

## Motivation

- Current sidebar shows a flat list of sessions with opaque labels like `"main (main)"`
- No way to see which agent a session belongs to or navigate between agents
- No session topics — users cannot identify conversations at a glance
- `agent_id` is fragily parsed from session_key strings instead of being a first-class signal
- Users on non-panel channels (Telegram, Bot) create sessions that never get titles

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Topic generation timing | First message in session | Like ChatGPT; works for all channels; /new would leave most sessions untitled |
| Topic generation method | LLM call: prefer lightweight model, fallback to current agent's model | Cost-effective when configured; always works via fallback |
| Sidebar layout | Collapsible agent groups | Shows all agents + sessions in one view; collapse controls complexity |
| agent_id in ChatState | Dedicated `RwSignal<Option<String>>` | Independent routing signal; needed when creating new chat (no session_key yet) |
| Backend data source | Extend `SessionListRow` | `SessionMetadata` already has agent_id + metadata_json; just expose them |

## Backend Changes

### 1. Extend `SessionListRow`

**File:** `src/builtin_tools/sessions/list_tool.rs`

Add two fields to `SessionListRow`:

```rust
pub struct SessionListRow {
    pub key: String,
    pub kind: String,
    pub channel: String,
    pub updated_at: Option<i64>,
    pub message_count: usize,
    pub messages: Option<Vec<StoredMessage>>,
    // NEW
    pub agent_id: String,
    pub topic: Option<String>,
}
```

- `agent_id`: taken directly from `SessionMetadata.agent_id`
- `topic`: extracted from `SessionMetadata.metadata_json` by parsing JSON and reading the `"topic"` key

**Implementation detail:** The `metadata_to_row` function must be updated to:
1. Parse `meta.metadata_json` as `serde_json::Value`
2. Extract `.topic` as `Option<String>`
3. Pass through `meta.agent_id`

### 2. Auto-generate topic on first message

**Trigger point:** `handle_chat_send_with_engine` in `server_init.rs`

**LLM selection strategy:** The function has generic `P: ProviderRegistry`.

1. **Prefer lightweight model:** Check if the registry has a provider registered under
   a lightweight model name (e.g., `provider_registry.get("haiku")` or a config-defined
   `topic_model`). Lightweight models are cheaper and faster for this trivial task.
2. **Fallback to current agent's model:** If no lightweight model is configured, use
   the resolved agent's model (already available as `agent.model` in the function scope).
   Call `provider_registry.get(&agent.model)` or `provider_registry.default_provider()`.

This ensures topic generation always works — users who configured a lightweight model
get cost savings, others seamlessly use their existing model.

**LLM call:** Use `provider.process()` with a minimal `RequestPayload`:

```rust
let payload = RequestPayload {
    messages: &[UnifiedMessage::user(&format!(
        "Generate a concise topic title (5-10 characters, same language as the message) \
         for a conversation that starts with: {}", user_message
    ))],
    system_prompt: Some("You are a title generator. Output ONLY the title, nothing else."),
    tools: None,
    think_level: None,
    temperature: Some(0.3),
    max_tokens: Some(30),
};
```

**When to trigger:** When `chat.send` receives `session_key: None` (new session), after
the normal response is returned to the user:

1. Execute `chat.send` normally — return `run_id` + `session_key` immediately
2. Clone the `ProviderRegistry` reference and `tokio::spawn` an async task:
   - Call `default_provider().process(payload)` with the lightweight prompt above
   - Extract the text response
   - Write the topic via `session_manager.update_metadata(key, "topic", generated_topic)`
   - Emit `stream.session_updated` event so frontends refresh

**Edge cases:**
- Topic generation failure (LLM timeout, network error) → log warning, silently ignore; session displays as "New Chat"
- Rapid second message before topic generated → do not re-trigger (check if topic already exists in metadata before spawning)
- `/new` closing old session without topic → fallback: spawn topic generation from the old session's first user message

### 3. `/new` command simplification

**File:** `src/builtin_tools/sessions/new_tool.rs`

`session_new` becomes simpler:
- Close current session, open new empty session
- Does NOT generate topic itself
- If old session has no topic at close time, trigger a fallback topic generation (same async LLM mechanism)

## Frontend Changes

### 4. ChatState extension

**File:** `apps/panel/src/views/chat/state.rs`

```rust
pub struct ChatState {
    // ... existing fields ...
    pub agent_id: RwSignal<Option<String>>,  // NEW
}
```

New method:
- `clear_session(&self)` — clears messages, session_key, phase, reasoning_text, errors but KEEPS agent_id (for "+" new chat within same agent)

Existing `clear()` resets everything including agent_id.

**Default agent on init:** `agent_id` starts as `None`. When no agent is selected, messages route to the default agent (existing backend behavior). The sidebar auto-selects the default agent's group as expanded on mount.

### 5. ChatSidebar rewrite

**File:** `apps/panel/src/components/chat_sidebar.rs`

**Data fetching:**
- On mount (when connected): parallel calls to `agents.list` + `sessions.list`
- Subscribe to `stream.session_updated` for live refresh (existing mechanism)

**SessionEntry updated:**
```rust
#[derive(Debug, Clone, Deserialize)]
struct SessionEntry {
    key: String,
    agent_id: String,
    topic: Option<String>,
    message_count: u32,
    last_active_at: String,
}
```

Note: Remove `#[serde(rename_all = "camelCase")]` from the frontend struct. The backend
`SessionListRow` serializes as snake_case. Frontend must match. The current `SessionEntry`
has `#[serde(rename_all = "camelCase")]` with `#[serde(default)]` on fields, which means
`agent_id` and `message_count` have been silently deserializing as defaults (empty/zero).
Fix: use snake_case on the frontend to match backend serialization.

**AgentEntry (new):**
```rust
#[derive(Debug, Clone, Deserialize)]
struct AgentEntry {
    id: String,
    name: String,
    is_active: bool,
}
```

**Rendering logic:**
1. Group sessions by `agent_id` into a `HashMap<String, Vec<SessionEntry>>`
2. For each agent from `agents.list`:
   - Collapsible header: agent name + session count + "+" button
   - Active agent's group (or default agent if none selected) expanded by default, others collapsed
   - Under header: session list sorted by `last_active_at` desc
3. Session item displays: topic (or "New Chat" if None) + subtitle (msg count + date)

**Interactions:**
- Click session → `chat.clear_session()` first, then `chat.agent_id.set(Some(agent_id))`, then `chat.session_key.set(Some(key))`, then load history. Order matters: `clear_session()` preserves agent_id from previous state but clears messages; then we set the new agent_id and session_key.
- Click "+" on agent → `chat.clear_session()`, then `chat.agent_id.set(Some(agent_id))` — starts new empty chat under that agent

### 6. InputArea simplification

**File:** `apps/panel/src/views/chat/view.rs`

In `send_message`:
- Read `chat.agent_id.get()` directly instead of parsing from session_key
- Remove the `session_key.split(':')` agent_id extraction logic (lines 260-267)

## Data Flow

```
User sends first message
  → InputArea reads chat.agent_id, passes to ChatApi::send
  → Backend chat.send creates session, returns session_key
  → Backend spawns async topic generation (ProviderRegistry::default_provider().process())
  → Topic written to session metadata
  → stream.session_updated event emitted
  → ChatSidebar receives event, re-fetches sessions.list
  → New session appears in sidebar with generated topic

User clicks "+" on agent
  → chat.clear_session(), chat.agent_id = Some("coder")
  → Next message creates new session under "coder" agent
  → Topic auto-generated from first message

User clicks existing session
  → chat.clear_session()
  → chat.agent_id + chat.session_key set
  → History loaded, messages rendered
```

## Files Modified

| File | Change |
|------|--------|
| `src/builtin_tools/sessions/list_tool.rs` | Add agent_id, topic to SessionListRow; update metadata_to_row |
| `src/bin/aleph/server_init.rs` | Spawn async topic generation via ProviderRegistry on first message |
| `src/builtin_tools/sessions/new_tool.rs` | Simplify: no topic gen, just close+open; fallback topic gen if missing |
| `apps/panel/src/views/chat/state.rs` | Add agent_id signal, clear_session() method |
| `apps/panel/src/components/chat_sidebar.rs` | Full rewrite: two-level hierarchy, fix serde to snake_case |
| `apps/panel/src/views/chat/view.rs` | Simplify agent_id handling in send_message |

## Not In Scope

- Search functionality (existing placeholder stays as-is)
- Cross-channel session visibility filtering
- Session deletion from sidebar
- Agent creation/management from sidebar
- Closed session filtering (all sessions shown regardless of status)
- Optimizing session_updated to carry payload (full re-fetch for now)
