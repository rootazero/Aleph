# Proposal: Add Contextual Memory (Local RAG)

**Change ID**: `add-contextual-memory-rag`
**Status**: Proposed
**Author**: System
**Date**: 2025-12-24

## Overview

Implement a local RAG (Retrieval-Augmented Generation) memory system that enables Aether to remember past interactions and provide context-aware AI responses based on the active application and window context.

## Problem Statement

Currently, Aether processes each user request in isolation without any memory of past interactions. This limits its ability to:
- Provide continuity in conversations within specific contexts (e.g., a particular document or chat window)
- Build up understanding over time about user preferences or project-specific terminology
- Offer more relevant and personalized responses based on historical context

Users must manually re-provide context in every interaction, which breaks the "frictionless" philosophy of Aether.

## Proposed Solution

Add a **Context-Aware Local RAG** system that:

1. **Captures Context Anchors**: Tags each interaction with metadata:
   - `app_bundle_id` (e.g., `com.apple.Notes`)
   - `window_title` (e.g., "Project Plan.txt")
   - `timestamp` (UTC)

2. **Stores Memories Locally**: Uses an embedded vector database to store interaction embeddings:
   - User input + AI output pairs embedded using a lightweight local model
   - Zero-knowledge cloud: raw memories never leave the device
   - Database file stored in `~/.aether/memory.db` or `~/.aether/memory.lance`

3. **Retrieves Relevant Context**: When processing a new request:
   - Query vector DB filtered by current `app_bundle_id` + `window_title`
   - Retrieve top-K most similar past interactions
   - Inject retrieved context into LLM system prompt

4. **Privacy-First Design**:
   - All embeddings and storage remain on-device
   - Only the final augmented prompt (with retrieved context) is sent to cloud LLMs
   - User controls: view all memories, delete specific entries, configure retention policies

## Technical Approach

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                       AetherCore (Rust)                      │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │  Context    │  │  Embedding   │  │  Vector Database │   │
│  │  Capture    │→ │  Model       │→ │  (LanceDB/       │   │
│  │  (macOS     │  │  (all-MiniLM │  │   sqlite-vec)    │   │
│  │   AX API)   │  │   -L6-v2)    │  │                  │   │
│  └─────────────┘  └──────────────┘  └──────────────────┘   │
│         ↓                                      ↑             │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Memory Module                                      │    │
│  │  - Ingestion: Store user input + AI output         │    │
│  │  - Retrieval: Query by context + similarity        │    │
│  │  - Augmentation: Inject context into prompts       │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### Component Breakdown

1. **Context Capture Module** (`memory/context.rs`)
   - Use macOS Accessibility API to query active app/window
   - Capture at hotkey press moment
   - Bundle ID: `NSWorkspace.shared.frontmostApplication?.bundleIdentifier`
   - Window title: `AXUIElementCopyAttributeValue` with `kAXTitleAttribute`

2. **Embedding Module** (`memory/embedding.rs`)
   - Local inference using ONNX Runtime (`ort` crate) or Candle
   - Model: `all-MiniLM-L6-v2` (384-dimensional embeddings, ~23MB)
   - Lazy loading: load model on first use
   - Performance target: <100ms inference time

3. **Vector Database** (`memory/database.rs`)
   - Option A: LanceDB (native Rust, high performance)
   - Option B: SQLite + `sqlite-vec` extension (simpler, familiar)
   - Schema:
     ```sql
     CREATE TABLE memories (
       id INTEGER PRIMARY KEY,
       app_bundle_id TEXT NOT NULL,
       window_title TEXT NOT NULL,
       user_input TEXT NOT NULL,
       ai_output TEXT NOT NULL,
       embedding BLOB NOT NULL,
       timestamp INTEGER NOT NULL,
       INDEX idx_context (app_bundle_id, window_title)
     );
     ```

4. **Ingestion Pipeline** (`memory/ingestion.rs`)
   - Triggered after successful AI response
   - Concatenate user input + AI output
   - Generate embedding asynchronously
   - Store in database with context anchors

5. **Retrieval Module** (`memory/retrieval.rs`)
   - Query: embed current user input
   - Filter: WHERE app_bundle_id = ? AND window_title = ?
   - Rank: cosine similarity with stored embeddings
   - Return: top-K interactions (default K=5)

