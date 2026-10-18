# Markdown-First Storage Architecture Design

> **Status**: Draft  
> **Date**: 2026-10-18  
> **Author**: Architecture Design Phase 2  
> **Related**: [Storage Survey](../investigation/2026-10-18-storage-architecture-survey.md)

---

## 1. Overview

### 1.1 Goals

**Primary**:
- Raw memories (sessions, tool outputs, attachments) stored as Markdown files
- SQLite only for IDs, indexes, and derived data
- Git-manageable: readable diffs, no binary blobs, portable `~/.aleph`
- Cross-agent compatibility: standard Markdown + Obsidian wikilinks

**Non-goals**:
- ❌ Backward compatibility with existing `raw_memories` table
- ❌ Migration tooling (manual data migration)
- ❌ Supporting non-filesystem backends

### 1.2 Design Principles

1. **Markdown is the source of truth** — SQLite can be rebuilt from `.md` files
2. **Relative paths for portability** — entire `~/.aleph` is relocatable
3. **Content-addressed attachments** — hash-based naming prevents duplicates
4. **Atomic writes** — write-temp-then-rename for crash safety
5. **Fail-closed IDs** — every entity has a unique ID in `metadata.db`

---

## 2. Target Directory Structure

```
~/.aleph/
├── metadata.db              ← SQLite: IDs, indexes, metadata only
│
├── memory/
│   └── note/{agent}/{category}/*.md  ← Knowledge notes (unchanged)
│
├── raw/                     ← NEW: Raw memories in Markdown
│   ├── sessions/
│   │   └── {session_id}/
│   │       ├── transcript.md          ← Conversation
│   │       ├── tool_outputs.md        ← Tool execution results
│   │       └── metadata.json          ← Structured session data
│   │
│   └── attachments/         ← NEW: Content-addressed attachments
│       ├── images/
│       │   └── {sha256_prefix}.{ext}  ← e.g., a3f5b2c8d1e4f7a9.png
│       ├── documents/
│       │   └── {sha256_prefix}.{ext}
│       └── videos/
│           └── {sha256_prefix}.{ext}
│
└── archive/                 ← NEW: Archived old sessions
    └── {year}/
        └── {month}/
            └── sessions/...
```

### 2.1 Session Directory Layout

**Example**: Session `sess_2026-10-18_abc123`

```
~/.aleph/raw/sessions/sess_2026-10-18_abc123/
├── transcript.md           ← Main conversation log
├── tool_outputs.md         ← Aggregated tool results
└── metadata.json           ← Structured metadata
```

#### `transcript.md`

```markdown
---
session_id: sess_2026-10-18_abc123
agent_id: main
workspace_id: ws_default
started_at: 2026-10-18T18:05:35Z
ended_at: 2026-10-18T19:23:12Z
tags: [rust, memory, refactor]
---

# Session: 2026-10-18 Main Agent

## Summary
Discussed and designed Markdown-first storage architecture for Aleph memory system.

---

## Transcript

### [18:05:35] User
我想把Aleph记忆管理中raw存储从sqlite改为md原始文档存储...

### [18:05:42] Assistant
明白了。让我先看看参考项目的记忆管理实现...

### [18:10:15] User
同意，现在作阶段一调研...

### [18:10:20] Assistant  
好的，我现在对两个参考项目和 Aleph 当前的记忆系统有了足够了解...

---

## Attachments

- ![Architecture Diagram](../../attachments/images/a3f5b2c8d1e4f7a9.png)
- [Design Doc](../../attachments/documents/e9f1a2b3c4d5e6f7.pdf)
```

#### `tool_outputs.md`

```markdown
---
session_id: sess_2026-10-18_abc123
---

# Tool Execution Log

## [18:06:10] bash
```bash
sqlite3 ~/Library/Application\ Support/aleph/aleph.db '.tables'
```

**Exit Code**: 0  
**Duration**: 120ms

**Output**:
```
(no output)
```

---

## [18:06:15] read
**File**: `/Volumes/TBU4/Workspace/Aleph/src/memory/store/raw_memory.rs`  
**Lines**: 1-150

**Content**:
```rust
use crate::error::AlephError;
...
```

---

## [18:12:30] ctx_batch_execute
**Commands**: 8  
**Duration**: 3.2s

**Results**: See [detailed log](./tool_outputs/ctx_batch_20261018_181230.md)
```

