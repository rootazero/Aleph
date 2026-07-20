# Session Management Redesign Design

> Status: Draft  
> Date: 2026-04-17  
> Author: AI Agent (Sisyphus)  
> Scope: alephcore session subsystem, Gateway RPC, builtin tools  

---

## 1. Background & Goals

### 1.1 Why Now

Aleph’s current session management (`gateway/session_manager`) is built on a single SQLite database that mixes **metadata** and **transcript messages** in one schema:

- `sessions` table — metadata (key, agent_id, state, message_count, total_tokens, topic, etc.)
- `messages` table — full transcript content
- `messages_fts` virtual table — FTS5 full-text search

This design worked for the initial MVP, but as the project matures it shows several structural weaknesses compared to the reference implementation in OpenClaw:

1. **Monolithic storage** — all agent messages live in one DB file, creating a write bottleneck and making large-session compaction expensive.
2. **Destructive compaction** — old messages are permanently deleted; there is no way to branch or restore a pre-compaction state.
3. **Poor frontend ergonomics** — listing sessions requires scanning the DB and, if previews are needed, fetching full history rows. There is no lightweight `preview` or derived-title mechanism.
4. **No real-time sync** — multiple clients (Webchat, Tauri, macOS) have no pub/sub channel for session-list updates.
5. **Shallow metadata** — token costs, model references, compaction checkpoints, delivery context, and subagent relationships are either absent or stored as ad-hoc JSON blobs.

### 1.2 Goals

| # | Goal | Success Criteria |
|---|---|---|
| G1 | **Pluggable storage backend** | Upper layers (Gateway, Tools) depend only on a `SessionStore` trait, not SQLite specifics. |
| G2 | **Per-session file transcript** | Each session gets a standalone JSONL file for messages, enabling fast append, cheap preview, and external tooling. |
| G3 | **Checkpoint-based compaction** | Compaction creates a recoverable checkpoint; users can `branch` or `restore` to any checkpoint. |
| G4 | **Real-time session sync** | Gateway emits `sessions.changed` events; clients can `subscribe`/`unsubscribe`. |
| G5 | **Derived titles & previews** | Session list can return `derived_title` (from first user message) and `last_message_preview` without loading full history. |
| G6 | **Zero-downtime migration** | Old SQLite messages can be exported to JSONL automatically; rollback to SQLite-only is possible via config. |
| G7 | **Clean removal of legacy code** | After cutover, old `messages` table code, memory-only handlers, and duplicated `SessionKey` types are deleted. |

---

## 2. Current State Analysis

### 2.1 Aleph Today

```
gateway/session_manager/
├── mod.rs          # SessionManager struct, config, schema init
├── ops.rs          # CRUD, compaction, cleanup, search
└── tests.rs        # Unit tests

gateway/handlers/session/
├── mod.rs
├── db_handlers.rs  # RPC: list, history, reset, delete, compact, set_topic
└── store.rs        # In-memory handlers (legacy dev path)

builtin_tools/sessions/
├── list_tool.rs
├── send_tool.rs
├── spawn_tool.rs
├── new_tool.rs
├── set_topic_tool.rs
└── helpers.rs

builtin_tools/session_search.rs   # FTS5 cross-session search
builtin_tools/session_complete.rs # Close session

routing/session_key.rs            # Enhanced SessionKey (Main, DM, Group, Task, Subagent, Ephemeral)
gateway/router.rs                 # Legacy SessionKey (kept for compat)
```

**Notable patterns to keep**

- `SessionKey` hierarchy in `routing/session_key.rs` is well-designed and type-safe.
- `SessionIdentityMeta` → `IdentityContext` flow is clean and security-critical; must remain untouched.
- `ExecutionSession` in `components/types/session.rs` tracks in-memory agent-loop state; it is orthogonal to persistent session storage.

### 2.2 OpenClaw Reference (What to Learn From)

OpenClaw uses a **split-model** architecture:

- **Metadata store** — JSON file (`sessions.json`) mapping `sessionKey → SessionEntry`
- **Transcript store** — per-session JSONL (`{sessionId}.jsonl`) with newline-delimited message events
- **Checkpoints** — pre/post compaction transcript snapshots referenced from `SessionEntry.compactionCheckpoints`
- **Gateway API** — `sessions.list`, `sessions.preview`, `sessions.subscribe`, `sessions.changed`, `sessions.compaction.branch/restore`
- **Derived UX** — `derivedTitle` extracted from first user message in transcript; `lastMessagePreview` from tail read

### 2.3 Gap Matrix

