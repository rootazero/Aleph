# Design Document: Contextual Memory (Local RAG)

**Change ID**: `add-contextual-memory-rag`
**Last Updated**: 2025-12-24

## Overview

This document details the architectural design for Aether's context-aware memory system. The memory module enables Aether to remember past interactions and provide contextually relevant responses based on the user's active application and window.

## Design Goals

1. **Context-Aware**: Remember interactions per-app and per-window, not globally
2. **Privacy-First**: All memory data stays on-device, zero-knowledge cloud
3. **Low Latency**: Memory operations add <150ms to request processing
4. **Semantic Search**: Use embeddings for intelligent recall, not just keyword matching
5. **User Control**: Full transparency and management of stored memories
6. **Modular**: Swappable vector DB and embedding backends

## High-Level Architecture

```
┌───────────────────────────────────────────────────────────────────┐
│                        Swift UI Layer                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ Settings UI  │  │ EventHandler │  │ ContextCapture       │   │
│  │ (MemoryView) │  │              │  │ (NSWorkspace+AX API) │   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────────────┘   │
│         │                 │                  │                    │
│         └─────────────────┴──────────────────┘                    │
│                           │ UniFFI                                │
└───────────────────────────┼───────────────────────────────────────┘
                            ▼
┌───────────────────────────────────────────────────────────────────┐
│                      Rust Core Library                            │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │                      AetherCore                           │   │
│  │  ┌────────────────────────────────────────────────────┐  │   │
│  │  │            Request Processing Pipeline            │  │   │
│  │  │  1. Capture context (app+window)                  │  │   │
│  │  │  2. Retrieve memories ───────────────────┐        │  │   │
│  │  │  3. Augment prompt with memories         │        │  │   │
│  │  │  4. Send to AI provider                  │        │  │   │
│  │  │  5. Store interaction as new memory ─────┤        │  │   │
│  │  └────────────────────────────────────────┬─┴────────┘  │   │
│  └───────────────────────────────────────────┼─────────────┘   │
│                                              ▼                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                   Memory Module                          │   │
│  │  ┌────────────┐  ┌────────────┐  ┌──────────────────┐  │   │
│  │  │  Context   │  │ Embedding  │  │ Vector Database  │  │   │
│  │  │  (Anchor)  │  │   Model    │  │ (SQLite+vec /    │  │   │
│  │  │            │  │ (ONNX RT)  │  │  LanceDB)        │  │   │
│  │  └────────────┘  └────────────┘  └──────────────────┘  │   │
│  │  ┌─────────────────────────────────────────────────┐   │   │
│  │  │  Ingestion  │  Retrieval  │  Augmentation       │   │   │
│  │  │  (Store)    │  (Search)   │  (Prompt Injection) │   │   │
│  │  └─────────────────────────────────────────────────┘   │   │
│  └──────────────────────────────────────────────────────────┘   │
└───────────────────────────────────────────────────────────────────┘
                            │
                            ▼
                ┌───────────────────────┐
                │  Local Storage        │
                │  ~/.config/aether/    │
                │  - memory.db          │
                │  - models/            │
                └───────────────────────┘
```

## Component Design

### 1. Context Capture (Swift → Rust)

**Purpose**: Capture the current application and window context at the moment of hotkey press.

**Implementation Location**: Primarily Swift (`ContextCapture.swift`)

**Data Flow**:
```
User presses hotkey
  ↓
Swift EventHandler receives event
  ↓
Call NSWorkspace.shared.frontmostApplication?.bundleIdentifier
  ↓
Call AXUIElementCopyAttributeValue for window title (Accessibility API)
  ↓
Package as CapturedContext struct
  ↓
Send to Rust via core.setCurrentContext(context)
  ↓
Rust stores in Arc<Mutex<Option<CapturedContext>>>
```

**Key Considerations**:
- **Permission Handling**: Requires Accessibility permission for window title
  - Gracefully degrade: use bundle ID only if title unavailable
  - Prompt user for permission on first use
- **Error Handling**: Some apps don't expose window title (e.g., menubar apps)
  - Fall back to empty string or generic title