#### `metadata.json`

```json
{
  "version": 1,
  "session_id": "sess_2026-10-18_abc123",
  "agent_id": "main",
  "workspace_id": "ws_default",
  "started_at": "2026-10-18T18:05:35Z",
  "ended_at": "2026-10-18T19:23:12Z",
  "duration_seconds": 4657,
  "transcript_path": "raw/sessions/sess_2026-10-18_abc123/transcript.md",
  "tool_outputs_path": "raw/sessions/sess_2026-10-18_abc123/tool_outputs.md",
  "attachments": [
    {
      "attachment_id": "att_abc123",
      "type": "image",
      "mime_type": "image/png",
      "file_path": "raw/attachments/images/a3f5b2c8d1e4f7a9.png",
      "content_hash": "sha256:a3f5b2c8d1e4f7a9...",
      "size_bytes": 245760,
      "uploaded_at": "2026-10-18T18:15:30Z"
    }
  ],
  "tags": ["rust", "memory", "refactor"],
  "message_count": 42,
  "tool_call_count": 15,
  "compression_status": "pending"
}
```

### 2.2 Attachment Naming Strategy

**Content-addressed**: `{sha256_first_16_hex}.{ext}`

**Why**:
- ✅ Deduplication: identical files share one copy
- ✅ Integrity: filename = content hash
- ✅ Safe: no name collisions
- ✅ Relocatable: no absolute paths

**Example**:
```
Original: screenshot-2026-10-18.png
Stored:   raw/attachments/images/a3f5b2c8d1e4f7a9.png
Hash:     sha256:a3f5b2c8d1e4f7a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3
```

**Reference in Markdown**:
```markdown
![Screenshot](../../attachments/images/a3f5b2c8d1e4f7a9.png)
```

---

## 3. SQLite Schema (metadata.db)

### 3.1 Core Principles

- **IDs only**: user, agent, session, workspace, attachment IDs
- **Paths to Markdown**: every session/note points to its `.md` file
- **Indexes**: FTS5, vector, links for fast queries
- **Derived data**: statistics, caches (rebuildable)

### 3.2 Table Definitions

#### `sessions`

```sql
CREATE TABLE sessions (
    session_id       TEXT PRIMARY KEY,
    agent_id         TEXT NOT NULL,
    workspace_id     TEXT,
    transcript_path  TEXT NOT NULL,   -- relative: raw/sessions/{id}/transcript.md
    started_at       INTEGER NOT NULL,
    ended_at         INTEGER,
    duration_seconds INTEGER,
    message_count    INTEGER DEFAULT 0,
    tool_call_count  INTEGER DEFAULT 0,
    content_hash     TEXT,             -- sha256 of transcript.md for change detection
    compression_status TEXT DEFAULT 'pending',  -- 'pending', 'compressed', 'archived'
    tags_json        TEXT DEFAULT '[]',
    FOREIGN KEY (agent_id) REFERENCES agents(agent_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id)
);

CREATE INDEX idx_sessions_agent ON sessions(agent_id, started_at DESC);
CREATE INDEX idx_sessions_workspace ON sessions(workspace_id);
CREATE INDEX idx_sessions_compression ON sessions(compression_status);
```

#### `agents`

```sql
CREATE TABLE agents (
    agent_id    TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    persona     TEXT,
    created_at  INTEGER NOT NULL,
    last_active INTEGER
);
```

#### `workspaces`

```sql
CREATE TABLE workspaces (
    workspace_id TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    path         TEXT NOT NULL,  -- filesystem path
    created_at   INTEGER NOT NULL
);
```

#### `attachments`

```sql
CREATE TABLE attachments (
    attachment_id TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL,
    file_path     TEXT NOT NULL,    -- relative: raw/attachments/{type}/{hash}.{ext}
    content_hash  TEXT NOT NULL,    -- sha256 full hash
    mime_type     TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL,
    uploaded_at   INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX idx_attachments_hash ON attachments(content_hash);
CREATE INDEX idx_attachments_session ON attachments(session_id);
```

#### `notes_index` (unchanged)

```sql
CREATE TABLE notes_index (
    path            TEXT NOT NULL,
    filename        TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    category        TEXT NOT NULL,
    tags_json       TEXT DEFAULT '[]',
    aliases_json    TEXT DEFAULT '[]',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_accessed_at INTEGER,
    content_hash    TEXT NOT NULL,
    PRIMARY KEY (agent_id, path)
);
```