6. **Augmentation Module** (`memory/augmentation.rs`)
   - Format retrieved memories as context
   - Inject into system prompt:
     ```
     You are Aether AI. Here are relevant past interactions in this context:

     [Memory 1]
     User: ...
     Assistant: ...

     [Memory 2]
     ...

     Now respond to the current request:
     ```

### Configuration Schema

Add to `config.toml`:

```toml
[memory]
enabled = true                     # Enable/disable memory module
embedding_model = "all-MiniLM-L6-v2"  # Local embedding model
max_context_items = 5              # Max number of past interactions to retrieve
retention_days = 90                # Auto-delete memories older than N days (0 = never)
vector_db = "lancedb"              # lancedb | sqlite-vec
similarity_threshold = 0.7         # Minimum similarity score to include memory
```

### UniFFI Interface Additions

Add to `aether.udl`:

```idl
// Memory context for a single interaction
dictionary MemoryEntry {
  string id;
  string app_bundle_id;
  string window_title;
  string user_input;
  string ai_output;
  i64 timestamp;
  f32 similarity_score;  // When retrieved
};

// Memory module interface
interface AetherCore {
  // ... existing methods ...

  // Get memory statistics
  MemoryStats get_memory_stats();

  // Search memories by context
  sequence<MemoryEntry> search_memories(string app_bundle_id, string? window_title, u32 limit);

  // Delete specific memory by ID
  [Throws=AetherError]
  void delete_memory(string id);

  // Clear all memories (with optional context filter)
  [Throws=AetherError]
  void clear_memories(string? app_bundle_id, string? window_title);

  // Get memory configuration
  MemoryConfig get_memory_config();

  // Update memory configuration
  [Throws=AetherError]
  void update_memory_config(MemoryConfig config);
};

// Memory statistics
dictionary MemoryStats {
  u64 total_memories;
  u64 total_apps;
  f64 database_size_mb;
  i64 oldest_memory_timestamp;
  i64 newest_memory_timestamp;
};

// Memory configuration
dictionary MemoryConfig {
  boolean enabled;
  string embedding_model;
  u32 max_context_items;
  u32 retention_days;
  string vector_db;
  f32 similarity_threshold;
};
```

## Dependencies

### New Rust Crates

```toml
[dependencies]
# Vector database (choose one)
lancedb = { version = "0.4", optional = true }
rusqlite = { version = "0.30", features = ["bundled"], optional = true }

# ONNX Runtime for embedding inference (choose one)
ort = { version = "2.0", features = ["download-binaries"], optional = true }
candle-core = { version = "0.3", optional = true }
candle-nn = { version = "0.3", optional = true }
candle-transformers = { version = "0.3", optional = true }

# Tokenization
tokenizers = "0.15"

# macOS Accessibility API (Swift interop)
# Note: Context capture will be implemented in Swift and passed to Rust

[features]
default = ["lancedb", "ort"]
lancedb-backend = ["lancedb"]
sqlite-backend = ["rusqlite"]
ort-inference = ["ort"]
candle-inference = ["candle-core", "candle-nn", "candle-transformers"]
```

### Embedding Model

- **Model**: `sentence-transformers/all-MiniLM-L6-v2`
- **Format**: ONNX (for `ort`) or SafeTensors (for `candle`)
- **Download Source**: Hugging Face Hub
- **Storage**: `~/.aether/models/all-MiniLM-L6-v2/`
- **Size**: ~23MB
- **License**: Apache 2.0

## Performance Considerations

### Embedding Inference
- **Target**: <100ms per inference
- **Strategy**:
  - Lazy model loading (load on first use)
  - Model quantization (INT8) for faster inference
  - Batch processing if multiple memories need embedding

### Vector Search
- **Target**: <50ms per query
- **Strategy**:
  - Index on `(app_bundle_id, window_title)` for fast filtering
  - Approximate nearest neighbor search (ANN) for large databases
  - LanceDB has built-in ANN support

### Memory Overhead
- **Database Size**: ~1KB per memory entry (text + 384-dim float32 embedding = ~1.5KB)
- **Example**: 10,000 memories = ~15MB
- **Mitigation**: Automatic cleanup based on retention policy

## Privacy & Security