- **Performance**: Capture must be fast (<10ms) to avoid hotkey latency
  - Cache `AXUIElementRef` if possible

**Data Structures**:
```rust
// In aether.udl
dictionary CapturedContext {
  string app_bundle_id;       // e.g., "com.apple.Notes"
  string? window_title;       // e.g., "Project Plan.txt"
};

// In Rust
pub struct ContextAnchor {
    pub app_bundle_id: String,
    pub window_title: String,  // Empty string if unavailable
    pub timestamp: i64,        // Unix timestamp
}
```

---

### 2. Vector Database

**Purpose**: Store interaction embeddings with context metadata for fast similarity search.

**Technology Choice**: SQLite + `sqlite-vec` extension

**Rationale**:
- ✅ Familiar technology, easier debugging
- ✅ Simpler deployment (single library)
- ✅ Good performance for <100k memories
- ✅ Supports metadata filtering + vector search
- ❌ Slower than LanceDB for very large datasets (acceptable trade-off for Phase 4)

**Schema**:
```sql
-- Main memories table
CREATE TABLE memories (
  id TEXT PRIMARY KEY,                  -- UUID
  app_bundle_id TEXT NOT NULL,          -- e.g., "com.apple.Notes"
  window_title TEXT NOT NULL,           -- e.g., "Project Plan.txt"
  user_input TEXT NOT NULL,             -- Original user input
  ai_output TEXT NOT NULL,              -- AI response
  timestamp INTEGER NOT NULL,           -- Unix timestamp
  created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

-- Index for fast context filtering
CREATE INDEX idx_context ON memories(app_bundle_id, window_title);
CREATE INDEX idx_timestamp ON memories(timestamp);

-- Virtual table for vector search (sqlite-vec)
CREATE VIRTUAL TABLE vec_memories USING vec0(
  id TEXT PRIMARY KEY,
  embedding FLOAT[384]  -- 384-dimensional for all-MiniLM-L6-v2
);
```

**Query Pattern** (similarity search with context filter):
```sql
-- Step 1: Get candidate IDs from context filter
WITH candidates AS (
  SELECT id FROM memories
  WHERE app_bundle_id = ?
    AND window_title = ?
  ORDER BY timestamp DESC
  LIMIT 100  -- Pre-filter to recent memories
)
-- Step 2: Vector similarity search on candidates
SELECT m.*, vec.distance
FROM vec_memories vec
JOIN memories m ON vec.id = m.id
WHERE vec.id IN (SELECT id FROM candidates)
  AND vec.embedding MATCH ?  -- Query embedding
ORDER BY vec.distance ASC
LIMIT ?;  -- max_context_items from config
```

**Performance Characteristics**:
- **Insert**: O(log n) with index updates
- **Search**: O(n) for filtered vector scan (acceptable for n < 1000 per context)
- **Storage**: ~1.5KB per memory (text + 384 * 4 bytes for f32 embedding)

**Migration Path to LanceDB** (future optimization):
- LanceDB offers better performance for large datasets
- Migration: export from SQLite, import to Lance format
- Abstraction: `VectorDatabase` trait allows swapping backends

---

### 3. Embedding Model

**Purpose**: Convert text to dense vector representations for semantic similarity.

**Model**: `sentence-transformers/all-MiniLM-L6-v2`

**Specifications**:
- **Architecture**: Transformer-based sentence encoder
- **Dimensions**: 384-dimensional float32 vectors
- **Size**: ~23MB (ONNX format)
- **License**: Apache 2.0
- **Input**: Text string (max 256 tokens after tokenization)
- **Output**: Fixed-size 384-dim vector (L2-normalized)

**Inference Engine**: ONNX Runtime (`ort` crate)

**Rationale**:
- ✅ Mature, production-ready
- ✅ Pre-compiled binaries available
- ✅ Cross-platform (macOS, Windows, Linux)
- ✅ Faster than pure-Rust alternatives (e.g., Candle) for Phase 4
- ❌ External dependency (acceptable for simplicity)