#### `notes_fts` (unchanged)

```sql
CREATE VIRTUAL TABLE notes_fts USING fts5(
    agent_id,
    path,
    filename,
    content,
    tokenize = 'porter unicode61'
);
```

#### `notes_vec_{dim}` (unchanged)

```sql
-- Per-dimension vector tables (e.g., notes_vec_1024)
CREATE VIRTUAL TABLE notes_vec_1024 USING vec0(
    embedding FLOAT[1024]
);

CREATE TABLE notes_vec_1024_meta (
    rowid       INTEGER PRIMARY KEY,
    agent_id    TEXT NOT NULL,
    note_path   TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    UNIQUE(agent_id, note_path, chunk_index)
);
```

### 3.3 Removed Tables

**Deleted** (no longer needed):
- ❌ `raw_memories` — replaced by `raw/sessions/` Markdown files
- ❌ `compression_metadata` — merged into `sessions.compression_status`

**Kept** (unchanged):
- ✅ `notes_links`, `notes_sources`, `notes_graph_cache`
- ✅ `recall_signals`, `dream_reports`, `dream_status`, `daily_insights`

---

## 4. Rust API Design

### 4.1 Trait: `RawSessionStore`

**New trait** replacing `RawMemoryStore`:

```rust
use std::path::{Path, PathBuf};
use async_trait::async_trait;

#[async_trait]
pub trait RawSessionStore: Send + Sync {
    /// Create a new session and its Markdown files
    async fn create_session(&self, session: SessionMetadata) -> Result<(), AlephError>;
    
    /// Append a message to the transcript
    async fn append_transcript(
        &self,
        session_id: &str,
        timestamp: i64,
        role: MessageRole,
        content: &str,
    ) -> Result<(), AlephError>;
    
    /// Append a tool output
    async fn append_tool_output(
        &self,
        session_id: &str,
        timestamp: i64,
        tool_name: &str,
        args: &str,
        result: &ToolResult,
    ) -> Result<(), AlephError>;
    
    /// Store an attachment and return its file path
    async fn store_attachment(
        &self,
        session_id: &str,
        attachment: Attachment,
    ) -> Result<AttachmentRef, AlephError>;
    
    /// Finalize session (update end time, compute stats)
    async fn finalize_session(
        &self,
        session_id: &str,
        ended_at: i64,
    ) -> Result<(), AlephError>;
    
    /// Read transcript content
    async fn read_transcript(&self, session_id: &str) -> Result<String, AlephError>;
    
    /// List sessions for an agent
    async fn list_sessions(
        &self,
        agent_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SessionMetadata>, AlephError>;
}
```

### 4.2 Data Structures

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub agent_id: String,
    pub workspace_id: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub transcript_path: PathBuf,   // relative
    pub tool_outputs_path: PathBuf, // relative
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub original_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub attachment_id: String,
    pub file_path: PathBuf,      // relative
    pub content_hash: String,    // sha256 hex
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}
```

### 4.3 Implementation: `MarkdownSessionStore`

```rust
pub struct MarkdownSessionStore {
    base_dir: PathBuf,              // ~/.aleph
    conn: Arc<Mutex<Connection>>,   // metadata.db
}