| Capability | Aleph | OpenClaw | Priority |
|---|---|---|---|
| Metadata → structured `SessionEntry` | Partial (JSON blob) | Full | P0 |
| Per-session transcript file | No | Yes | P0 |
| Compaction checkpoint / branch / restore | No | Yes | P0 |
| Real-time session list sync | No | Yes | P1 |
| Lightweight preview / derived title | No | Yes | P1 |
| Usage tracking (cost, tokens, model) | Partial | Full | P1 |
| Archive on delete | No | Yes | P2 |
| Subagent / parent session linkage | Partial | Full | P2 |

---

## 3. Design Principles

1. **Trait-first (R1 alignment)**  
   Core defines `SessionStore` trait contracts; concrete backends implement them. Gateway and tools never call SQLite directly.

2. **Append-only transcripts**  
   Messages are appended to JSONL; compaction rewrites a new file rather than deleting in-place rows.

3. **Metadata index stays small**  
   SQLite (or any fast KV) keeps only metadata + search index; heavy transcript I/O happens on per-session files.

4. **Backward compatibility**  
   Old RPC shapes remain valid; new fields are additive. A config flag controls which backend is active.

5. **YAGNI for archive**  
   Archive on delete is desirable but not blocking; we’ll leave a hook in the trait and implement it in Phase 4 if bandwidth allows.

---

## 4. High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           INTERFACE LAYER                                │
│  (Webchat / Tauri / macOS / CLI / TUI)                                  │
└─────────────────────────────────┬───────────────────────────────────────┘
                                  │ JSON-RPC / WebSocket
