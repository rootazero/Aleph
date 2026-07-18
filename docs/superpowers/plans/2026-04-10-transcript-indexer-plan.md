# TranscriptIndexer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement real transcript indexing so raw conversation fragments are embedded and searchable alongside compressed facts.

**Architecture:** Add `FactType::Transcript` enum variant. Implement `TranscriptIndexer.index_turn_text()` to embed and store conversation chunks. Wire it into `SessionCompactor.post_turn_compress()` after `store_raw_chunk()`. Exclude transcript facts from the compression pipeline.

**Tech Stack:** Rust, async_trait, rusqlite, serde

---

## Task 1: Add FactType::Transcript Variant

**Files:**
- Modify: `src/memory/context/enums.rs`

- [ ] **Step 1: Add Transcript variant to FactType enum**

In `src/memory/context/enums.rs`, add `Transcript` variant to the `FactType` enum (before `Other`):

```rust
    /// Conversation transcript chunk (embedded for direct retrieval)
    Transcript,
```

- [ ] **Step 2: Add as_str mapping**

In the `as_str()` match block, add:

```rust
            FactType::Transcript => "transcript",
```

- [ ] **Step 3: Add default_path mapping**

In the `default_path()` match block, add:

```rust
            FactType::Transcript => "aleph://transcript/",
```

- [ ] **Step 4: Add default_category mapping**

In the `default_category()` match block, add `FactType::Transcript` to the Events category. Find the `SubagentTranscript` line and add Transcript alongside it:

```rust
            FactType::SubagentTranscript | FactType::Transcript => MemoryCategory::Events,
```

- [ ] **Step 5: Add FromStr mapping**

In the `from_str()` match block, add:

```rust
            "transcript" => Ok(FactType::Transcript),
```

- [ ] **Step 6: Compile and test**

```bash
cargo check -p alephcore 2>&1 | head -10
cargo test -p alephcore --lib memory::context -- --test-threads=1 2>&1 | tail -10
cargo test -p alephcore --lib memory::proptest_enums 2>&1 | tail -10
```

- [ ] **Step 7: Commit**

```bash
git add src/memory/context/enums.rs
git commit -m "memory: add FactType::Transcript variant for conversation indexing

New fact type for embedded conversation transcript chunks. Path prefix
aleph://transcript/, category Events, tier ShortTerm by default."
```

---

## Task 2: Implement TranscriptIndexer.index_turn_text()

**Files:**
- Modify: `src/memory/transcript_indexer/indexer.rs`

- [ ] **Step 1: Read the current indexer.rs**

Read `src/memory/transcript_indexer/indexer.rs` to see the current stub implementation. Also read `src/memory/context/fact.rs` (or wherever `MemoryFact::new()` is defined) to understand the constructor.

- [ ] **Step 2: Store database and embedder references**

Change the struct and constructors to actually store the dependencies:

```rust
pub struct TranscriptIndexer {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: TranscriptIndexerConfig,
}

impl TranscriptIndexer {
    pub fn new(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            database,
            embedder,
            config: TranscriptIndexerConfig::default(),
        }
    }

    pub fn with_config(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: TranscriptIndexerConfig,
    ) -> Self {
        Self { database, embedder, config }
    }
```

- [ ] **Step 3: Implement index_turn_text**

Add the main indexing method:

```rust
    /// Index a conversation turn by embedding and storing as Transcript facts.
    ///
    /// Combines user input and AI output, chunks if over 800 tokens,
    /// embeds each chunk, and stores as FactType::Transcript MemoryFacts.
    /// Returns the IDs of created facts. Never fails the caller — logs
    /// warnings on embed/insert errors and skips failed chunks.
    pub async fn index_turn_text(
        &self,
        session_key: &str,
        seq: u32,
        user_input: &str,
        ai_output: &str,
        namespace: &str,
        agent: &str,
    ) -> Vec<String> {
        use crate::memory::context::{FactSource, FactType, MemoryFact, MemoryTier};
        use crate::memory::store::MemoryStore;

        let combined = if ai_output.is_empty() {
            user_input.to_string()
        } else {
            format!("[user]: {}\n\n[assistant]: {}", user_input, ai_output)
        };

        if combined.trim().is_empty() {
            return Vec::new();
        }

        // Chunk if over 800 tokens (estimate: 4 chars per token)
        let chunks = if self.estimate_tokens(&combined) > 800 {
            self.chunk_text(&combined)
        } else {
            vec![combined]
        };

        let mut fact_ids = Vec::new();

        for (i, chunk) in chunks.iter().enumerate() {
            // Embed
            let embedding = match self.embedder.embed(chunk).await {
                Ok(emb) => emb,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = session_key,
                        seq,
                        chunk_idx = i,
                        "Transcript embed failed, skipping chunk"
                    );
                    continue;
                }
            };

            // Build path: aleph://transcript/{session_key}/{seq} or {seq}_chunk{i}
            let path = if chunks.len() == 1 {
                format!("aleph://transcript/{}/{}", session_key, seq)
            } else {
                format!("aleph://transcript/{}/{}_chunk{}", session_key, seq, i)
            };

            // Construct fact
            let mut fact = MemoryFact::new(
                chunk.clone(),
                FactType::Transcript,
                Vec::new(), // no source memory IDs
            );
            fact.path = path;
            fact.parent_path = format!("aleph://transcript/{}", session_key);
            fact.embedding = Some(embedding);
            fact.embedding_model = self.embedder.model_name().to_string();
            fact.fact_source = FactSource::Extracted;
            fact.tier = MemoryTier::ShortTerm;
            fact.namespace = namespace.to_string();
            fact.agent = agent.to_string();

            // Insert
            match self.database.insert_fact(&fact).await {
                Ok(()) => {
                    fact_ids.push(fact.id.clone());
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = session_key,
                        seq,
                        chunk_idx = i,
                        "Transcript fact insert failed, skipping"
                    );
                }
            }
        }

        if !fact_ids.is_empty() {
            tracing::debug!(
                session = session_key,
                seq,
                facts = fact_ids.len(),
                "Transcript chunks indexed"
            );
        }

        fact_ids
    }
```

- [ ] **Step 4: Keep existing index_turn as backward-compatible wrapper**

Update the existing `index_turn` to call `index_turn_text` if needed, or leave it as a no-op with a deprecation note. The important thing is not to break existing callers:

```rust
    /// Legacy no-op — use index_turn_text() for real indexing.
    pub async fn index_turn(&self, _memory_id: &str) -> crate::Result<()> {
        Ok(())
    }

    /// Legacy stub — use index_turn_text() for real indexing.
    pub async fn index_with_chunking(&self, memory_id: &str) -> crate::Result<Vec<String>> {
        Ok(vec![memory_id.to_string()])
    }
```

- [ ] **Step 5: Compile**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 6: Run existing indexer tests**

```bash
cargo test -p alephcore --lib memory::transcript_indexer -- --test-threads=1 2>&1 | tail -15
```

Existing tests use `TranscriptIndexer::new(db, embedder)` and `with_config(db, embedder, config)` — they should still compile since the constructor signatures haven't changed (only the underscore prefixes are removed from the stored fields).

- [ ] **Step 7: Commit**

```bash
git add src/memory/transcript_indexer/indexer.rs
git commit -m "memory: implement TranscriptIndexer.index_turn_text()

Real implementation replacing the no-op stub. Combines user+AI text,
chunks if >800 tokens, embeds each chunk, stores as FactType::Transcript
MemoryFacts at aleph://transcript/{session}/{seq}. Graceful degradation
on embed/insert failures."
```

---

## Task 3: Wire TranscriptIndexer into SessionCompactor

**Files:**
- Modify: `src/memory/session_compactor/mod.rs`

- [ ] **Step 1: Read SessionCompactor struct and constructor**

Read `src/memory/session_compactor/mod.rs` lines 119-135 to see the current struct fields and `new()` constructor. Also read `post_turn_compress()` starting at line 314.

- [ ] **Step 2: Add embedder and indexer fields to SessionCompactor**

Add fields to the struct:

```rust
pub struct SessionCompactor {
    database: MemoryBackend,
    provider: Option<Arc<dyn AiProvider>>,
    config: SessionCompactorConfig,
    metrics: Arc<CompactorMetrics>,
    indexer: Option<crate::memory::transcript_indexer::TranscriptIndexer>,
}
```

- [ ] **Step 3: Add with_embedder builder method**

```rust
    /// Attach an embedding provider for transcript indexing.
    ///
    /// When set, each compressed turn is also embedded and stored as
    /// a Transcript fact for direct conversation retrieval.
    pub fn with_embedder(mut self, embedder: Arc<dyn crate::memory::EmbeddingProvider>) -> Self {
        self.indexer = Some(crate::memory::transcript_indexer::TranscriptIndexer::new(
            self.database.clone(),
            embedder,
        ));
        self
    }
```

Initialize `indexer: None` in the existing `new()` constructor.

- [ ] **Step 4: Wire into post_turn_compress**

In `post_turn_compress()`, after the `store_raw_chunk()` call (around line 433-434, inside the loop after `d0_created += 1`), add transcript indexing:

```rust
                // Index raw content as transcript for direct retrieval
                if let Some(ref indexer) = self.indexer {
                    let ns = "owner"; // SessionCompactor operates in owner namespace
                    indexer.index_turn_text(
                        &session_id,
                        next_seq,
                        &raw_content,
                        "", // AI output already included in raw_content
                        ns,
                        &agent_id,
                    ).await;
                    // index_turn_text never fails — logs warnings internally
                }
```

Important: `raw_content` at line 428-432 already combines `[user]: ... [assistant]: ...` pairs. So we pass it as user_input with empty ai_output to avoid double-formatting.

- [ ] **Step 5: Compile**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 6: Find where SessionCompactor is constructed and wire embedder**

Search for `SessionCompactor::new` in the codebase:
```bash
grep -rn "SessionCompactor::new" src/ --include="*.rs"
```

At each production construction site where an embedder is available, chain `.with_embedder(embedder.clone())`. If no embedder is available at construction, leave as-is — the indexer will be None and transcript indexing is silently skipped.

- [ ] **Step 7: Compile and test**

```bash
cargo check -p alephcore 2>&1 | head -10
cargo test -p alephcore --lib memory::session_compactor -- --test-threads=1 2>&1 | tail -10
```

- [ ] **Step 8: Commit**

```bash
git add src/memory/session_compactor/mod.rs
git commit -m "memory: wire TranscriptIndexer into SessionCompactor.post_turn_compress

Each compressed turn is now also embedded and stored as a Transcript
fact for direct conversation retrieval. Indexer attached via
with_embedder() builder. Gracefully skipped when no embedder available."
```

---

## Task 4: Exclude Transcript Facts from Compression Pipeline

**Files:**
- Modify: `src/memory/compression/service.rs`

- [ ] **Step 1: Read compress_in_workspace around line 170-200**

Read `src/memory/compression/service.rs` to see how `get_uncompressed_session_facts()` is called and how the results are filtered before passing to the extractor.

- [ ] **Step 2: Filter out Transcript facts**

After `raw_facts` is fetched (around line 174-185), add a filter to exclude Transcript facts:

```rust
        let raw_facts: Vec<_> = raw_facts
            .into_iter()
            .filter(|f| f.fact_type != crate::memory::context::FactType::Transcript)
            .collect();
```

This ensures transcript chunks are never re-compressed into structured facts.

Alternative: if `get_uncompressed_session_facts` accepts a filter parameter, add the exclusion there. Check the method signature — if it only takes timestamp and workspace, do the filter in Rust as shown above.

- [ ] **Step 3: Compile and test**

```bash
cargo check -p alephcore 2>&1 | head -10
cargo test -p alephcore --lib memory::compression -- --test-threads=1 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add src/memory/compression/service.rs
git commit -m "memory: exclude Transcript facts from compression pipeline

Transcript chunks are raw conversation data stored for direct retrieval.
They should not be re-compressed into structured facts by the
CompressionService."
```

---

## Task 5: Wire Embedder at SessionCompactor Construction Sites

**Files:**
- Modify: wherever `SessionCompactor::new()` is called in production code

- [ ] **Step 1: Find all construction sites**

```bash
grep -rn "SessionCompactor::new" src/ --include="*.rs"
```

- [ ] **Step 2: Wire embedder at each production site**

For each construction site that has access to an embedder (`Arc<dyn EmbeddingProvider>`), chain `.with_embedder(embedder.clone())`:

```rust
// Before:
let compactor = SessionCompactor::new(database.clone(), compactor_config);

// After:
let compactor = SessionCompactor::new(database.clone(), compactor_config)
    .with_embedder(embedder.clone());
```

Leave test construction sites unchanged — they'll use `indexer: None` (no transcript indexing).

- [ ] **Step 3: Compile**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "memory: pass embedder to SessionCompactor at production construction sites

Enables transcript indexing in production. Test sites use default
(no embedder, transcript indexing silently skipped)."
```

---

## Task 6: Final Verification

- [ ] **Step 1: Full compilation**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 2: Run all memory tests**

```bash
cargo test -p alephcore --lib memory:: -- --test-threads=1 2>&1 | tail -10
```

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | grep "error\[" | head -10
```

- [ ] **Step 4: Verify the full data path**

```bash
grep -n "index_turn_text\|FactType::Transcript\|with_embedder" src/memory/transcript_indexer/indexer.rs src/memory/session_compactor/mod.rs src/memory/compression/service.rs src/memory/context/enums.rs | head -20
```

Expected: index_turn_text defined and called, FactType::Transcript in enums, with_embedder in compactor, Transcript excluded from compression.

- [ ] **Step 5: Commit if clippy fixes needed**

```bash
git add -A
git commit -m "memory: fix clippy warnings after transcript indexer implementation"
```