impl MarkdownSessionStore {
    pub fn new(base_dir: &Path) -> Result<Self, AlephError> {
        let db_path = base_dir.join("metadata.db");
        let conn = Connection::open(&db_path)?;
        
        // Initialize schema
        schema::init_metadata_schema(&conn)?;
        
        // Ensure directories exist
        fs::create_dir_all(base_dir.join("raw/sessions"))?;
        fs::create_dir_all(base_dir.join("raw/attachments/images"))?;
        fs::create_dir_all(base_dir.join("raw/attachments/documents"))?;
        fs::create_dir_all(base_dir.join("raw/attachments/videos"))?;
        
        Ok(Self {
            base_dir: base_dir.to_path_buf(),
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    
    /// Atomic write: write to temp file, then rename
    async fn atomic_write_markdown(
        &self,
        path: &Path,
        content: &str,
    ) -> Result<(), AlephError> {
        let temp_path = path.with_extension("tmp");
        tokio::fs::write(&temp_path, content).await?;
        tokio::fs::rename(&temp_path, path).await?;
        Ok(())
    }
    
    /// Append to Markdown file (with file lock)
    async fn append_markdown(
        &self,
        path: &Path,
        section: &str,
    ) -> Result<(), AlephError> {
        // TODO: Use file lock (flock/LockFile)
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .await?;
        
        use tokio::io::AsyncWriteExt;
        file.write_all(section.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        
        Ok(())
    }
}
```

### 4.4 Key Operations

#### Create Session

```rust
#[async_trait]
impl RawSessionStore for MarkdownSessionStore {
    async fn create_session(&self, session: SessionMetadata) -> Result<(), AlephError> {
        let session_dir = self.base_dir
            .join("raw/sessions")
            .join(&session.session_id);
        
        tokio::fs::create_dir_all(&session_dir).await?;
        
        // Create transcript.md
        let transcript_content = format!(
            "---\n\
             session_id: {}\n\
             agent_id: {}\n\
             workspace_id: {}\n\
             started_at: {}\n\
             tags: {:?}\n\
             ---\n\n\
             # Session: {} {}\n\n\
             ## Transcript\n\n",
            session.session_id,
            session.agent_id,
            session.workspace_id.as_deref().unwrap_or("none"),
            session.started_at,
            session.tags,
            format_timestamp(session.started_at),
            session.agent_id,
        );
        
        self.atomic_write_markdown(
            &session_dir.join("transcript.md"),
            &transcript_content,
        ).await?;
        
        // Create tool_outputs.md
        let tool_outputs_content = format!(
            "---\n\
             session_id: {}\n\
             ---\n\n\
             # Tool Execution Log\n\n",
            session.session_id,
        );
        
        self.atomic_write_markdown(
            &session_dir.join("tool_outputs.md"),
            &tool_outputs_content,
        ).await?;
        
        // Create metadata.json
        let metadata = serde_json::json!({
            "version": 1,
            "session_id": session.session_id,
            "agent_id": session.agent_id,
            "workspace_id": session.workspace_id,
            "started_at": format_timestamp(session.started_at),
            "transcript_path": session.transcript_path,
            "tool_outputs_path": session.tool_outputs_path,
            "tags": session.tags,
        });
        
        self.atomic_write_markdown(
            &session_dir.join("metadata.json"),
            &serde_json::to_string_pretty(&metadata)?,
        ).await?;
        
        // Insert into SQLite
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO sessions (session_id, agent_id, workspace_id, transcript_path, started_at, tags_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session.session_id,
                session.agent_id,
                session.workspace_id,
                session.transcript_path.display().to_string(),
                session.started_at,
                serde_json::to_string(&session.tags)?,
            ],
        )?;
        
        Ok(())
    }
}
```

#### Append Transcript

```rust
async fn append_transcript(
    &self,
    session_id: &str,
    timestamp: i64,
    role: MessageRole,
    content: &str,
) -> Result<(), AlephError> {
    let transcript_path = self.base_dir
        .join("raw/sessions")
        .join(session_id)
        .join("transcript.md");
    
    let role_str = match role {
        MessageRole::User => "User",
        MessageRole::Assistant => "Assistant",
        MessageRole::System => "System",
    };
    
    let section = format!(
        "### [{}] {}\n{}\n\n",
        format_timestamp(timestamp),
        role_str,
        content,
    );
    
    self.append_markdown(&transcript_path, &section).await?;
    
    // Update message count in SQLite
    let conn = self.conn.lock().await;
    conn.execute(
        "UPDATE sessions SET message_count = message_count + 1 WHERE session_id = ?",
        params![session_id],
    )?;
    
    Ok(())
}
```

#### Store Attachment

```rust
async fn store_attachment(
    &self,
    session_id: &str,
    attachment: Attachment,
) -> Result<AttachmentRef, AlephError> {
    use sha2::{Sha256, Digest};
    
    // Compute hash
    let mut hasher = Sha256::new();
    hasher.update(&attachment.data);
    let hash = hasher.finalize();
    let hash_hex = format!("{:x}", hash);
    let hash_prefix = &hash_hex[..16];
    
    // Determine type directory
    let type_dir = if attachment.mime_type.starts_with("image/") {
        "images"
    } else if attachment.mime_type.starts_with("video/") {
        "videos"
    } else {
        "documents"
    };
    
    // Determine extension
    let ext = attachment.original_name
        .as_ref()
        .and_then(|n| Path::new(n).extension())
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    
    let filename = format!("{}.{}", hash_prefix, ext);
    let rel_path = PathBuf::from(format!("raw/attachments/{}/{}", type_dir, filename));
    let abs_path = self.base_dir.join(&rel_path);
    
    // Check if already exists (deduplication)
    if !abs_path.exists() {
        tokio::fs::write(&abs_path, &attachment.data).await?;
    }
    
    // Insert into SQLite
    let attachment_id = format!("att_{}", nanoid::nanoid!(12));
    let conn = self.conn.lock().await;
    conn.execute(
        "INSERT INTO attachments (attachment_id, session_id, file_path, content_hash, mime_type, size_bytes, uploaded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            attachment_id,
            session_id,
            rel_path.display().to_string(),
            hash_hex,
            attachment.mime_type,
            attachment.data.len() as i64,
            chrono::Utc::now().timestamp(),
        ],
    )?;
    
    Ok(AttachmentRef {
        attachment_id,
        file_path: rel_path,
        content_hash: hash_hex,
        size_bytes: attachment.data.len() as u64,
    })
}
```

---

## 5. Integration Points

### 5.1 Gateway: Session Lifecycle

```rust
// src/gateway/session/lifecycle.rs

pub struct SessionLifecycle {
    session_store: Arc<dyn RawSessionStore>,
    session_id: String,
}

impl SessionLifecycle {
    pub async fn start(&self, agent_id: &str, workspace_id: Option<&str>) -> Result<(), AlephError> {
        let metadata = SessionMetadata {
            session_id: self.session_id.clone(),
            agent_id: agent_id.to_string(),
            workspace_id: workspace_id.map(String::from),
            started_at: chrono::Utc::now().timestamp(),
            ended_at: None,
            transcript_path: PathBuf::from(format!("raw/sessions/{}/transcript.md", self.session_id)),
            tool_outputs_path: PathBuf::from(format!("raw/sessions/{}/tool_outputs.md", self.session_id)),
            tags: vec![],
        };
        
        self.session_store.create_session(metadata).await
    }
    
    pub async fn log_message(&self, role: MessageRole, content: &str) -> Result<(), AlephError> {
        self.session_store.append_transcript(
            &self.session_id,
            chrono::Utc::now().timestamp(),
            role,
            content,
        ).await
    }
    
    pub async fn log_tool_output(&self, tool_name: &str, args: &str, result: ToolResult) -> Result<(), AlephError> {
        self.session_store.append_tool_output(
            &self.session_id,
            chrono::Utc::now().timestamp(),
            tool_name,
            args,
            &result,
        ).await
    }
    
    pub async fn end(&self) -> Result<(), AlephError> {
        self.session_store.finalize_session(
            &self.session_id,
            chrono::Utc::now().timestamp(),
        ).await
    }
}
```

### 5.2 Compression Service

```rust
// src/memory/compression/service.rs

impl CompressionService {
    pub async fn process_pending_sessions(&self) -> Result<(), AlephError> {
        // 1. Query uncompressed sessions from SQLite
        let sessions = self.session_store.list_sessions("main", 100, 0).await?;
        let pending: Vec<_> = sessions.into_iter()
            .filter(|s| s.compression_status == "pending")
            .collect();
        
        for session in pending {
            // 2. Read transcript
            let transcript = self.session_store.read_transcript(&session.session_id).await?;
            
            // 3. Ask LLM to extract knowledge
            let facts = self.extract_facts(&transcript).await?;
            
            // 4. Write to memory/note/{agent}/{category}/*.md
            for fact in facts {
                self.note_store.create_note(&fact).await?;
            }
            
            // 5. Mark as compressed
            self.session_store.update_compression_status(&session.session_id, "compressed").await?;
        }
        
        Ok(())
    }
}
```

### 5.3 Attachment Upload

```rust
// src/gateway/handlers/attachment.rs

pub async fn handle_attachment_upload(
    session_id: String,
    file: MultipartFile,
    session_store: Arc<dyn RawSessionStore>,
) -> Result<AttachmentRef, AlephError> {
    let data = file.bytes().await?;
    let mime_type = file.content_type().unwrap_or("application/octet-stream").to_string();
    let original_name = file.file_name().map(String::from);
    
    let attachment = Attachment {
        data: data.to_vec(),
        mime_type,
        original_name,
    };
    
    let att_ref = session_store.store_attachment(&session_id, attachment).await?;
    
    // Return relative path for embedding in transcript
    Ok(att_ref)
}
```

---

## 6. File Locking Strategy

### 6.1 Problem

Concurrent appends to `transcript.md` from multiple threads/processes can corrupt the file.

### 6.2 Solution

Use **advisory file locks** (flock on Unix, LockFile on Windows):

```rust
use std::fs::OpenOptions;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

pub struct LockedFile {
    file: File,
}

impl LockedFile {
    pub fn open_append(path: &Path) -> Result<Self, AlephError> {
        #[cfg(unix)]
        {
            use nix::fcntl::{flock, FlockArg};
            let file = OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)?;
            flock(file.as_raw_fd(), FlockArg::LockExclusive)?;
            Ok(Self { file })
        }
        
        #[cfg(windows)]
        {
            use windows::Win32::Storage::FileSystem::LockFile;
            let file = OpenOptions::new()
                .append(true)
                .create(true)
                .share_mode(0) // exclusive
                .open(path)?;
            // LockFile implementation...
            Ok(Self { file })
        }
    }
    
    pub fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf)
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        // Unlock automatically on drop
    }
}
```

### 6.3 Usage

```rust
async fn append_markdown(&self, path: &Path, content: &str) -> Result<(), AlephError> {
    tokio::task::spawn_blocking({
        let path = path.to_path_buf();
        let content = content.to_string();
        move || {
            let mut file = LockedFile::open_append(&path)?;
            file.write_all(content.as_bytes())?;
            file.write_all(b"\n")?;
            Ok::<_, AlephError>(())
        }
    }).await??;
    
    Ok(())
}
```

---

## 7. Archive Strategy

### 7.1 When to Archive

- Sessions older than 90 days (configurable)
- Sessions explicitly marked for archive
- Low-activity agents (< 10 recalls in 30 days)

### 7.2 Archive Process

```rust
pub async fn archive_old_sessions(&self, cutoff_days: u32) -> Result<usize, AlephError> {
    let cutoff = chrono::Utc::now().timestamp() - (cutoff_days as i64 * 86400);
    
    let conn = self.conn.lock().await;
    let sessions: Vec<String> = conn.prepare(
        "SELECT session_id FROM sessions 
         WHERE ended_at < ? AND compression_status = 'compressed'"
    )?
    .query_map(params![cutoff], |row| row.get(0))?
    .collect::<Result<_, _>>()?;
    
    for session_id in &sessions {
        // Move files
        let src = self.base_dir.join("raw/sessions").join(session_id);
        let year = extract_year(session_id);
        let month = extract_month(session_id);
        let dest = self.base_dir.join(format!("archive/{}/{}/sessions/{}", year, month, session_id));
        
        tokio::fs::create_dir_all(dest.parent().unwrap()).await?;
        tokio::fs::rename(&src, &dest).await?;
        
        // Update SQLite
        conn.execute(
            "UPDATE sessions SET compression_status = 'archived', 
             transcript_path = ? WHERE session_id = ?",
            params![
                dest.join("transcript.md").strip_prefix(&self.base_dir).unwrap().display().to_string(),
                session_id,
            ],
        )?;
    }
    
    Ok(sessions.len())
}
```

---

## 8. Testing Strategy

### 8.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_create_session() {
        let temp = TempDir::new().unwrap();
        let store = MarkdownSessionStore::new(temp.path()).unwrap();
        
        let metadata = SessionMetadata {
            session_id: "test_session".to_string(),
            agent_id: "test_agent".to_string(),
            workspace_id: None,
            started_at: chrono::Utc::now().timestamp(),
            ended_at: None,
            transcript_path: PathBuf::from("raw/sessions/test_session/transcript.md"),
            tool_outputs_path: PathBuf::from("raw/sessions/test_session/tool_outputs.md"),
            tags: vec![],
        };
        
        store.create_session(metadata).await.unwrap();
        
        // Verify files exist
        assert!(temp.path().join("raw/sessions/test_session/transcript.md").exists());
        assert!(temp.path().join("raw/sessions/test_session/tool_outputs.md").exists());
        assert!(temp.path().join("raw/sessions/test_session/metadata.json").exists());
        
        // Verify SQLite entry
        let sessions = store.list_sessions("test_agent", 10, 0).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "test_session");
    }
    
    #[tokio::test]
    async fn test_append_transcript() {
        let temp = TempDir::new().unwrap();
        let store = MarkdownSessionStore::new(temp.path()).unwrap();
        
        // Create session first
        let metadata = /* ... */;
        store.create_session(metadata).await.unwrap();
        
        // Append messages
        store.append_transcript("test_session", 100, MessageRole::User, "Hello").await.unwrap();
        store.append_transcript("test_session", 105, MessageRole::Assistant, "Hi!").await.unwrap();
        
        // Read transcript
        let content = store.read_transcript("test_session").await.unwrap();
        assert!(content.contains("### [") && content.contains("] User\nHello"));
        assert!(content.contains("### [") && content.contains("] Assistant\nHi!"));
    }
    
    #[tokio::test]
    async fn test_attachment_deduplication() {
        let temp = TempDir::new().unwrap();
        let store = MarkdownSessionStore::new(temp.path()).unwrap();
        
        let attachment = Attachment {
            data: b"test data".to_vec(),
            mime_type: "text/plain".to_string(),
            original_name: Some("test.txt".to_string()),
        };
        
        // Upload twice
        let ref1 = store.store_attachment("sess1", attachment.clone()).await.unwrap();
        let ref2 = store.store_attachment("sess2", attachment.clone()).await.unwrap();
        
        // Should have same hash
        assert_eq!(ref1.content_hash, ref2.content_hash);
        
        // File should exist only once
        let file_count = std::fs::read_dir(temp.path().join("raw/attachments/documents"))
            .unwrap()
            .count();
        assert_eq!(file_count, 1);
    }
}
```