**Model Loading Strategy**:
```rust
pub struct EmbeddingModel {
    session: Arc<Mutex<Option<Session>>>,  // Lazy-loaded
    tokenizer: Arc<Tokenizer>,
    model_path: PathBuf,
}

impl EmbeddingModel {
    pub fn new(model_path: PathBuf) -> Result<Self> {
        let tokenizer = Tokenizer::from_file(&model_path.join("tokenizer.json"))?;
        Ok(Self {
            session: Arc::new(Mutex::new(None)),
            tokenizer: Arc::new(tokenizer),
            model_path,
        })
    }

    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        // Load session on first use
        let mut session_guard = self.session.lock().await;
        if session_guard.is_none() {
            let session = Session::builder()?
                .with_model_from_file(self.model_path.join("model.onnx"))?;
            *session_guard = Some(session);
        }

        // Tokenize + run inference
        let tokens = self.tokenizer.encode(text, true)?;
        let outputs = session_guard.as_ref().unwrap().run(...)?;
        Ok(outputs.embedding)
    }
}
```

**Performance Optimization**:
- **Lazy Loading**: Only load model when first memory operation occurs
- **Caching**: Keep model in memory for subsequent requests
- **Batching**: Process multiple texts in one inference pass (future optimization)
- **Quantization**: Use INT8 quantized model if float32 too slow (future optimization)

**Text Preprocessing**:
1. Concatenate user input + AI output: `"{user_input}\n\n{ai_output}"`
2. Truncate to 256 tokens (model max length)
3. Tokenize with `all-MiniLM-L6-v2` tokenizer
4. Mean pooling over token embeddings
5. L2 normalize final vector

---

### 4. Ingestion Pipeline

**Purpose**: Store new interactions as memories after successful AI responses.

**Trigger Point**: After `AetherCore` receives AI response and sends to user.

**Data Flow**:
```
AI response received
  ↓
Extract current context (app_bundle_id, window_title)
  ↓
Spawn async task (don't block response)
  ↓
Apply PII scrubbing to user_input + ai_output
  ↓
Generate embedding for concatenated text
  ↓
Insert into database with context anchor
  ↓
Log success/failure
```

**Implementation**:
```rust
pub struct MemoryIngestion {
    database: Arc<VectorDatabase>,
    embedding_model: Arc<EmbeddingModel>,
    config: Arc<MemoryConfig>,
}

impl MemoryIngestion {
    pub async fn store_memory(
        &self,
        context: ContextAnchor,
        user_input: &str,
        ai_output: &str,
    ) -> Result<()> {
        // 1. Check if memory enabled
        if !self.config.enabled {
            return Ok(());
        }

        // 2. Check if app excluded
        if self.config.excluded_apps.contains(&context.app_bundle_id) {
            return Ok(());
        }

        // 3. Scrub PII
        let scrubbed_input = scrub_pii(user_input);
        let scrubbed_output = scrub_pii(ai_output);

        // 4. Generate embedding
        let text = format!("{}\n\n{}", scrubbed_input, scrubbed_output);
        let embedding = self.embedding_model.embed_text(&text).await?;

        // 5. Insert into database
        let memory = MemoryEntry {
            id: Uuid::new_v4().to_string(),
            context,
            user_input: scrubbed_input.to_string(),
            ai_output: scrubbed_output.to_string(),
            embedding: Some(embedding),
            similarity_score: None,
        };
        self.database.insert_memory(memory).await?;

        Ok(())
    }
}
```

