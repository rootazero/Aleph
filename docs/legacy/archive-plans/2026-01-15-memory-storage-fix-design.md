# Memory Storage Fix Design

**Date**: 2026-01-15
**Status**: Approved
**Author**: Claude (Brainstorming Session)

## Problem Statement

1. Memory management cannot store conversations in both single-turn and multi-turn modes
2. Need to maintain multi-turn conversation topic system integrity
3. Clearing memory database should also clear topic system content
4. Deleting a topic should also delete associated memories in the database

## Root Cause Analysis

The memory ingestion pipeline exists but is **never connected** to the main request processing flow:

| Component | Status | Issue |
|-----------|--------|-------|
| `MemoryIngestion` | Fully implemented | Never instantiated or called |
| `store_memory()` | Complete implementation | Never invoked after response |
| `RigAgentManager` | Has memory_store field | Always initialized as `None` |
| `on_memory_stored` | Callback defined | Never triggered |

## Solution Architecture

### Data Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        Swift UI Layer                           │
├─────────────────────────────────────────────────────────────────┤
│  Single-turn:                                                   │
│    process(input, options: ProcessOptions(topicId: nil))       │
│                                                                 │
│  Multi-turn:                                                    │
│    process(input, options: ProcessOptions(topicId: topic.id))  │
│                                                                 │
│  Delete operations:                                             │
│    clearAllMemories()           → Clear memories + topics       │
│    deleteMemoriesByTopic(id)    → Delete topic memories         │
│    deleteTopic(id)              → Delete topic + call above API │
└──────────────────────────┬──────────────────────────────────────┘
                           │ UniFFI
┌──────────────────────────▼──────────────────────────────────────┐
│                        Rust Core                                │
├─────────────────────────────────────────────────────────────────┤
│  After process() completion:                                    │
│    1. Create ContextAnchor (app, window, topic_id)             │
│    2. Call MemoryIngestion.store_memory()                      │
│    3. Trigger on_memory_stored() callback                      │
│                                                                 │
│  New APIs:                                                      │
│    clear_all_memories() → DELETE FROM memories                 │
│    delete_memories_by_topic(id) → DELETE WHERE topic_id = ?    │
└─────────────────────────────────────────────────────────────────┘
```

## Implementation Details

### 1. ProcessOptions Extension

**File**: `Aleph/core/src/uniffi_core.rs`

```rust
pub struct ProcessOptions {
    pub app_context: Option<String>,
    pub window_title: Option<String>,
    pub topic_id: Option<String>,        // NEW
    pub stream: Option<bool>,
    pub attachments: Option<Vec<MediaAttachment>>,
}
```

### 2. Memory Storage Integration

**File**: `Aleph/core/src/uniffi_core.rs` (process function)

```rust
match result {
    Ok(response) => {
        // === NEW: Store memory ===
        let topic_id = options.as_ref()
            .and_then(|o| o.topic_id.clone())
            .unwrap_or_else(|| "single-turn".to_string());

        let context = ContextAnchor::with_topic(
            app_context.unwrap_or_default(),
            window_title.unwrap_or_default(),
            topic_id,
        );

        if let Err(e) = memory_ingestion.store_memory(
            context, &input, &response.content
        ).await {
            warn!("Memory storage failed: {}", e);
        } else {
            handler.on_memory_stored();
        }
        // === END NEW ===

        handler.on_complete(response.content);
    }
}
```

### 3. New Delete APIs

**File**: `Aleph/core/src/aleph.udl`

```
interface AlephCore {
    // Existing methods...

    // New delete APIs
    [Throws=AlephFfiError]
    void clear_all_memories();

    [Throws=AlephFfiError]
    void delete_memories_by_topic(string topic_id);
};
```

### 4. VectorDatabase Delete Implementation

**File**: `Aleph/core/src/memory/database.rs`

```rust
impl VectorDatabase {
    pub fn delete_by_topic_id(&self, topic_id: &str) -> Result<usize, AlephError> {
        let conn = self.conn.lock()?;

        // Get memory IDs for fact cleanup
        let memory_ids: Vec<String> = conn
            .prepare("SELECT id FROM memories WHERE topic_id = ?")?
            .query_map([topic_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;

        // Delete memories
        let deleted = conn.execute(
            "DELETE FROM memories WHERE topic_id = ?",
            [topic_id]
        )?;

        // Invalidate associated facts (soft delete)
        if !memory_ids.is_empty() {
            self.invalidate_facts_by_source(&conn, &memory_ids)?;
        }

        Ok(deleted)
    }

    pub fn clear_all(&self) -> Result<usize, AlephError> {
        let conn = self.conn.lock()?;

        conn.execute("DELETE FROM memory_facts", [])?;
        conn.execute("DELETE FROM compression_sessions", [])?;
        let deleted = conn.execute("DELETE FROM memories", [])?;

        Ok(deleted)
    }
}
```

### 5. Swift Integration

**File**: `Aleph/Sources/Store/ConversationStore.swift`

```swift
func deleteTopic(id: String) throws {
    try dbQueue.write { db in
        try Topic.filter(Column("id") == id).updateAll(db, Column("isDeleted").set(to: true))
    }
    try alephCore.deleteMemoriesByTopic(topicId: id)
}

func clearAllData() throws {
    try dbQueue.write { db in
        try Topic.deleteAll(db)
        try ConversationMessage.deleteAll(db)
    }
    try alephCore.clearAllMemories()
}
```

## Files to Modify

| File | Change Type | Description |
|------|-------------|-------------|
| `core/src/uniffi_core.rs` | Modify | Add topic_id to ProcessOptions, integrate memory storage, add delete APIs |
| `core/src/aleph.udl` | Modify | Add topic_id field and delete API declarations |
| `core/src/memory/database.rs` | Modify | Add delete_by_topic_id() and clear_all() |
| `core/src/memory/context.rs` | Verify | Confirm with_topic() method exists |
| `Sources/Generated/aleph.swift` | Auto-gen | Regenerate with UniFFI |
| `Sources/Coordinators/MultiTurnCoordinator.swift` | Modify | Pass topic_id |
| `Sources/Store/ConversationStore.swift` | Modify | Integrate delete API calls |
| `Sources/Views/SettingsView.swift` | Modify | Clear button calls clearAllData() |

## Test Verification Points

1. **Single-turn storage**: Check `memories` table has records with `topic_id = "single-turn"`
2. **Multi-turn storage**: Verify record's `topic_id` matches Swift layer `topic.id`
3. **Topic deletion**: Associated memories deleted, other topics unaffected
4. **Full clear**: All tables (`memories`, `memory_facts`, `topics`, `messages`) empty

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| Pass topic_id via ProcessOptions | Stateless design, minimal changes, matches existing pattern |
| Rust provides unified delete APIs | Single source of truth, data consistency |
| Soft delete for facts | Preserve audit trail, mark `is_valid = 0` |
| Cascade cleanup | Delete memories also invalidates associated facts |