### Zero-Knowledge Cloud
- Raw memory data never transmitted to external servers
- Only the final augmented prompt (with retrieved snippets) sent to cloud LLMs
- Cloud providers never see the full memory database

### Local Storage
- Database file: `~/.aether/memory.db` or `.lance`
- Permissions: 600 (user read/write only)
- No encryption at rest (relies on OS-level disk encryption like FileVault)

### User Controls
- **View All Memories**: Settings UI shows all stored interactions
- **Delete Specific Memory**: User can delete individual entries
- **Clear by Context**: Delete all memories for specific app/window
- **Clear All**: Nuclear option to wipe entire memory database
- **Retention Policy**: Auto-delete after N days (configurable)
- **Disable Memory**: Toggle to turn off memory entirely

### Sensitive Data Handling
- PII scrubbing happens BEFORE memory storage (reuse existing PII filter)
- User can exclude specific apps from memory (e.g., password managers)

## User Experience

### Typical Flow

1. **User works in Notes.app**, document "Project Plan.txt"
2. **First interaction**:
   - User: "Summarize the key milestones"
   - Aether: [Summarizes] (no context available yet)
   - Memory: Stores interaction tagged with `com.apple.Notes` + "Project Plan.txt"

3. **Second interaction** (same document):
   - User: "What's the timeline for Phase 2?"
   - Memory: Retrieves previous summary from vector DB
   - Aether: [Responds with context from previous interaction]

4. **Third interaction** (different document):
   - User switches to "Budget.txt" in Notes.app
   - Memory: No relevant context (different window_title)
   - Aether: Starts fresh for this document

### Settings UI (Phase 6)

Add new "Memory" tab in Settings:
- **Enable/Disable**: Toggle switch
- **Retention Policy**: Dropdown (7/30/90/365 days, Never)
- **Max Context Items**: Slider (1-10)
- **Memory Browser**:
  - List of all memories grouped by app
  - Search/filter by app, window, date range
  - Delete button for each entry
  - "Clear All" button with confirmation dialog
- **Statistics**: Total memories, database size, oldest/newest entry

## Implementation Phases

### Phase 4A: Foundation (Weeks 1-2)
- Set up memory module structure (`memory/` directory)
- Integrate vector database (LanceDB or SQLite+vec)
- Implement basic storage/retrieval without embeddings (exact match)
- Add configuration schema and parsing

### Phase 4B: Embedding Integration (Weeks 3-4)
- Download and integrate embedding model
- Implement local inference with ONNX Runtime or Candle
- Add embedding generation to ingestion pipeline
- Implement similarity-based retrieval

### Phase 4C: Context Capture (Week 5)
- Implement macOS Accessibility API queries in Swift
- Add UniFFI bridge for context data
- Capture app_bundle_id and window_title on hotkey press
- Pass context to Rust core

### Phase 4D: Augmentation & Testing (Week 6)
- Implement context augmentation for LLM prompts
- Add comprehensive unit tests for all memory components
- Integration tests with mock AI responses
- Performance benchmarking and optimization

### Phase 4E: Privacy & UX (Week 7)
- Implement retention policies and auto-cleanup
- Add memory management API for Swift UI
- Implement PII scrubbing before storage
- Add app exclusion list feature

## Success Criteria

### Functional
- [ ] Memories are stored with correct context anchors (app + window + timestamp)
- [ ] Embeddings are generated locally within 100ms
- [ ] Vector search returns relevant memories within 50ms
- [ ] Retrieved context is correctly injected into LLM prompts
- [ ] Memory persists across app restarts
- [ ] Retention policy auto-deletes old memories

### Performance
- [ ] No noticeable latency added to request processing (<150ms total overhead)
- [ ] Database size grows linearly with memory count (~1KB per entry)
- [ ] Embedding model loads lazily and caches in memory

### Privacy
- [ ] Raw memory data never leaves the device
- [ ] PII is scrubbed before storage
- [ ] User can view/delete all memories via Settings UI
- [ ] Database file has correct permissions (600)

### UX
- [ ] Aether provides context-aware responses in same app/window
- [ ] No cross-contamination between different contexts
- [ ] Settings UI allows full memory management
- [ ] Clear feedback when memory is being used (optional indicator)

## Risks & Mitigations

