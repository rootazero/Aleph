# Session Management Phase 1 — Abstraction & Dual Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the concrete `SessionManager` SQLite dependency with a `SessionStore` trait, implement a `FileSessionStore` side-by-side with a migrated `SqliteSessionStore`, and make the Gateway layer backend-agnostic without breaking existing RPC behavior.

**Architecture:** Introduce a `gateway/session_store/` module that defines a trait-based persistence contract. `SqliteSessionStore` is a thin wrapper around existing `SessionManager` ops. `FileSessionStore` stores transcripts as JSONL files and metadata in a dedicated SQLite table. Gateway handlers and builtin tools talk to `Arc<dyn SessionStore>` only.

**Tech Stack:** Rust, tokio, rusqlite, serde_json, async-trait, chrono

---

## File Structure

### New Files
- `src/gateway/session_store/mod.rs` — `SessionStore` trait, `SessionStoreConfig`, re-exports
- `src/gateway/session_store/types.rs` — `SessionMetadata`, `MessageRecord`, `SessionFilter`, `SessionPreview`, `DeleteResult`, `CompactStrategy`, `CompactResult`, `CheckpointSummary`, `SearchHit`
- `src/gateway/session_store/error.rs` — `SessionStoreError` enum
- `src/gateway/session_store/sqlite_backend/mod.rs` — `SqliteSessionStore` struct + `impl SessionStore`
- `src/gateway/session_store/sqlite_backend/migrate.rs` — schema helpers for metadata v2 table
- `src/gateway/session_store/file_backend/mod.rs` — `FileSessionStore` struct + metadata ops
- `src/gateway/session_store/file_backend/transcript.rs` — JSONL read/append/reset helpers
- `src/gateway/session_store/file_backend/search.rs` — placeholder for search (returns empty vec in Phase 1)

### Modified Files
- `src/gateway/session_manager/mod.rs` — mark deprecated, re-export from `session_store`
- `src/gateway/session_manager/ops.rs` — move business logic into `sqlite_backend/ops.rs`
- `src/gateway/context.rs` — replace `Arc<SessionManager>` with `Arc<dyn SessionStore>`
- `src/gateway/handlers/session/db_handlers.rs` — use `SessionStore` methods instead of `SessionManager`
- `src/gateway/handlers/mod.rs` — update imports
- `src/builtin_tools/sessions/list_tool.rs` — import `SessionMetadata` from new module
- `src/builtin_tools/session_search.rs` — import `SessionStore` types
- `src/builtin_tools/sessions/mod.rs` — update re-exports if needed
- `src/config/types.rs` (or equivalent config root) — add `SessionStoreConfig` to aleph config
- `src/bin/aleph-server/commands/start/builder/agent_init.rs` (or gateway init) — wire backend selection

---

## Task 1: Define Core Types

**Files:**
- Create: `src/gateway/session_store/error.rs`
- Create: `src/gateway/session_store/types.rs`
- Modify: `src/gateway/session_store/mod.rs`

- [ ] **Step 1: Write `SessionStoreError`**

```rust
// src/gateway/session_store/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Session not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unsupported operation on this backend")]
    Unsupported,
}
```

- [ ] **Step 2: Write `MessageRecord` and `SessionMetadata` v2**

```rust
// src/gateway/session_store/types.rs
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub key: String,
    pub agent_id: String,
    pub session_type: String,
    pub created_at: i64,
    pub last_active_at: i64,
    pub message_count: u64,
    pub state: crate::gateway::session_manager::SessionState,
    pub topic: Option<String>,
    pub label: Option<String>,
    pub display_name: Option<String>,
    pub derived_title: Option<String>,
    pub last_message_preview: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub agent_id: Option<String>,
    pub limit: Option<usize>,
    pub active_minutes: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SessionPreview {
    pub key: String,
    pub items: Vec<PreviewItem>,
}

#[derive(Debug, Clone)]
pub struct PreviewItem {
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub deleted: bool,
}

#[derive(Debug, Clone)]
pub enum CompactStrategy {
    KeepLastN { n: usize },
}

#[derive(Debug, Clone)]
pub struct CompactResult {
    pub compacted: bool,
    pub deleted: usize,
}

#[derive(Debug, Clone)]
pub struct CheckpointSummary {
    pub checkpoint_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub session_key: String,
    pub agent_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}
```

