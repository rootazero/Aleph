# TranscriptIndexer — Complement Implementation & Wiring

> Implement real transcript indexing so raw conversation fragments are directly searchable alongside compressed facts.

**Date**: 2026-04-10
**Scope**: `src/memory/transcript_indexer/indexer.rs`, `src/memory/session_compactor/mod.rs`, `src/memory/context/enums.rs`, `src/memory/compression/service.rs`

---

## Problem

TranscriptIndexer is an empty shell — constructor discards database/embedder references, `index_turn()` is a no-op. Users cannot retrieve original conversation fragments; only compressed facts are searchable. When a user asks "what did I say about Rust last week?", the system returns compressed facts like "user prefers Rust" but cannot surface the actual conversation context.

## Solution

Implement `TranscriptIndexer.index_turn_text()` to embed conversation text and store as `FactType::Transcript` MemoryFacts. Call it from `SessionCompactor.post_turn_compress()` on every turn. No retrieval path changes needed — HybridRetrieval automatically covers the new facts.

## Data Flow

```
SessionCompactor.post_turn_compress(user_input, ai_output)
  → existing: store_raw_chunk() [aleph://session/{id}/raw/{seq}]
  → NEW: indexer.index_turn_text(session_key, seq, user_input, ai_output)
    → combine text: "{user_input}\n\n{ai_output}"
    → if tokens > 800: chunk_text() splits into overlapping chunks
    → else: single chunk
    → for each chunk:
      → embedder.embed(chunk)
      → construct MemoryFact {
          fact_type: FactType::Transcript,
          content: chunk,
          path: "aleph://transcript/{session_key}/{seq}" (or "{seq}_chunk{n}" if split),
          embedding: Some(embedding_vec),
          tier: ShortTerm,
          fact_source: FactSource::Extracted,
          namespace, agent: inherited from session context,
        }
      → database.insert_fact(fact)
    → return Vec<fact_id>
```

## Changes

### 1. Add FactType::Transcript variant

**File**: `src/memory/context/enums.rs`

Add `Transcript` variant to the `FactType` enum:
- `as_str()` → `"transcript"`
- `default_path()` → `"aleph://transcript/"`
- `category()` → `MemoryCategory::Events`
- `from_str()` → `"transcript" => Ok(FactType::Transcript)`

### 2. Implement TranscriptIndexer

**File**: `src/memory/transcript_indexer/indexer.rs`

- Store `database: MemoryBackend` and `embedder: Arc<dyn EmbeddingProvider>` (remove underscore prefixes)
- Implement `index_turn_text()`:

```rust
pub async fn index_turn_text(
    &self,
    session_key: &str,
    seq: u32,
    user_input: &str,
    ai_output: &str,
    namespace: &str,
    agent: &str,
) -> Result<Vec<String>>
```

Logic:
1. Combine `user_input` and `ai_output` into a single text
2. If > 800 estimated tokens, call existing `self.chunk_text(&combined)` to split
3. For each chunk, call `self.embedder.embed(&chunk).await`
4. Construct `MemoryFact` with:
   - `fact_type: FactType::Transcript`
   - `path: format!("aleph://transcript/{session_key}/{seq}")` (append `_chunk{n}` if multiple)
   - `tier: MemoryTier::ShortTerm`
   - `fact_source: FactSource::Extracted`
   - `embedding: Some(embedding_vec)`
   - `namespace`, `agent` from parameters
5. Call `self.database.insert_fact(&fact).await`
6. Return vector of created fact IDs

Error handling: log warning on embed/insert failure, skip that chunk, continue with others. Never fail the whole operation.

### 3. Wire into SessionCompactor

**File**: `src/memory/session_compactor/mod.rs`

- Add `indexer: Option<TranscriptIndexer>` field to `SessionCompactor` struct
- In constructor, create `TranscriptIndexer` when embedder is available
- In `post_turn_compress()`, after existing `store_raw_chunk()` call, add:

```rust
if let Some(ref indexer) = self.indexer {
    if let Err(e) = indexer.index_turn_text(
        session_key, seq, user_input, ai_output, namespace, agent
    ).await {
        tracing::warn!(error = %e, "Transcript indexing failed, continuing");
    }
}
```

The indexer must NOT block the main compactor flow — fire-and-forget with warning on error.

### 4. Exclude Transcript facts from compression

**File**: `src/memory/compression/service.rs`

In `compress_in_workspace()`, where uncompressed facts are queried for compression, add a filter to exclude `FactType::Transcript`. Transcript chunks are raw conversation data — they should not be re-compressed into structured facts.

Find where `get_uncompressed_session_facts()` or similar is called and ensure transcript facts are filtered out (either by adding a `fact_type != 'transcript'` filter to the query, or filtering in Rust after retrieval).

## Error Handling

| Failure | Behavior |
|---------|----------|
| Embedding API fails | Log warning, skip this chunk, continue |
| Database insert fails | Log warning, skip this chunk, continue |
| All chunks fail | Log warning, return empty Vec — no transcript indexed for this turn |
| SessionCompactor.post_turn_compress continues | Always — transcript indexing never blocks |

## Testing

- Existing `chunk_text()` tests remain valid
- New unit test: `index_turn_text` with mock embedder verifies facts are inserted with correct FactType::Transcript and path format
- Verify `FactType::Transcript` roundtrips through `as_str()` / `from_str()`
- Verify compression service skips Transcript facts

## Out of Scope

- SemanticChunker integration (future upgrade path)
- Configurable token threshold (hardcode 800 for now)
- Transcript-specific retrieval UI formatting (HybridRetrieval handles it)
- Transcript decay/cleanup policy (uses default ShortTerm decay)
