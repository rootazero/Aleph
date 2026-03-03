# Chat Persistence & Memory Pipeline Fix

> Date: 2026-03-03
> Status: Approved
> Scope: Session persistence, sidebar refresh, auto-memorization

## Problem

Three related issues prevent the chat system from working as expected:

1. **Chat sidebar doesn't show topics**: Only loads sessions on initial WebSocket connection, never refreshes.
2. **Chat records disappear after closing**: Session data may not persist correctly to SQLite due to missing in-memory session pre-creation.
3. **Memory Dashboard is empty**: No pipeline bridges chat conversations to the memory system (LanceDB).

## Root Cause Analysis

### Issue 1: Sidebar Refresh

`chat_sidebar.rs` fetches `sessions.list` once in an `Effect` triggered by `is_connected`. No event subscription or polling mechanism exists for subsequent updates.

### Issue 2: Session Persistence

`ExecutionEngine::execute()` calls `agent.add_message()` which:
- Checks in-memory HashMap (`sessions.get_mut`) — **fails if session not pre-created**
- Writes to SQLite via SessionManager — **works independently but state diverges**

The in-memory session is never pre-created because `execute()` skips `ensure_session()` before `add_message()`.

### Issue 3: Memory Pipeline

The execution flow writes ONLY to `SessionManager` (SQLite sessions.db). The memory system (`SessionStore` in LanceDB for Layer 1, `MemoryStore` for Layer 2) receives zero data from conversations. Memory tools (profile_update, scratchpad, memory_search) are agent-invoked tools, not automatic pipelines.

## Design

### Approach: Event-Driven + Async Memory Pipeline

Selected over polling (wasteful) and unified storage (breaks three-layer memory architecture).

### Part 1: Session Persistence Fix

**Files**: `agent_instance.rs`, `execution_engine/engine.rs`

Add `ensure_session()` to `AgentInstance`:
- Creates session in in-memory HashMap if missing
- Creates session in SQLite via SessionManager if missing
- Called by `ExecutionEngine::execute()` before first `add_message()`

```rust
// AgentInstance
pub async fn ensure_session(&self, key: &SessionKey) {
    let key_str = key.to_key_string();
    // Ensure in-memory
    {
        let mut sessions = self.sessions.write().await;
        sessions.entry(key_str.clone()).or_insert_with(|| {
            SessionData { messages: Vec::new(), created_at: now, last_active_at: now }
        });
    }
    // Ensure SQLite
    if let Some(ref sm) = self.session_manager {
        let _ = sm.get_or_create(key).await;
    }
}
```

### Part 2: Sidebar Event-Driven Refresh

**Files**: `event_emitter.rs`, `execution_engine/engine.rs`, `chat_sidebar.rs`

Backend:
- Add `SessionUpdated { session_key, message_count }` variant to `StreamEvent`
- Emit after successful run completion in `ExecutionEngine::execute()`

Frontend:
- Subscribe to `session.*` events in sidebar component
- Extract `reload_sessions()` as reusable function
- Call on: initial connect, event received, message send success (dual guarantee)

### Part 3: Auto-Memorization Pipeline

**Files**: `execution_engine/engine.rs`, `execution_engine/mod.rs`, `start/mod.rs`, `server_init.rs`

Add `memory_backend: Option<MemoryBackend>` to `ExecutionEngine`.

After successful run completion:
```rust
tokio::spawn(async move {
    write_conversation_memory(&memory_db, &session_key, &user_input, &ai_output).await
});
```

Creates `MemoryEntry` with:
- `app_bundle_id`: "aleph.chat"
- `window_title`: session_key (for filtering)
- `embedding`: empty (filled by compression pipeline later)
- `topic_id`: session_key

Data flow:
```
chat.send → ExecutionEngine::execute()
  → SessionManager (SQLite)     [session persistence]
  → SessionStore (LanceDB L1)   [memory pipeline, async]
  → EventBus (session.updated)  [sidebar refresh]
```

## Files Changed

| File | Change |
|------|--------|
| `core/src/gateway/agent_instance.rs` | Add `ensure_session()` |
| `core/src/gateway/execution_engine/engine.rs` | Add `memory_backend`, call `ensure_session`, write L1, emit event |
| `core/src/gateway/execution_engine/mod.rs` | Update `new()` signature |
| `core/src/gateway/event_emitter.rs` | Add `SessionUpdated` variant |
| `core/ui/control_plane/src/components/chat_sidebar.rs` | Event subscription, `reload_sessions()` |
| `core/src/bin/aleph_server/commands/start/mod.rs` | Pass `memory_db` to engine |
| `core/src/bin/aleph_server/server_init.rs` | Adapt `handle_*_with_engine` signatures |

## Files NOT Changed

- `SessionManager` — already complete
- `memory.rs` (handlers) — already has search/stats/delete
- `memory.rs` (views) — Dashboard UI already renders data
- Compression pipeline — already processes Layer 1 → Layer 2

## Risks

| Risk | Level | Mitigation |
|------|-------|------------|
| LanceDB write failure | Low | Async spawn + warn! log, non-blocking |
| EventBus event loss | Low | Dual guarantee: event + proactive refresh |
| Empty embedding limits semantic search | Medium | FTS and time-sort still work; compression pipeline fills later |
| `ExecutionEngine::new()` signature change | Low | Only 2 call sites |