┌─────────────────────────────────┴───────────────────────────────────────┐
│                           GATEWAY LAYER                                  │
│  handlers/session/*.rs  ──►  SessionStore trait (dyn or generic)         │
│  ├─ sessions.list                                          │
│  ├─ sessions.preview                                       │
│  ├─ sessions.subscribe  ──►  EventBus (sessions.changed)   │
│  ├─ sessions.compact   ──►  Checkpoint + transcript rewrite│
│  ├─ sessions.reset / delete                                │
│  └─ sessions.send / spawn                                  │
└─────────────────────────────────┬───────────────────────────────────────┘
                                  │
              ┌───────────────────┴───────────────────┐
              ▼                                       ▼
    ┌─────────────────────┐                 ┌─────────────────────┐
    │   FileSessionStore  │                 │  SqliteSessionStore │
    │   (new, default)    │                 │  (legacy, compat)   │
    └──────────┬──────────┘                 └──────────┬──────────┘
               │                                       │
    ┌──────────┴──────────┐                  ┌────────┴────────┐
    │  JSONL Transcripts  │                  │  messages table │
    │  (per session)      │                  │  (legacy)       │
    └──────────┬──────────┘                  └─────────────────┘
               │
    ┌──────────┴──────────┐
    │  SQLite Metadata    │
    │  (sessions table v2)│
    └─────────────────────┘
```

### 4.1 Key Files (Target Layout)

```
src/gateway/session_store/
├── mod.rs              # SessionStore trait, re-exports
├── types.rs            # SessionEntry, Checkpoint, MessageRecord, etc.
├── error.rs            # SessionStoreError
├── file_backend/       # FileSessionStore implementation
│   ├── mod.rs
│   ├── transcript.rs   # JSONL read/append/rewrite
│   ├── checkpoint.rs   # Checkpoint create/branch/restore
│   └── search.rs       # Optional: ripgrep or simple scan for FTS
├── sqlite_backend/     # SqliteSessionStore (legacy compat)
│   ├── mod.rs
│   └── ops.rs
└── migration.rs        # Export legacy SQLite messages → JSONL

gateway/session_manager/  # Existing module — to be refactored into facade
```

---

## 5. Core Trait Design

```rust
/// The unified contract for session persistence.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Metadata ops
    async fn get_or_create(&self, key: &SessionKey) -> Result<SessionMetadata, SessionStoreError>;
    async fn get_metadata(&self, key: &SessionKey) -> Result<Option<SessionMetadata>, SessionStoreError>;
    async fn patch_metadata(&self, key: &SessionKey, patch: MetadataPatch) -> Result<SessionMetadata, SessionStoreError>;
    async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionMetadata>, SessionStoreError>;
    async fn delete_session(&self, key: &SessionKey) -> Result<DeleteResult, SessionStoreError>;

    /// Transcript ops
    async fn append_message(&self, key: &SessionKey, msg: MessageRecord) -> Result<(), SessionStoreError>;
    async fn get_history(&self, key: &SessionKey, limit: Option<usize>) -> Result<Vec<MessageRecord>, SessionStoreError>;
    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError>;

    /// Preview / derived title (optional optimization — default impl falls back to get_history)
    async fn preview(&self, key: &SessionKey, limit: usize, max_chars: usize) -> Result<SessionPreview, SessionStoreError>;

    /// Search (default impl can scan; FileBackend may use ripgrep or index)
    async fn search_messages(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, SessionStoreError>;

    /// Compaction & checkpoints
    async fn compact(&self, key: &SessionKey, strategy: CompactStrategy) -> Result<CompactResult, SessionStoreError>;
    async fn list_checkpoints(&self, key: &SessionKey) -> Result<Vec<CheckpointSummary>, SessionStoreError>;
    async fn branch_from_checkpoint(&self, key: &SessionKey, checkpoint_id: &str, new_key: &SessionKey) -> Result<SessionMetadata, SessionStoreError>;
    async fn restore_checkpoint(&self, key: &SessionKey, checkpoint_id: &str) -> Result<SessionMetadata, SessionStoreError>;

    /// Lifecycle hooks
    async fn close_session(&self, key: &SessionKey, topic: Option<&str>) -> Result<(), SessionStoreError>;
}
```

**Notes**

- `SessionMetadata` will be expanded to include `model`, `provider`, `input_tokens`, `output_tokens`, `estimated_cost_usd`, `compaction_checkpoints`, `parent_session_key`, `child_sessions`, `derived_title`, `last_message_preview`, `status`, `started_at`, `ended_at`, `runtime_ms`.
- `MessageRecord` replaces `StoredMessage` with a stricter schema: `id`, `role`, `content`, `timestamp`, `metadata`.
- Default implementations on the trait keep backend-specific code minimal.

---

## 6. Data Models

### 6.1 SessionMetadata (v2)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub key: String,
    pub agent_id: String,
    pub session_type: String,          // "main", "dm", "group", "task", "subagent", "ephemeral"
    pub created_at: i64,
    pub last_active_at: i64,
    pub state: SessionState,

    // --- new fields ---
    pub topic: Option<String>,
    pub label: Option<String>,
    pub display_name: Option<String>,
    pub derived_title: Option<String>,
    pub last_message_preview: Option<String>,

    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub total_tokens_fresh: bool,
    pub estimated_cost_usd: Option<f64>,
    pub context_tokens: Option<u64>,

    pub status: Option<SessionRunStatus>, // idle | running | completed | error
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub runtime_ms: Option<u64>,

    pub parent_session_key: Option<String>,
    pub spawned_by: Option<String>,
    pub spawn_depth: u32,
    pub subagent_role: Option<String>,

    pub compaction_checkpoints: Vec<CompactionCheckpoint>,

    // Identity (preserved from old SessionIdentityMeta)
    pub identity: SessionIdentityMeta,
}
```

### 6.2 MessageRecord (JSONL line schema)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TranscriptEntry {
    #[serde(rename = "message")]
    Message {
        id: String,               // uuid or snowflake
        role: String,             // "user" | "assistant" | "system" | "tool"
        content: String,
        timestamp: i64,
        metadata: Option<Value>,  // tool_call_id, attachments, etc.
    },
    #[serde(rename = "event")]
    Event {
        id: String,
        event: String,            // "run_started", "run_ended", "model_switch", "checkpoint"
        timestamp: i64,
        payload: Option<Value>,
    },
}
```

Each JSONL file starts with a header line:

```json
{"type":"header","version":"1.0","session_id":"...","created_at":"...","cwd":"..."}
```

### 6.3 CompactionCheckpoint

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    pub checkpoint_id: String,          // uuid
    pub created_at: i64,
    pub reason: CheckpointReason,       // manual | auto_threshold | overflow_retry | timeout_retry
    pub tokens_before: Option<u64>,
    pub tokens_after: Option<u64>,
    pub summary: Option<String>,
    pub first_kept_entry_id: Option<String>,
    pub pre_compaction: TranscriptReference,
    pub post_compaction: TranscriptReference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptReference {
    pub session_id: String,
    pub transcript_path: String,
    pub leaf_id: Option<String>,
}
```

**Compaction behavior**

1. Interrupt active run if any.
2. Read transcript; optionally invoke LLM summarizer (reuse existing `session_compactor`).
3. Write a **snapshot** of the current transcript to `{session_id}-checkpoint-{id}.jsonl`.
4. Rewrite the **active** transcript, keeping only the last N messages (or summary + tail).
5. Store both references in the checkpoint; update metadata.

---

## 7. Phase Roadmap (Chosen: Hybrid Parallel — C)

### Phase 1 — Abstraction & Dual Backend
**Duration:** 1 sprint  
**Goal:** Establish the `SessionStore` trait and run both backends side-by-side without breaking existing behavior.

**Tasks**

1. Define `SessionStore` trait + `SessionStoreError` in `gateway/session_store/mod.rs`.
2. Create `SqliteSessionStore` by extracting current `SessionManager` ops behind the trait.
   - Keep `messages` table fully operational.
   - Expose `SessionMetadata` v2 by enriching the existing DB rows with JSON metadata.
3. Create `FileSessionStore` skeleton:
   - `metadata` stored in a new SQLite table `session_metadata_v2` (key → JSON blob).
   - `transcript` stored as JSONL files under `~/.aleph/sessions/{agent_id}/{session_id}.jsonl`.
   - Implement append, read, reset.
4. Add `SessionStoreConfig` to `aleph.toml`:
   ```toml
   [session_store]
   backend = "sqlite"   # or "file"
   ```
5. Wire the chosen backend into `GatewayContext`.
6. Ensure all existing tests pass with `sqlite` backend.

**Deliverable:** `cargo test -p alephcore --lib` passes; config switch works.

---

### Phase 2 — Build Rich Features on File Backend
**Duration:** 1–1.5 sprints  
**Goal:** Make the file backend feature-complete with OpenClaw-level capabilities.

**Tasks**

1. **Checkpoint compaction**
   - Implement `compact`, `list_checkpoints`, `branch_from_checkpoint`, `restore_checkpoint`.
   - Reuse `components/session_compactor` for LLM summarization.
2. **Preview & derived title**
   - `preview`: efficient tail read of JSONL (last N KB).
   - `derive_title`: scan from start until first `role == "user"` message.
3. **Search**
   - Simple fallback: mmap + regex scan across JSONL files.
   - Optional fast path: maintain a small SQLite FTS5 index over `transcript_path` + `content` (separate from legacy messages table).
4. **Metadata enrichment**
   - On every `append_message`, update token counts (using same heuristics as today).
   - On run end, update `runtime_ms`, `status`, `estimated_cost_usd`.
5. **Archive hook**
   - On `delete_session`, move transcript to `~/.aleph/sessions/.archive/{date}/{session_id}.jsonl`.
6. **Gateway event plumbing**
   - Add `subscribe_session_events(conn_id)` / `unsubscribe...` to `GatewayContext`.
   - Emit `sessions.changed` after create, send, compact, delete, patch, reset.

**Deliverable:** All new RPCs work when `backend = "file"`; SQLite backend returns `Unsupported` for checkpoint APIs.

---

### Phase 3 — Cutover & Migration
**Duration:** 0.5 sprint  
**Goal:** Switch default to `file` and provide automatic migration.

**Tasks**

1. Change default `backend` to `"file"`.
2. Implement `session_store::migration::export_legacy_messages()`:
   - Iterate all rows in old `messages` table.
   - Write each to corresponding JSONL file.
   - Populate `session_metadata_v2` from existing `sessions` rows.
3. Run migration automatically on first startup if legacy messages table is non-empty and `file` backend is selected.
4. Update `builtin_tools/sessions/list_tool.rs` to use new `SessionFilter` and `SessionMetadata` fields.
5. Update Webchat/Tauri handlers to consume `sessions.changed`.

**Deliverable:** Fresh installs use file backend; existing installs auto-migrate; no data loss.

---

### Phase 4 — Cleanup & Retirement
**Duration:** 0.5 sprint  
**Goal:** Remove legacy code and finalize the architecture.

**Tasks**

1. Delete `gateway/handlers/session/store.rs` (in-memory handlers).
2. Remove legacy `messages` table writes from `SqliteSessionStore`; keep read-only for emergency rollback only.
3. Delete `StoredMessage` and old `SessionInfo` types if unused.
4. Consolidate `SessionKey` duplicates:
   - Migrate `gateway/router.rs` `SessionKey` usages to `routing/session_key.rs`.
   - Deprecate and finally delete the legacy router version.
5. Remove `metadata_json` string blob from `SessionMetadata` in favor of strongly-typed fields.
6. Add a `just` command for manual migration/rollback verification.

**Deliverable:** Clean build with zero warnings about deprecated session code; all tests green.

---

## 8. Gateway API Changes

### 8.1 New / Enhanced RPCs

| RPC | Change | Note |
|---|---|---|
| `sessions.list` | **Enhanced** | Returns new `SessionMetadata` fields (`derived_title`, `last_message_preview`, `total_tokens`, `model`, `status`, etc.). |
| `sessions.preview` | **New** | Params: `keys: string[]`, `limit?: number`, `max_chars?: number`. Returns lightweight preview items per session. |
| `sessions.subscribe` | **New** | Subscribes connection to `sessions.changed` broadcasts. |
| `sessions.unsubscribe` | **New** | Unsubscribes. |
| `sessions.messages.subscribe` | **New** | Subscribes to message events for a single session key. |
| `sessions.messages.unsubscribe` | **New** | Unsubscribes. |
| `sessions.compaction.list` | **New** | Lists compaction checkpoints. |
| `sessions.compaction.branch` | **New** | Branches a new session from a checkpoint. |
| `sessions.compaction.restore` | **New** | Restores current session to a checkpoint. |
| `sessions.patch` | **Enhanced** | Allows patching `label`, `model`, `thinking_level`, `fast_mode`, etc. (aligned with OpenClaw). |

### 8.2 Event Broadcast: `sessions.changed`

Payload shape (additive over current empty broadcast):

```json
{
  "session_key": "agent:main:main",
  "reason": "send|create|compact|delete|patch|reset|checkpoint-branch|checkpoint-restore",
  "ts": 1713345600000,
  "updated_at": 1713345600000,
  "session_id": "uuid",
  "kind": "direct",
  "channel": "telegram",
  "label": null,
  "display_name": null,
  "total_tokens": 1240,
  "model": "claude-sonnet-4",
  "status": "running",
  "compacted": false
}
```

Upper layers (Webchat, Tauri, macOS) can update their session list in place without re-calling `sessions.list`.

---

## 9. Migration & Compatibility

### 9.1 Rollback Strategy

- If `backend = "file"` causes issues, user can revert to `backend = "sqlite"` in `aleph.toml`.
- The old `messages` table remains untouched until Phase 4 cleanup, so rollback is immediate.
- After Phase 4, we will provide a one-way `aleph migrate-sessions` CLI command for explicit forward migration.

### 9.2 API Compatibility

- All existing RPCs keep the same method names and required params.
- New fields in responses are additive; old clients ignore them.
- `session_list` tool arguments remain valid; new optional filters are added.

---

## 10. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Trait boundary mis-designed | Medium | High | Phase 1 includes a full integration-test matrix against both backends. |
| File backend corruption / race conditions | Low | High | Append-only JSONL + atomic `rename()` for rewrites; metadata updates use SQLite transactions. |
| Migration loses messages | Low | Critical | Migration is read-only export; old table is kept. Validation step counts rows vs JSONL lines. |
| Frontend not updated for new events | Medium | Medium | New fields are additive; frontend can adopt incrementally after backend ships. |
| Performance regression on search | Medium | Medium | Fallback to file scan; can add parallel indexing later without breaking the trait. |

---

## 11. Open Decisions (Author Recommendations)

The following three questions were raised during brainstorming. This spec includes a recommended default; please flag during review if you prefer a different choice.

1. **SessionKey consolidation**  
   **Recommendation:** In Phase 1, unify `gateway/router.rs` `SessionKey` into `routing/session_key.rs`. The legacy variant has been a source of confusion (`from_key_string` vs `parse`, `LegacyKey` vs `RoutingKey`). A single type reduces conversion boilerplate.

2. **Checkpoint granularity**  
   **Recommendation:** Start with **automatic checkpoints on compaction only** (triggered by overflow or manual `sessions.compact`). Manual arbitrary checkpoint creation can be added later via `sessions.compaction.create` without changing the data model.

3. **Search implementation on FileBackend**  
   **Recommendation:** Use a **dedicated SQLite FTS5 index** (`transcript_search_index` table) that is updated on `append_message`. This gives us OpenClaw-level search speed without introducing external dependencies like Elasticsearch or ripgrep as a runtime requirement.

---

## 12. Definition of Done (Whole Project)

- [ ] `SessionStore` trait is the only persistence interface used by Gateway and tools.
- [ ] `FileSessionStore` is the default backend.
- [ ] `sessions.subscribe` + `sessions.changed` work across WebSocket clients.
- [ ] `sessions.preview` returns derived titles and last-message previews.
- [ ] Compaction creates recoverable checkpoints; `branch` and `restore` pass integration tests.
- [ ] Legacy `messages` table code is removed; no `#[allow(deprecated)]` hacks remain.
- [ ] All existing `cargo test -p alephcore --lib` tests pass.
- [ ] New integration tests cover: dual-backend parity, checkpoint roundtrip, migration validation, event broadcast.

---

*Next step after approval: invoke `writing-plans` to produce the implementation plan for Phase 1.*