**Error Handling**:
- Embedding generation failure: Log error, skip storage (don't block response)
- Database write failure: Log error, retry once, then skip
- PII scrubbing failure: Skip storage (safety first)

**Performance**:
- Run entirely async (tokio task)
- Target: Complete within 200ms (not blocking user-facing response)
- If slow, queue for background processing

---

### 5. Retrieval Module

**Purpose**: Find semantically similar past interactions filtered by current context.

**Query Flow**:
```
User submits new request
  ↓
Get current context (app_bundle_id, window_title)
  ↓
Embed user query text
  ↓
Search vector DB:
  - Filter by app_bundle_id AND window_title
  - Rank by cosine similarity
  - Apply similarity_threshold
  - Return top-K (max_context_items)
  ↓
Return MemoryEntry list with similarity scores
```

**Implementation**:
```rust
pub struct MemoryRetrieval {
    database: Arc<VectorDatabase>,
    embedding_model: Arc<EmbeddingModel>,
    config: Arc<MemoryConfig>,
}

impl MemoryRetrieval {
    pub async fn retrieve_memories(
        &self,
        context: &ContextAnchor,
        query: &str,
    ) -> Result<Vec<MemoryEntry>> {
        // 1. Check if memory enabled
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        // 2. Generate query embedding
        let query_embedding = self.embedding_model.embed_text(query).await?;

        // 3. Search database
        let mut memories = self.database
            .search_memories(
                &context.app_bundle_id,
                &context.window_title,
                &query_embedding,
                self.config.max_context_items,
            )
            .await?;

        // 4. Filter by similarity threshold
        memories.retain(|m| {
            m.similarity_score.unwrap_or(0.0) >= self.config.similarity_threshold
        });

        // 5. Sort by timestamp (newest first) if same similarity
        memories.sort_by(|a, b| {
            b.context.timestamp.cmp(&a.context.timestamp)
        });

        Ok(memories)
    }
}
```

**Similarity Metric**: Cosine similarity
```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot_product / (norm_a * norm_b)
}
```

**Performance**:
- Target: <50ms for typical query (10-100 memories per context)
- Optimization: Pre-filter by context before vector scan
- Fallback: If >1000 memories, use approximate nearest neighbor (ANN)

---

### 6. Augmentation Module

**Purpose**: Inject retrieved memories into LLM system prompt.

**Prompt Template**:
```
You are Aether AI, an intelligent assistant that helps users with various tasks.

[If memories available:]
Here are relevant past interactions in this context ({{app_name}} - {{window_title}}):

{{#each memories}}
[Previous Interaction {{@index + 1}} - {{timestamp}}]
User: {{user_input}}
Assistant: {{ai_output}}

{{/each}}

[End of context]

Now, respond to the current request:
User: {{current_input}}
```

**Implementation**:
```rust
pub struct PromptAugmenter {
    config: Arc<MemoryConfig>,
}

impl PromptAugmenter {
    pub fn augment_prompt(
        &self,
        base_prompt: &str,
        memories: &[MemoryEntry],
        current_input: &str,
    ) -> String {
        if memories.is_empty() {
            return format!("{}\n\nUser: {}", base_prompt, current_input);
        }

        let mut prompt = base_prompt.to_string();
        prompt.push_str("\n\nHere are relevant past interactions:\n\n");

        for (i, memory) in memories.iter().enumerate() {
            let timestamp = format_timestamp(memory.context.timestamp);
            prompt.push_str(&format!(
                "[Previous Interaction {} - {}]\n\
                 User: {}\n\
                 Assistant: {}\n\n",
                i + 1,
                timestamp,
                truncate(&memory.user_input, 500),  // Avoid token limit
                truncate(&memory.ai_output, 500)
            ));
        }

        prompt.push_str(&format!("\nNow respond to:\nUser: {}", current_input));
        prompt
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
```

**Token Budget Management**:
- Reserve ~2000 tokens for memories (configurable)
- If total memory text exceeds budget:
  - Truncate older memories first
  - Or reduce max_context_items dynamically
- Monitor via tokenizer before sending to LLM

---

### 7. Privacy & Security

**Data Flow Security**:
```
User Input (Local)
  ↓ PII Scrubbing
Scrubbed Input (Local)
  ↓ Embedding
Embedding Vector (Local)
  ↓ Store
SQLite Database (Local)
  ↓ Query
Retrieved Memories (Local)
  ↓ Augment Prompt
Augmented Prompt (Sent to Cloud LLM)
  ↓
LLM Response (From Cloud)
```

**Key Principle**: Cloud LLM only sees:
- Current user input (after PII scrubbing)
- Retrieved memory snippets (already scrubbed)
- System prompt

**Cloud LLM never sees**:
- Full memory database
- Raw embeddings
- Unfiltered historical data

**PII Scrubbing**:
Reuse existing regex patterns (from Phase 5):
```rust
fn scrub_pii(text: &str) -> String {
    let email_regex = Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap();
    let phone_regex = Regex::new(r"\b\d{3}[-.]?\d{3}[-.]?\d{4}\b").unwrap();
    let ssn_regex = Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();

    let mut scrubbed = text.to_string();
    scrubbed = email_regex.replace_all(&scrubbed, "[EMAIL]").to_string();
    scrubbed = phone_regex.replace_all(&scrubbed, "[PHONE]").to_string();
    scrubbed = ssn_regex.replace_all(&scrubbed, "[SSN]").to_string();
    scrubbed
}
```

**App Exclusion List**:
Default excluded apps (don't store memories):
- `com.apple.keychainaccess` (Keychain Access)
- `com.agilebits.onepassword7` (1Password)
- `com.lastpass.LastPass` (LastPass)
- `com.bitwarden.desktop` (Bitwarden)

User can add more via config:
```toml
[memory]
excluded_apps = [
  "com.apple.keychainaccess",
  "com.bank.app",  # User-added
]
```

**Database File Permissions**:
```rust
use std::os::unix::fs::PermissionsExt;

fn create_database(path: &Path) -> Result<()> {
    let db = Database::open(path)?;

    // Set permissions to 600 (owner read/write only)
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;

    Ok(())
}
```

---

### 8. Retention Policies & Cleanup

**Purpose**: Automatically delete old memories to prevent unbounded growth.

**Default Policy**: 90 days retention

**Cleanup Service**:
```rust
pub struct CleanupService {
    database: Arc<VectorDatabase>,
    config: Arc<MemoryConfig>,
}

impl CleanupService {
    pub async fn start(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(86400));  // Daily
        loop {
            interval.tick().await;
            if let Err(e) = self.cleanup_old_memories().await {
                error!("Memory cleanup failed: {}", e);
            }
        }
    }

    async fn cleanup_old_memories(&self) -> Result<()> {
        if self.config.retention_days == 0 {
            return Ok(());  // Never delete
        }

        let cutoff_timestamp = Utc::now().timestamp() - (self.config.retention_days as i64 * 86400);
        let deleted = self.database.delete_older_than(cutoff_timestamp).await?;
        info!("Cleaned up {} old memories", deleted);
        Ok(())
    }
}
```

**Manual Cleanup**:
User can trigger via Settings UI:
- Delete all memories (with confirmation)
- Delete by app/window filter
- Delete specific memory by ID

---

## Configuration Design

**Full Schema**:
```toml
[memory]
enabled = true                          # Master toggle
embedding_model = "all-MiniLM-L6-v2"    # Model name
max_context_items = 5                   # Top-K retrieval
retention_days = 90                     # Auto-delete after N days (0 = never)
vector_db = "sqlite-vec"                # sqlite-vec | lancedb
similarity_threshold = 0.7              # Minimum similarity to include (0.0-1.0)
excluded_apps = [                       # Bundle IDs to exclude
  "com.apple.keychainaccess",
]
```

**Config Loading**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default = "default_model")]
    pub embedding_model: String,

    #[serde(default = "default_max_items")]
    pub max_context_items: u32,

    #[serde(default = "default_retention")]
    pub retention_days: u32,

    #[serde(default = "default_db")]
    pub vector_db: String,

    #[serde(default = "default_threshold")]
    pub similarity_threshold: f32,

    #[serde(default)]
    pub excluded_apps: Vec<String>,
}