### 8.2 Integration Tests

```rust
#[tokio::test]
async fn test_full_session_lifecycle() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(MarkdownSessionStore::new(temp.path()).unwrap());
    
    let lifecycle = SessionLifecycle {
        session_store: store.clone(),
        session_id: "integration_test".to_string(),
    };
    
    // Start session
    lifecycle.start("test_agent", None).await.unwrap();
    
    // Log messages
    lifecycle.log_message(MessageRole::User, "帮我实现一个函数").await.unwrap();
    lifecycle.log_message(MessageRole::Assistant, "好的").await.unwrap();
    
    // Log tool output
    lifecycle.log_tool_output(
        "bash",
        "ls -la",
        ToolResult {
            exit_code: Some(0),
            stdout: "total 0\n".to_string(),
            stderr: String::new(),
            duration_ms: 50,
        },
    ).await.unwrap();
    
    // End session
    lifecycle.end().await.unwrap();
    
    // Verify transcript
    let transcript = store.read_transcript("integration_test").await.unwrap();
    assert!(transcript.contains("帮我实现一个函数"));
    assert!(transcript.contains("好的"));
    
    // Verify tool_outputs.md
    let tool_outputs_path = temp.path().join("raw/sessions/integration_test/tool_outputs.md");
    let tool_outputs = std::fs::read_to_string(tool_outputs_path).unwrap();
    assert!(tool_outputs.contains("bash"));
    assert!(tool_outputs.contains("ls -la"));
    assert!(tool_outputs.contains("total 0"));
}
```