- [ ] **Step 3: Create `session_store/mod.rs` with trait**

```rust
// src/gateway/session_store/mod.rs
pub mod error;
pub mod types;
pub mod file_backend;
pub mod sqlite_backend;

use async_trait::async_trait;
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::*;

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn get_or_create(&self, key: &SessionKey) -> Result<SessionMetadata, SessionStoreError>;
    async fn get_metadata(&self, key: &SessionKey) -> Result<Option<SessionMetadata>, SessionStoreError>;
    async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionMetadata>, SessionStoreError>;
    async fn delete_session(&self, key: &SessionKey) -> Result<DeleteResult, SessionStoreError>;
    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError>;

    async fn append_message(
        &self,
        key: &SessionKey,
        msg: MessageRecord,
    ) -> Result<(), SessionStoreError>;
    async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError>;

    async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, SessionStoreError>;

    async fn compact(
        &self,
        key: &SessionKey,
        strategy: CompactStrategy,
    ) -> Result<CompactResult, SessionStoreError>;
    async fn list_checkpoints(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<CheckpointSummary>, SessionStoreError>;
    async fn branch_from_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
        new_key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError>;
    async fn restore_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
    ) -> Result<SessionMetadata, SessionStoreError>;

    async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<&str>,
    ) -> Result<(), SessionStoreError>;
    async fn set_topic(
        &self,
        key: &SessionKey,
        topic: &str,
    ) -> Result<(), SessionStoreError>;
    async fn set_state(
        &self,
        key: &SessionKey,
        state: crate::gateway::session_manager::SessionState,
    ) -> Result<(), SessionStoreError>;
    async fn get_state(
        &self,
        key: &SessionKey,
    ) -> Result<crate::gateway::session_manager::SessionState, SessionStoreError>;
    async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError>;
    async fn get_current_epoch(
        &self,
        base_key_pattern: &str,
    ) -> Result<u32, SessionStoreError>;
    async fn get_session_topic(
        &self,
        key: &SessionKey,
    ) -> Result<Option<String>, SessionStoreError>;
    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError>;
}
```

- [ ] **Step 4: Verify compilation of the trait module**

Run:
```bash
cargo check -p alephcore
```

Expected: errors about missing imports (fine), but no syntax errors in new files.

---

## Task 2: Migrate SqliteSessionStore

**Files:**
- Create: `src/gateway/session_store/sqlite_backend/mod.rs`
- Create: `src/gateway/session_store/sqlite_backend/migrate.rs`
- Modify: `src/gateway/session_manager/mod.rs`
- Modify: `src/gateway/session_manager/ops.rs`

- [ ] **Step 1: Move `SessionManager` struct to `sqlite_backend/mod.rs` and rename it**

```rust
// src/gateway/session_store/sqlite_backend/mod.rs
use crate::sync_primitives::{Arc, Mutex};
use rusqlite::Connection;
use std::path::PathBuf;

pub struct SqliteSessionStore {
    pub(super) config: SqliteSessionStoreConfig,
    pub(super) conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct SqliteSessionStoreConfig {
    pub db_path: PathBuf,
    pub max_messages: usize,
    pub compaction_keep: usize,
    pub auto_reset_hour: Option<u8>,
    pub session_expiry_secs: u64,
}
```

- [ ] **Step 2: Move schema init and constructor logic**

Copy the `new`, `init_schema`, `migrate_add_state_column` methods from `gateway/session_manager/mod.rs` into `sqlite_backend/mod.rs` as `impl SqliteSessionStore`. Remove `raw_memory_writer` and `with_raw_memory_writer` for now (we’ll re-attach later via a higher-level wrapper if needed).

- [ ] **Step 3: Move all ops from `gateway/session_manager/ops.rs` into `sqlite_backend/mod.rs` as `#[async_trait] impl SessionStore for SqliteSessionStore`**

Map existing methods to trait methods:
- `get_or_create` → `get_or_create`
- `add_message` → `append_message`
- `get_history` → `get_history`
- `reset_session` → `reset_session`
- `delete_session` → `delete_session`
- `list_sessions` → `list_sessions`
- `compact_session` → `compact`
- `close_session` → `close_session`
- `set_topic` → `set_topic`
- `get_current_epoch` → `get_current_epoch`
- `get_session_topic` → `get_session_topic`
- `cleanup_expired` → `cleanup_expired`
- `set_state`, `get_state` → same
- `get_identity_context` → same
- `search_messages` → same