fn default_enabled() -> bool { true }
fn default_model() -> String { "all-MiniLM-L6-v2".to_string() }
fn default_max_items() -> u32 { 5 }
fn default_retention() -> u32 { 90 }
fn default_db() -> String { "sqlite-vec".to_string() }
fn default_threshold() -> f32 { 0.7 }
```

---

## Integration with Existing System

**Request Pipeline Modification**:

**Before (Phase 3)**:
```
Hotkey → Clipboard → Route to AI → Send → Paste Response
```

**After (Phase 4)**:
```
Hotkey → Capture Context → Clipboard
  ↓
Retrieve Memories (async)
  ↓
Augment Prompt → Route to AI → Send
  ↓
Store Memory (async) → Paste Response
```

**Code Integration Points**:

1. **In `core.rs` hotkey handler**:
   ```rust
   async fn handle_hotkey(&self) {
       // Existing: Get clipboard content
       let clipboard = self.clipboard.get_text()?;

       // NEW: Retrieve memories
       let memories = self.memory_retrieval
           .retrieve_memories(&self.current_context, &clipboard)
           .await?;

       // NEW: Augment prompt
       let augmented_prompt = self.prompt_augmenter
           .augment_prompt(&base_prompt, &memories, &clipboard);

       // Existing: Route to AI
       let response = self.router.route(&augmented_prompt).await?;

       // NEW: Store memory (async, don't wait)
       tokio::spawn({
           let ingestion = self.memory_ingestion.clone();
           let context = self.current_context.clone();
           let input = clipboard.clone();
           let output = response.clone();
           async move {
               let _ = ingestion.store_memory(context, &input, &output).await;
           }
       });

       // Existing: Paste response
       self.input_simulator.paste(&response)?;
   }
   ```

2. **In Swift `EventHandler`**:
   ```swift
   func onHotkeyDetected(clipboardContent: String) {
       // NEW: Capture context
       let bundleId = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "unknown"
       let windowTitle = captureWindowTitle() ?? ""
       let context = CapturedContext(
           appBundleId: bundleId,
           windowTitle: windowTitle
       )

       // NEW: Send context to Rust
       core.setCurrentContext(context: context)

       // Existing: Continue processing
       // ...
   }
   ```

---

## Performance Budget

**End-to-End Target**: <150ms total memory overhead

**Breakdown**:
- Context capture (Swift): ~5ms
- Embedding generation (Rust): ~80ms
- Vector search (SQLite): ~30ms
- Prompt augmentation (Rust): ~10ms
- Storage (async, non-blocking): ~150ms

**Optimizations**:
- Lazy model loading (first use only)
- Keep model in memory (no per-request loading)
- Pre-built database indices
- Async storage (doesn't block response)

**Monitoring**:
```rust
use std::time::Instant;