---

## 9. Performance Considerations

### 9.1 Write Performance

| Operation | Current (SQLite) | Target (Markdown) | Trade-off |
|-----------|------------------|-------------------|-----------|
| Append message | 0.5-2ms | 1-3ms | Slightly slower (file I/O + lock) |
| Batch append | 5-10ms | 10-20ms | More predictable (no transaction overhead) |
| Store attachment | 2-5ms | 5-10ms | Deduplication saves space |

**Optimization**:
- Buffer writes in memory, flush every 5s or 10 messages
- Use async I/O (tokio::fs)
- Keep file handle open during session

### 9.2 Read Performance

| Operation | Current (SQLite) | Target (Markdown) | Trade-off |
|-----------|------------------|-------------------|-----------|
| Read full transcript | 1-5ms | 5-20ms | Slower (file read vs indexed query) |
| Search across sessions | 10-50ms | 100-500ms | Much slower without FTS5 |
| List recent sessions | 1-3ms | 1-3ms | Same (metadata in SQLite) |

**Mitigation**:
- Keep metadata in SQLite for listing/filtering
- Use FTS5 index for cross-session search
- Cache recently accessed transcripts

### 9.3 Disk Space

| Data Type | SQLite | Markdown + Attachments | Savings |
|-----------|--------|------------------------|---------|
| 1 year sessions | ~180MB | ~180MB | None (text is text) |
| Attachments | BLOB in DB | Content-addressed files | Deduplication saves 20-40% |
| Indexes | ~50MB | ~50MB | Same (still in SQLite) |