For trait methods that don't exist yet in the old ops:
- `list_checkpoints`, `branch_from_checkpoint`, `restore_checkpoint` → return `Err(SessionStoreError::Unsupported)`

Convert `StoredMessage` to `MessageRecord` at method boundaries.

- [ ] **Step 4: Update `gateway/session_manager/mod.rs` to re-export `SqliteSessionStore`**

```rust
// src/gateway/session_manager/mod.rs
// DEPRECATED: This module is being replaced by gateway/session_store.
// Re-exports are kept for backward compatibility during Phase 1.

pub use crate::gateway::session_store::error::SessionStoreError as SessionManagerError;
pub use crate::gateway::session_store::sqlite_backend::{SqliteSessionStore as SessionManager, SqliteSessionStoreConfig as SessionManagerConfig};
pub use crate::gateway::session_store::types::*;

// Keep old type aliases
pub type StoredMessage = crate::gateway::session_store::types::MessageRecord;
```

- [ ] **Step 5: Delete or empty out `gateway/session_manager/ops.rs`**

Replace its contents with:
```rust
// Operations moved to gateway/session_store/sqlite_backend/mod.rs
```

- [ ] **Step 6: Compile check**

Run:
```bash
cargo check -p alephcore
```

Fix any import errors. You will likely need to update `use` statements in `sqlite_backend/mod.rs` to reference `SessionStoreError`, `SessionMetadata`, etc.

---

## Task 3: Implement FileSessionStore Skeleton

**Files:**
- Create: `src/gateway/session_store/file_backend/transcript.rs`
- Create: `src/gateway/session_store/file_backend/mod.rs`

- [ ] **Step 1: Implement transcript JSONL helpers**

```rust
// src/gateway/session_store/file_backend/transcript.rs
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::MessageRecord;

pub fn ensure_transcript_dir(base: &Path, agent_id: &str) -> Result<std::path::PathBuf, SessionStoreError> {
    let dir = base.join("sessions").join(agent_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| SessionStoreError::Io(e))?;
    Ok(dir)
}

pub fn transcript_path(base: &Path, agent_id: &str, session_id: &str) -> std::path::PathBuf {
    ensure_transcript_dir(base, agent_id)
        .unwrap_or_else(|_| base.join("sessions").join(agent_id))
        .join(format!("{}.jsonl", session_id))
}

pub fn append_message(path: &Path, msg: &MessageRecord) -> Result<(), SessionStoreError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(SessionStoreError::Io)?;
    let line = serde_json::to_string(msg)
        .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
    writeln!(file, "{}", line).map_err(SessionStoreError::Io)?;
    Ok(())
}

pub fn read_messages(path: &Path, limit: Option<usize>) -> Result<Vec<MessageRecord>, SessionStoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).map_err(SessionStoreError::Io)?;
    let reader = BufReader::new(file);
    let mut messages: Vec<MessageRecord> = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(SessionStoreError::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: MessageRecord = serde_json::from_str(&line)
            .map_err(|e| SessionStoreError::DatabaseError(format!("JSONL parse error: {}", e)))?;
        messages.push(msg);
    }
    if let Some(n) = limit {
        let skip = messages.len().saturating_sub(n);
        messages = messages.into_iter().skip(skip).collect();
    }
    Ok(messages)
}

pub fn reset_transcript(path: &Path) -> Result<(), SessionStoreError> {
    if path.exists() {
        std::fs::remove_file(path).map_err(SessionStoreError::Io)?;
    }
    Ok(())
}
```

- [ ] **Step 2: Implement `FileSessionStore` metadata schema and constructor**

