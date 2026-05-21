# Session Isolation Design

Date: 2026-03-14

## Problem

当前一个 bot 聊天窗口或 webchat 窗口的 session 永续延伸，上下文无限堆积。缺少"新建对话"能力，记忆系统也无法按 session 维度过滤。

## Goals

1. `/new` 斜杠命令在 Gateway 层统一拦截，开启新 session
2. 切换 agent 等同 `/new`，为旧 session 生成主题后切换
3. LLM 在关闭旧 session 时即时生成主题摘要
4. 记忆系统按 session 主题相似度做两阶段检索过滤
5. 清理遗留 `topic_id` 技术债，替换为 `session_id`

## Non-Goals

- Session 归档/压缩（旧 session 保留不动）
- 前端 UI 重新设计（仅适配新字段）
- 跨 session 记忆合并

---

## Design

### 1. SessionKey Epoch 扩展

在 `SessionKey::Main` 和 `SessionKey::DirectMessage` 中加入 `epoch: u32`：

```rust
enum SessionKey {
    Main {
        agent_id: String,
        main_key: String,
        epoch: u32,           // default 0
    },
    DirectMessage {
        agent_id: String,
        channel: String,
        peer_id: String,
        dm_scope: DmScope,
        epoch: u32,           // default 0
    },
    // Group, Task, Subagent, Ephemeral unchanged
}
```

Serialization format (backward compatible):
```
epoch=0:  agent:main:main          (no suffix)
epoch=1:  agent:main:main:s1
epoch=2:  agent:main:dm:user123:s2
```

Parse: keys without `:sN` suffix are epoch=0.

### 2. SessionMetadata Topic

Store topic and status in existing `sessions.metadata` JSON column (no schema change):

```json
{
  "role": "Owner",
  "identity_id": "owner",
  "source_channel": "telegram",
  "topic": "讨论了 Rust 并发测试和 loom 配置",
  "topic_embedding": [0.1, 0.2, ...],
  "epoch": 2,
  "status": "closed"
}
```

- `topic: Option<String>` — LLM-generated session topic
- `topic_embedding: Option<Vec<f32>>` — topic embedding for similarity search
- `epoch: u32` — version number
- `status: "active" | "closed"` — session state

### 3. topic_id → session_id Cleanup

Replace all `topic_id` with `session_id` across the codebase (~40 mechanical changes):

**ContextAnchor** (`memory/context/mod.rs`):
```rust
pub struct ContextAnchor {
    pub window_title: String,
    pub timestamp: i64,
    pub session_id: String,      // was topic_id
}
```

- `SINGLE_TURN_TOPIC_ID` → `NO_SESSION` constant (value `"none"`)
- `with_topic()` → `with_session()`

**Other structs** — same rename:
- `CapturedContext::topic_id` → `session_id`
- `InputEvent::topic_id` → `session_id`
- `RequestContext::topic_id` → `session_id`

**Storage**:
- LanceDB `memories` table: `topic_id` column → `session_id`
- SQLite `state_database`: `topic_id` column → `session_id`

### 4. Gateway `/new` Interception

In `inbound_router.rs` slash command handling (line ~599-646), add `/new` alongside `/switch`:

```rust
if text.starts_with('/') {
    match command {
        "/switch" => { /* existing + close old session */ },
        "/new"    => { /* new: close old, create new epoch */ },
        // ...
    }
}
```

`/new` flow:
1. Get current session_key (with epoch)
2. Call `session_manager.close_session(old_key, llm_provider)`
3. Increment epoch, create new session_key
4. Call `session_manager.get_or_create(new_key)`
5. Reply to user: "新对话已开始" (no LLM involved)

`/switch` flow addition:
- Before switching agent, call `close_session` on current agent's session

### 5. SessionManager New Methods

```rust
impl SessionManager {
    /// Close session: generate topic via LLM, mark as closed
    pub async fn close_session(
        &self,
        key: &SessionKey,
        llm_provider: &dyn LoopProvider,
    ) -> Result<SessionCloseResult, SessionManagerError>;

    /// Get current epoch for a base session key pattern
    pub async fn get_current_epoch(
        &self,
        agent_id: &str,
        base_key_pattern: &str,
    ) -> Result<u32, SessionManagerError>;

    /// List closed sessions with topics
    pub async fn list_session_history(
        &self,
        agent_id: &str,
    ) -> Result<Vec<SessionHistoryEntry>, SessionManagerError>;
}
```

Topic generation in `close_session`:
1. Get last N messages (e.g. 20)
2. If < 2 messages, skip LLM, topic = None
3. Prompt: `"用一句简短的中文概括以下对话的主题（10字以内）：\n{messages}"`
4. Single LLM call → topic string
5. Generate topic embedding
6. Store topic + embedding + status="closed" in metadata JSON

Epoch query: `SELECT key FROM sessions WHERE key LIKE ? ORDER BY created_at DESC LIMIT 1`

### 6. Memory Retrieval Integration

**SearchFilter extension**:
```rust
pub struct SearchFilter {
    pub namespace: Option<NamespaceScope>,
    pub workspace: Option<String>,
    pub session_ids: Option<Vec<String>>,  // new: filter by session
    // ... existing fields
}
```

**Two-phase retrieval** in `MemoryContextProvider`:

```
Phase 1: Find related sessions
  - Get all closed sessions with topics for current agent
  - Compare query embedding with each topic_embedding
  - Select sessions above similarity threshold

Phase 2: Filtered memory search
  - Add current session_id + related session_ids to SearchFilter
  - LanceDB query: session_id IN (...) + vector_search
  - Boost scores for memories from related sessions
```

### 7. RPC Interface

New endpoint:
```
sessions.new → close current, create new epoch, return both keys
```

Extended `sessions.list` response:
```json
{
  "sessions": [
    {
      "key": "agent:main:dm:user123:s2",
      "agent_id": "main",
      "session_type": "peer",
      "message_count": 15,
      "topic": "Rust 并发测试",
      "status": "closed",
      "created_at": "...",
      "last_active_at": "..."
    }
  ]
}
```

### 8. Panel UI Adaptation

`SessionEntry` in `chat_sidebar.rs` adds `topic: Option<String>` for display.
Session list shows topic as subtitle when available.

---

## Decisions Record

| Question | Decision |
|----------|----------|
| `/new` interception layer | Gateway (unified, not per-channel) |
| Topic generation timing | Immediate on session close |
| Memory filtering approach | Retrieval-time, two-phase (session similarity → filtered search) |
| Agent switch behavior | Triggers session close (same as `/new`) |
| Old session handling | Keep as-is, new session uses new key |
| topic_id cleanup | Full replace to session_id (technical debt removal) |
| Approach | SessionKey epoch encoding (Option A) |

## Impact

- **SessionKey**: add epoch field + serialize/parse changes
- **SessionManager**: 3 new methods
- **ContextAnchor**: rename topic_id → session_id
- **~40 files**: mechanical topic_id → session_id rename
- **LanceDB schema**: column rename (migration)
- **SQLite state_database**: column rename (migration)
- **inbound_router.rs**: `/new` handler + `/switch` enhancement
- **SearchFilter**: new session_ids field
- **MemoryContextProvider**: two-phase retrieval logic
- **Panel UI**: SessionEntry topic display