**Git Repo Size**:
- `.md` files: efficient (text diffs)
- Attachments: use `.gitattributes` + git-lfs
- Archive old sessions outside repo

---

## 10. Migration Path (Out of Scope)

**Note**: Migration is manual, but for reference:

```bash
# Export existing raw_memories to Markdown
cargo run --bin aleph-export-sessions -- \
  --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/.aleph/raw/sessions/

# Rebuild indexes
cargo run --bin aleph-rebuild-indexes -- \
  --base-dir ~/.aleph
```

---

## 11. Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| **File corruption** from concurrent writes | High | File locking (flock/LockFile) |
| **Large file count** (100K+ sessions) | Medium | Hierarchical directories (`2026/10/18/`) |
| **Slow search** without indexed content | High | Keep FTS5 index in SQLite |
| **Git repo bloat** from binary attachments | Medium | Use git-lfs or archive attachments |
| **Attachment deduplication failure** | Low | Hash-based naming prevents duplicates |

---

## 12. Future Enhancements (Deferred)

1. **Streaming writes**: WebSocket-based live transcript streaming
2. **Compression**: gzip old transcripts (`.md.gz`)
3. **Encryption**: Encrypt sensitive sessions at rest
4. **Cloud sync**: Push `~/.aleph` to S3/Git LFS
5. **Multi-user**: Separate `~/.aleph/{user_id}/` directories