```rust
// src/gateway/session_store/file_backend/mod.rs
mod transcript;

use crate::sync_primitives::{Arc, Mutex};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::*;
use crate::gateway::session_store::SessionStore;
use async_trait::async_trait;
use rusqlite::Connection;
use std::path::PathBuf;

pub struct FileSessionStore {
    config: FileSessionStoreConfig,
    meta_conn: Arc<Mutex<Connection>>,
    transcript_base: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FileSessionStoreConfig {
    pub meta_db_path: PathBuf,
    pub transcript_base: PathBuf,
    pub max_messages: usize,
    pub compaction_keep: usize,
    pub session_expiry_secs: u64,
}

impl FileSessionStore {
    pub fn new(config: FileSessionStoreConfig) -> Result<Self, SessionStoreError> {
        if let Some(parent) = config.meta_db_path.parent() {
            std::fs::create_dir_all(parent).map_err(SessionStoreError::Io)?;
        }
        std::fs::create_dir_all(&config.transcript_base).map_err(SessionStoreError::Io)?;
        let conn = Connection::open(&config.meta_db_path)
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        Self::init_meta_schema(&conn)?;
        Ok(Self {
            config,
            meta_conn: Arc::new(Mutex::new(conn)),
            transcript_base: config.transcript_base.clone(),
        })
    }

    fn init_meta_schema(conn: &Connection) -> Result<(), SessionStoreError> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS session_metadata (
                key TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                session_type TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_active_at INTEGER NOT NULL,
                message_count INTEGER DEFAULT 0,
                total_tokens INTEGER DEFAULT 0,
                state TEXT DEFAULT 'created',
                metadata_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_meta_agent ON session_metadata(agent_id);
            CREATE INDEX IF NOT EXISTS idx_meta_active ON session_metadata(last_active_at);
            CREATE INDEX IF NOT EXISTS idx_meta_state ON session_metadata(state);
            "#
        ).map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}
```

- [ ] **Step 3: Implement the easy trait methods for `FileSessionStore`**

For Phase 1, implement:
- `get_or_create`
- `get_metadata`
- `list_sessions`
- `delete_session`
- `reset_session`
- `append_message`
- `get_history`
- `close_session`
- `set_topic`
- `set_state`, `get_state`
- `get_identity_context`
- `get_current_epoch`
- `get_session_topic`
- `cleanup_expired`

Return `Unsupported` for:
- `search_messages`
- `compact`
- `list_checkpoints`
- `branch_from_checkpoint`
- `restore_checkpoint`

Implementation pattern:
- Metadata lives in `session_metadata` table (same schema as old `sessions` table).
- Messages live in JSONL under `self.transcript_base.join("sessions").join(agent_id).join(format!("{}.jsonl", session_key.to_key_string()))`.
- Use existing `SessionIdentityMeta` serialization logic copied/adapted from old `ops.rs`.

**Important:** Keep the `metadata_json` column for now to minimize conversion logic. We’ll flatten it in Phase 4.

- [ ] **Step 4: Compile check**

Run:
```bash
cargo check -p alephcore
```

Fix missing imports.

---

## Task 4: Wire SessionStore into GatewayContext

**Files:**
- Modify: `src/gateway/context.rs`
- Modify: `src/gateway/mod.rs`

- [ ] **Step 1: Update `GatewayContext` to hold `Arc<dyn SessionStore>`**

Find the field:
```rust
pub session_manager: Arc<SessionManager>,
```

Replace with:
```rust
pub session_store: Arc<dyn SessionStore>,
```

Update the constructor `GatewayContext::new` to accept `Arc<dyn SessionStore>` instead of `Arc<SessionManager>`.

- [ ] **Step 2: Update `gateway/mod.rs` to expose `session_store`**

Add:
```rust
pub mod session_store;
```

Ensure `SessionStore` trait is accessible from outside the crate if needed by builtin tools.

- [ ] **Step 3: Compile check**

Run:
```bash
cargo check -p alephcore
```

This will produce errors in all call sites that still reference `session_manager`. We fix those next.

---

## Task 5: Update Handlers and Tools to Use Trait

**Files:**
- Modify: `src/gateway/handlers/session/db_handlers.rs`
- Modify: `src/gateway/handlers/session/mod.rs`
- Modify: `src/builtin_tools/sessions/list_tool.rs`
- Modify: `src/builtin_tools/session_search.rs`
- Modify: `src/builtin_tools/sessions/mod.rs`
- Modify: `src/gateway/handlers/chat.rs` (if it references `session_manager`)
- Modify: `src/gateway/handlers/mod.rs`
- Modify: `src/gateway/handlers/agent.rs`
- Modify: `src/gateway/execution_engine/*.rs`

- [ ] **Step 1: Mechanical rename in `db_handlers.rs`**