let start = Instant::now();
let memories = retrieve_memories(...).await?;
let elapsed = start.elapsed();
if elapsed.as_millis() > 100 {
    warn!("Memory retrieval slow: {}ms", elapsed.as_millis());
}
```

---

## Testing Strategy

**Unit Tests** (per module):
1. **database.rs**: CRUD, vector search, filtering
2. **embedding.rs**: Model loading, inference, batching
3. **context.rs**: Serialization, timestamp handling
4. **ingestion.rs**: PII scrubbing, storage flow
5. **retrieval.rs**: Similarity calculation, filtering
6. **augmentation.rs**: Prompt formatting, truncation

**Integration Tests**:
1. End-to-end: Store → Retrieve → Augment
2. Context isolation: Verify no cross-contamination
3. Retention: Insert old memory → cleanup → verify deleted
4. Error handling: Database locked, model missing, etc.

**Performance Tests**:
1. Benchmark embedding inference (target: <100ms)
2. Benchmark vector search (target: <50ms)
3. Load test: 10,000 memories, measure search time
4. Memory leak test: Long-running process

**Manual Tests**:
1. Real-world usage in Notes.app (multiple documents)
2. Cross-app isolation (Notes vs VSCode)
3. Settings UI CRUD operations
4. Permission handling (Accessibility denial)

---

## Migration & Rollout Plan

**Phase 4A-4E** (Internal):
- Develop in feature branch
- Alpha testing with 1-2 users
- Collect performance metrics

**Beta Release**:
- Memory disabled by default
- User opts in via Settings
- Provide clear privacy documentation

**Production Release**:
- Memory enabled by default (opt-out)
- Onboarding tutorial explaining feature
- Settings shortcut for quick management

**Data Migration**: N/A (new feature, no existing data)

---

## Open Questions & Decisions

### Q1: Vector DB Choice
**Options**: SQLite+vec vs LanceDB
**Decision**: SQLite+vec for Phase 4
**Rationale**: Simpler, easier debugging, sufficient performance
**Future**: Migrate to LanceDB if performance bottleneck (Phase 7)

### Q2: Embedding Inference Engine
**Options**: ONNX Runtime vs Candle (pure Rust)
**Decision**: ONNX Runtime for Phase 4
**Rationale**: Mature, faster, pre-built binaries
**Future**: Evaluate Candle in Phase 7 (optimization)

### Q3: Context Capture Location
**Options**: Swift vs Rust
**Decision**: Swift-side capture, pass to Rust via UniFFI
**Rationale**: Easier access to macOS NSWorkspace and Accessibility APIs
**Trade-off**: Tight coupling with macOS (acceptable for Phase 4)

### Q4: Memory Usage Indicator
**Options**: Show/hide indicator when memory used
**Decision**: Optional, disabled by default
**Rationale**: Avoid UI clutter, power users can enable in Settings

### Q5: Cross-Device Sync
**Options**: iCloud sync, custom cloud service
**Decision**: Not in Phase 4 (local-only)
**Rationale**: Privacy-first, avoid complexity
**Future**: Consider as opt-in feature in Phase 8

---

## Risk Mitigation

### Risk: Performance Overhead
**Mitigation**:
- Async operations (don't block UI)
- Lazy model loading
- Performance monitoring and alerts
- Fallback: Disable memory if too slow

### Risk: Memory Bloat
**Mitigation**:
- Default 90-day retention
- User-facing database size indicator
- Warn if DB > 100MB

### Risk: Privacy Concerns
**Mitigation**:
- Clear documentation on local-only storage
- Prominent "Clear All" button in Settings
- App exclusion list
- PII scrubbing before storage

### Risk: Context Capture Accuracy
**Mitigation**:
- Graceful degradation (bundle ID only if window title fails)
- Log context captures for debugging
- User can manually delete incorrect memories

---

## Success Metrics

After implementation:
- [ ] Memory retrieval adds <150ms to request latency
- [ ] Embedding inference <100ms
- [ ] Vector search <50ms
- [ ] User can manage all memories via Settings UI
- [ ] No memory data sent to cloud (verified via network monitoring)
- [ ] Passes all unit + integration tests
- [ ] Positive user feedback on context-awareness

---

## Future Enhancements (Post-Phase 4)

1. **Cross-Device Sync** (Phase 8):
   - Optional iCloud sync (encrypted at rest)
   - Conflict resolution for concurrent edits

2. **Advanced Retrieval** (Phase 7):
   - Hybrid search (keyword + semantic)
   - Temporal decay (older memories less relevant)
   - Collaborative filtering (learn from user preferences)

3. **Memory Clustering** (Phase 8):
   - Group related memories into "topics"
   - Visualize memory graph in Settings

4. **Export/Import** (Phase 6):
   - Export memories as JSON
   - Import from other tools (Notion, Obsidian)

5. **Summarization** (Phase 7):
   - Automatically summarize long conversation threads
   - Store summaries instead of full text

---

## Appendix: Code Structure

```
Aether/core/src/memory/
├── mod.rs                  # Public API exports
├── database.rs             # VectorDatabase trait + SQLite impl
├── embedding.rs            # EmbeddingModel + ONNX inference
├── context.rs              # ContextAnchor + MemoryEntry types
├── ingestion.rs            # MemoryIngestion service
├── retrieval.rs            # MemoryRetrieval service
├── augmentation.rs         # PromptAugmenter
├── cleanup.rs              # CleanupService (retention policies)
└── tests/
    ├── integration.rs      # End-to-end tests
    └── fixtures/           # Test data (sample memories)
```

---

## References

- [Sentence Transformers](https://www.sbert.net/)
- [SQLite Vec Extension](https://github.com/asg017/sqlite-vec)
- [ONNX Runtime](https://onnxruntime.ai/)
- [UniFFI Book](https://mozilla.github.io/uniffi-rs/)
- [macOS Accessibility API](https://developer.apple.com/documentation/accessibility)