---

## 13. Acceptance Criteria

### 13.1 Functional

- ✅ Create session writes `transcript.md`, `tool_outputs.md`, `metadata.json`
- ✅ Append message/tool output appends to correct file
- ✅ Store attachment writes content-addressed file + SQLite entry
- ✅ Finalize session updates `ended_at` and compression status
- ✅ Read transcript returns full Markdown content
- ✅ List sessions queries SQLite metadata

### 13.2 Non-Functional

- ✅ Concurrent appends don't corrupt files (file locking)
- ✅ Attachments deduplicated by content hash
- ✅ Entire `~/.aleph` relocatable (no absolute paths)
- ✅ Git diff shows readable changes
- ✅ Performance acceptable (< 20ms per operation)

### 13.3 Testing

- ✅ Unit tests cover all `RawSessionStore` methods
- ✅ Integration test runs full session lifecycle
- ✅ Stress test: 1000 concurrent appends
- ✅ Deduplication test: same attachment uploaded 10 times

---

## 14. Implementation Plan

### Phase A: Core Infrastructure (3-5 days)

1. Define `RawSessionStore` trait
2. Implement `MarkdownSessionStore`
3. File locking utilities
4. Attachment storage + deduplication
5. Unit tests

### Phase B: Gateway Integration (2-3 days)

1. Update `SessionLifecycle` to use `RawSessionStore`
2. Hook message logging
3. Hook tool output logging
4. Attachment upload handler

### Phase C: Compression Service (2-3 days)

1. Read from `sessions` table
2. Load `transcript.md`
3. Extract facts via LLM
4. Write to `memory/note/`
5. Mark session as compressed

### Phase D: Testing & Polish (2-3 days)

1. Integration tests
2. Performance benchmarks
3. Documentation
4. Code review

**Total**: 9-14 days

---

## 15. Related Documents

- [Storage Survey](../investigation/2026-10-18-storage-architecture-survey.md)
- [MEMORY_SYSTEM.md](../reference/MEMORY_SYSTEM.md)
- [REDLINES.md](../reference/REDLINES.md)
- [Pi Memory README](~/.pi/agent/npm/node_modules/pi-memory/README.md)

---

**Design Status**: Ready for Implementation  
**Next Step**: Begin Phase A — Core Infrastructure