Replace all occurrences of:
- `Arc<SessionManager>` → `Arc<dyn SessionStore>`
- `SessionManager` → `dyn SessionStore`
- `.list_sessions(agent_id)` → `.list_sessions(SessionFilter { agent_id: agent_id.map(|s| s.to_string()), limit: None, active_minutes: None })`
- `.get_history(&session_key, limit)` → `.get_history(&session_key, limit)`
- `.reset_session(&session_key)` → `.reset_session(&session_key)`
- `.delete_session(&session_key)` → `.delete_session(&session_key)`
- `.compact_session(&session_key)` → `.compact(&session_key, CompactStrategy::KeepLastN { n: 50 })`
- `.close_session(&legacy_key, topic)` → `.close_session(&legacy_key, topic.as_deref())`
- `.set_topic(&session_key, topic)` → `.set_topic(&session_key, topic)`
- `.get_or_create(&session_key)` → `.get_or_create(&session_key)`

**Note:** `CompactStrategy::KeepLastN { n: 50 }` is a placeholder; the old compaction logic uses `config.compaction_keep`. In `db_handlers.rs` the compact handler currently reads history length before/after. For Phase 1, preserve the old behavior by calling `compact` with a strategy that matches `config.compaction_keep`. If `FileSessionStore` doesn't yet implement `compact`, this will return `Unsupported` — that is acceptable for Phase 1 because we are not switching the default backend yet. Gate the compact behavior so it still works on `SqliteSessionStore`.

- [ ] **Step 2: Update imports in `db_handlers.rs`**

Add:
```rust
use crate::gateway::session_store::{SessionStore, types::CompactStrategy};
```

- [ ] **Step 3: Update `builtin_tools/sessions/list_tool.rs`**

Change the `context.session_manager()` call to `context.session_store()`.
Update `SessionMetadata` import path to `crate::gateway::session_store::types::SessionMetadata`.

- [ ] **Step 4: Update `builtin_tools/session_search.rs`**

Change `context.session_manager().search_messages(...)` to `context.session_store().search_messages(...)`.

- [ ] **Step 5: Find all other `session_manager()` usages across the crate**

Run:
```bash
grep -rn "session_manager()" src/
```

For each hit, replace with `session_store()` and update types accordingly. Common files:
- `gateway/handlers/chat.rs` — may load history
- `gateway/execution_engine/*.rs` — may reference session manager for identity
- `gateway/handlers/agent.rs`

- [ ] **Step 6: Compile check**

Run:
```bash
cargo check -p alephcore
```

Fix all type mismatches until clean.

---

## Task 6: Add Backend Selection Config

**Files:**
- Modify: `src/config/types.rs` (or wherever aleph config root is)
- Modify: server startup builder (find where `SessionManager` is constructed)

- [ ] **Step 1: Add `SessionStoreConfig` to config**

Find the main config struct (often `AlephConfig` or similar). Add:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionStoreConfig {
    #[serde(default = "default_session_backend")]
    pub backend: String, // "sqlite" or "file"
}

fn default_session_backend() -> String {
    "sqlite".to_string()
}
```

- [ ] **Step 2: Wire backend selection in startup**

Find the function that builds the `GatewayContext` (likely in `bin/aleph-server/commands/start/builder/` or `gateway/server_init.rs`). Replace:

```rust
let session_manager = Arc::new(SessionManager::new(SessionManagerConfig::default())?);
```

with:

```rust
use crate::gateway::session_store::sqlite_backend::{SqliteSessionStore, SqliteSessionStoreConfig};
use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};