### Risk 1: Performance Overhead
**Impact**: Memory operations slow down request processing
**Likelihood**: Medium
**Mitigation**:
- Async embedding generation (doesn't block response)
- Pre-built vector index for fast queries
- Lazy model loading to reduce startup time
- Performance benchmarks and optimization in Phase 4D

### Risk 2: Memory Bloat
**Impact**: Database grows too large, slows down system
**Likelihood**: Low (with retention policies)
**Mitigation**:
- Default 90-day retention policy
- Configurable auto-cleanup
- User-facing database size indicator in Settings
- Warn user if database exceeds threshold (e.g., 100MB)

### Risk 3: Context Capture Accuracy
**Impact**: Wrong app/window title captured, leading to incorrect context
**Likelihood**: Medium
**Mitigation**:
- Robust Accessibility API error handling
- Fallback to app_bundle_id only if window title unavailable
- Log context capture for debugging
- User can manually correct/delete incorrect memories

### Risk 4: Privacy Concerns
**Impact**: User worried about sensitive data being stored
**Likelihood**: Medium
**Mitigation**:
- Clear documentation on local-only storage
- Prominent "Clear All Memories" button
- App exclusion list (e.g., exclude banking apps)
- Optional: encrypt database at rest (future enhancement)

### Risk 5: Embedding Model Download/Licensing
**Impact**: Model unavailable or licensing issues
**Likelihood**: Low
**Mitigation**:
- Use Apache 2.0 licensed model (`all-MiniLM-L6-v2`)
- Bundle model with app (adds ~23MB to package)
- Fallback to simpler TF-IDF if embedding fails

## Open Questions

1. **Vector DB Choice**: LanceDB (native Rust, better performance) vs SQLite+vec (simpler, familiar)?
   - **Recommendation**: Start with SQLite+vec for simplicity, migrate to LanceDB if performance issues

2. **Embedding Model**: ONNX Runtime (simpler) vs Candle (pure Rust)?
   - **Recommendation**: ONNX Runtime for Phase 4, evaluate Candle in Phase 7 (optimization)

3. **Context Capture**: Swift-side vs Rust-side?
   - **Decision**: Swift-side (easier access to macOS APIs), pass via UniFFI

4. **Memory Indicator**: Should Halo show when memory is being used?
   - **Defer to UX testing**: Optional indicator in Settings

5. **Cross-Platform**: How to handle Windows/Linux context capture?
   - **Defer to Phase 7**: Focus on macOS first, design abstraction for future

## Related Changes

- **Depends On**: None (can be implemented independently)
- **Blocks**: None
- **Related**:
  - Phase 5 (AI Integration): Memory augmentation integrates with provider routing
  - Phase 6 (Settings UI): Memory management tab

## Spec Deltas

This change introduces the following new capabilities:

1. **memory-storage**: Store and retrieve interaction memories with context anchors
2. **embedding-inference**: Local embedding model inference for semantic search
3. **context-capture**: Capture active app and window context on macOS
4. **memory-augmentation**: Inject retrieved context into LLM prompts
5. **memory-privacy**: Privacy controls and retention policies

See `specs/*/spec.md` for detailed requirements.

## Appendix: Alternative Approaches Considered

### Alternative 1: Cloud-Based Memory
**Description**: Store memories in cloud service (e.g., Supabase, Firebase)
**Pros**: Sync across devices, infinite storage
**Cons**: Privacy concerns, latency, requires internet
**Decision**: Rejected - violates "local-first" principle

### Alternative 2: Simple Key-Value Storage
**Description**: Use simple text matching instead of embeddings
**Pros**: Much simpler, no ML dependencies
**Cons**: Poor recall for semantic queries, brittle
**Decision**: Rejected - semantic search is core value proposition

### Alternative 3: Server-Side Embedding
**Description**: Send text to OpenAI/Cohere for embeddings
**Pros**: No local model needed, better quality
**Cons**: Privacy concerns, latency, API costs
**Decision**: Rejected - violates "zero-knowledge cloud" principle

### Alternative 4: Full-Text Search Only
**Description**: Use SQLite FTS5 for keyword search
**Pros**: Built-in, fast, simple
**Cons**: No semantic understanding, keyword-dependent
**Decision**: Considered as fallback, but embeddings preferred