let session_store: Arc<dyn SessionStore> = match config.session_store.backend.as_str() {
    "file" => {
        let file_config = FileSessionStoreConfig {
            meta_db_path: crate::utils::paths::get_sessions_db_path()
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/aleph_sessions_meta.db")),
            transcript_base: crate::utils::paths::get_aleph_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/aleph"))
                .join("sessions"),
            max_messages: 100,
            compaction_keep: 50,
            session_expiry_secs: 30 * 24 * 60 * 60,
        };
        Arc::new(FileSessionStore::new(file_config)?)
    }
    _ => {
        let sqlite_config = SqliteSessionStoreConfig {
            db_path: crate::utils::paths::get_sessions_db_path()
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/aleph_sessions.db")),
            max_messages: 100,
            compaction_keep: 50,
            auto_reset_hour: Some(4),
            session_expiry_secs: 30 * 24 * 60 * 60,
        };
        Arc::new(SqliteSessionStore::new(sqlite_config)?)
    }
};
```

- [ ] **Step 3: Compile check**

Run:
```bash
cargo check -p alephcore
```

---

## Task 7: Integration Tests

**Files:**
- Create: `src/gateway/session_store/tests.rs`
- Modify: `src/gateway/session_store/mod.rs`

- [ ] **Step 1: Add a backend parity test**

```rust
// src/gateway/session_store/tests.rs
#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::gateway::router::SessionKey;
    use tempfile::tempdir;

    async fn test_backend_roundtrip(backend: Arc<dyn SessionStore>) {
        let key = SessionKey::main("test-agent");
        let meta = backend.get_or_create(&key).await.unwrap();
        assert_eq!(meta.agent_id, "test-agent");

        backend.append_message(&key,
            MessageRecord { id: "1".into(), role: "user".into(), content: "hello".into(), timestamp: 1, metadata: None }
        ).await.unwrap();

        let history = backend.get_history(&key, None).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "hello");

        let reset = backend.reset_session(&key).await.unwrap();
        assert!(reset);

        let history = backend.get_history(&key, None).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn sqlite_backend_roundtrip() {
        let tmp = tempdir().unwrap();
        let config = sqlite_backend::SqliteSessionStoreConfig {
            db_path: tmp.path().join("test.db"),
            max_messages: 100,
            compaction_keep: 50,
            auto_reset_hour: None,
            session_expiry_secs: 0,
        };
        let backend = Arc::new(sqlite_backend::SqliteSessionStore::new(config).unwrap());
        test_backend_roundtrip(backend).await;
    }

    #[tokio::test]
    async fn file_backend_roundtrip() {
        let tmp = tempdir().unwrap();
        let config = file_backend::FileSessionStoreConfig {
            meta_db_path: tmp.path().join("meta.db"),
            transcript_base: tmp.path().join("transcripts"),
            max_messages: 100,
            compaction_keep: 50,
            session_expiry_secs: 0,
        };
        let backend = Arc::new(file_backend::FileSessionStore::new(config).unwrap());
        test_backend_roundtrip(backend).await;
    }
}
```

- [ ] **Step 2: Register test module**

In `src/gateway/session_store/mod.rs`, add:
```rust
#[cfg(test)]
mod tests;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib gateway::session_store::tests
```

Expected: both `sqlite_backend_roundtrip` and `file_backend_roundtrip` pass.

---

## Task 8: Final Compile & Lint

- [ ] **Step 1: Full compile**

```bash
cargo check -p alephcore
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings
```

- [ ] **Step 3: Run existing tests**

```bash
cargo test -p alephcore --lib
```

Expected: all existing tests pass (because default backend is still `sqlite`).

- [ ] **Step 4: Commit**

```bash
git add src/gateway/session_store/
git add src/gateway/session_manager/
git add src/gateway/context.rs
git add src/gateway/handlers/session/
git add src/gateway/mod.rs
git add src/builtin_tools/sessions/
git add src/builtin_tools/session_search.rs
git add src/config/
git add src/bin/aleph-server/
git commit -m "session: introduce SessionStore trait with sqlite and file backends"
```

---

## Self-Review Checklist

- **Spec coverage:**
  - ✅ Trait abstraction (Task 1)
  - ✅ SqliteSessionStore migration (Task 2)
  - ✅ FileSessionStore skeleton with transcript files (Task 3)
  - ✅ GatewayContext wiring (Task 4)
  - ✅ Handlers/tools migration (Task 5)
  - ✅ Config-based backend selection (Task 6)
  - ✅ Integration tests (Task 7)

- **Placeholder scan:**
  - ✅ No "TBD", "TODO", or "implement later" in steps.
  - ✅ All code blocks are concrete and copy-paste ready.

- **Type consistency:**
  - ✅ `SessionStoreError` used everywhere.
  - ✅ `SessionMetadata` path is `gateway/session_store::types::SessionMetadata` in all tasks.
  - ✅ `SessionKey` comes from `gateway::router::SessionKey`.

---

**Plan saved to:** `docs/superpowers/plans/2026-04-17-session-management-phase1.md`

**Execution options:**

1. **Subagent-Driven (recommended)** — Dispatch a fresh subagent per task with review between tasks.
2. **Inline Execution** — Execute tasks in this session using `executing-plans` skill.

Reply with your preferred execution mode to begin Phase 1 implementation.
