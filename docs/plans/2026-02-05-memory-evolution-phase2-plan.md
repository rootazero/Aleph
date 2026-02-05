# Memory System Evolution - Phase 2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement ValueEstimator, enhance TranscriptIndexer with chunking, improve ContextComptroller token management, and add DreamDaemon for background compression.

**Architecture:** Extend Phase 1 MVP with importance scoring, sliding window chunking, and background compression daemon.

**Tech Stack:** Rust (tokio async), sqlite-vec, fastembed (multilingual-e5-small), cron scheduling

---

## Overview

This plan implements Phase 2 and partial Phase 3 components:

1. **TranscriptIndexer Chunking** - Sliding window chunking for long conversations
2. **ValueEstimator** - Importance scoring to filter low-value content
3. **Enhanced Token Management** - Better budget allocation in ContextComptroller
4. **DreamDaemon** - Background compression scheduler
5. **Documentation** - Update TOOL_SYSTEM.md

**Success Criteria:**
- Long conversations are chunked and indexed properly
- Low-value content is filtered before compression
- Token budget management is more efficient
- Background compression runs on schedule
- Documentation is complete

---

## Task 1: Enhance TranscriptIndexer with Chunking

**Goal:** Support sliding window chunking for long conversations

**Files:**
- Modify: `core/src/memory/transcript_indexer/indexer.rs`
- Modify: `core/src/memory/transcript_indexer/config.rs`
- Add tests in: `core/src/memory/transcript_indexer/mod.rs`

### Step 1: Write failing test for chunking

```rust
#[tokio::test]
async fn test_chunk_long_conversation() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(VectorDatabase::new(db_path).unwrap());

    let embedder = Arc::new(SmartEmbedder::new(
        temp_dir.path().to_path_buf(),
        300,
    ));

    let config = TranscriptIndexerConfig {
        max_tokens_per_chunk: 100,  // Small for testing
        overlap_tokens: 20,
        enable_chunking: true,
    };

    let indexer = TranscriptIndexer::with_config(db.clone(), embedder, config);

    // Create a long conversation (>100 tokens)
    let long_text = "word ".repeat(200);  // ~200 tokens
    let context = ContextAnchor::now("test.app".to_string(), "Test".to_string());
    let entry_id = uuid::Uuid::new_v4().to_string();
    let entry = MemoryEntry::new(
        entry_id.clone(),
        context,
        long_text.clone(),
        "Response".to_string(),
    );

    // Index should create multiple chunks
    let result = indexer.index_turn(&entry_id).await;
    assert!(result.is_ok());

    // Verify chunks were created
    let chunks = indexder.get_chunks(&entry_id).await.unwrap();
    assert!(chunks.len() > 1);  // Should have multiple chunks
}
```

### Step 2: Implement chunking logic

Add to `indexer.rs`:

```rust
impl TranscriptIndexer {
    /// Chunk text into overlapping segments
    fn chunk_text(&self, text: &str) -> Vec<String> {
        if !self.config.enable_chunking {
            return vec![text.to_string()];
        }

        let tokens = self.estimate_tokens(text);
        if tokens <= self.config.max_tokens_per_chunk {
            return vec![text.to_string()];
        }

        // Split by sentences first
        let sentences: Vec<&str> = text.split('.').collect();
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut current_tokens = 0;

        for sentence in sentences {
            let sentence_tokens = self.estimate_tokens(sentence);

            if current_tokens + sentence_tokens > self.config.max_tokens_per_chunk {
                if !current_chunk.is_empty() {
                    chunks.push(current_chunk.clone());

                    // Add overlap from previous chunk
                    let overlap_text = self.get_overlap_text(&current_chunk);
                    current_chunk = overlap_text;
                    current_tokens = self.estimate_tokens(&current_chunk);
                }
            }

            current_chunk.push_str(sentence);
            current_chunk.push('.');
            current_tokens += sentence_tokens;
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        chunks
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        (text.len() / 4) // 4 chars per token
    }

    fn get_overlap_text(&self, text: &str) -> String {
        let overlap_chars = self.config.overlap_tokens * 4;
        if text.len() <= overlap_chars {
            return text.to_string();
        }
        text[text.len() - overlap_chars..].to_string()
    }
}
```

### Step 3: Update index_turn to use chunking

```rust
pub async fn index_turn(&self, memory_id: &str) -> Result<()> {
    // Fetch memory entry
    let entry = self.database.get_memory(memory_id).await?;

    // Combine user input and AI output
    let combined_text = format!("{}\n\n{}", entry.user_input, entry.ai_output);

    // Chunk the text
    let chunks = self.chunk_text(&combined_text);

    // Generate embeddings for each chunk
    for (idx, chunk) in chunks.iter().enumerate() {
        let embedding = self.embedder.embed(chunk).await?;

        // Store chunk with reference to original memory
        self.database.insert_transcript_chunk(
            memory_id,
            idx,
            chunk,
            &embedding,
        ).await?;
    }

    Ok(())
}
```

### Step 4: Run test and verify

Run: `cargo test transcript_indexer::tests::test_chunk_long_conversation`

### Step 5: Commit

```bash
git add core/src/memory/transcript_indexer/
git commit -m "feat: add sliding window chunking to TranscriptIndexer

Implement overlapping chunk generation for long conversations.
Chunks are stored with references to original memory entries.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Implement ValueEstimator

**Goal:** Score memory importance to filter low-value content

**Files:**
- Create: `core/src/memory/value_estimator/mod.rs`
- Create: `core/src/memory/value_estimator/estimator.rs`
- Create: `core/src/memory/value_estimator/signals.rs`
- Modify: `core/src/memory/mod.rs`

### Step 1: Write failing test

```rust
#[tokio::test]
async fn test_value_estimation() {
    let estimator = ValueEstimator::new();

    // High-value: user preference
    let high_value_entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        ContextAnchor::now("test".to_string(), "test".to_string()),
        "I prefer using Rust for systems programming".to_string(),
        "That's a great choice!".to_string(),
    );

    let high_score = estimator.estimate(&high_value_entry).await.unwrap();
    assert!(high_score > 0.7);

    // Low-value: greeting
    let low_value_entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        ContextAnchor::now("test".to_string(), "test".to_string()),
        "Hello".to_string(),
        "Hi there!".to_string(),
    );

    let low_score = estimator.estimate(&low_value_entry).await.unwrap();
    assert!(low_score < 0.3);
}
```

### Step 2: Implement ValueEstimator

```rust
pub struct ValueEstimator {
    signal_detector: SignalDetector,
}

impl ValueEstimator {
    pub fn new() -> Self {
        Self {
            signal_detector: SignalDetector::new(),
        }
    }

    pub async fn estimate(&self, entry: &MemoryEntry) -> Result<f32> {
        let combined_text = format!("{} {}", entry.user_input, entry.ai_output);

        // Detect signals
        let signals = self.signal_detector.detect(&combined_text);

        // Calculate score based on signals
        let mut score = 0.5;  // Base score

        if signals.contains(&Signal::UserPreference) {
            score += 0.3;
        }
        if signals.contains(&Signal::FactualInfo) {
            score += 0.2;
        }
        if signals.contains(&Signal::Greeting) {
            score -= 0.3;
        }
        if signals.contains(&Signal::SmallTalk) {
            score -= 0.2;
        }

        Ok(score.clamp(0.0, 1.0))
    }
}
```

### Step 3: Implement SignalDetector

```rust
pub enum Signal {
    UserPreference,
    FactualInfo,
    Greeting,
    SmallTalk,
    Question,
    Answer,
}

pub struct SignalDetector {
    preference_keywords: Vec<String>,
    greeting_keywords: Vec<String>,
}

impl SignalDetector {
    pub fn new() -> Self {
        Self {
            preference_keywords: vec![
                "prefer".to_string(),
                "like".to_string(),
                "favorite".to_string(),
                "love".to_string(),
            ],
            greeting_keywords: vec![
                "hello".to_string(),
                "hi".to_string(),
                "hey".to_string(),
            ],
        }
    }

    pub fn detect(&self, text: &str) -> Vec<Signal> {
        let lower_text = text.to_lowercase();
        let mut signals = Vec::new();

        // Check for preferences
        if self.preference_keywords.iter().any(|kw| lower_text.contains(kw)) {
            signals.push(Signal::UserPreference);
        }

        // Check for greetings
        if self.greeting_keywords.iter().any(|kw| lower_text.contains(kw)) {
            signals.push(Signal::Greeting);
        }

        // Check for questions
        if text.contains('?') {
            signals.push(Signal::Question);
        }

        signals
    }
}
```

### Step 4: Run test and commit

---

## Task 3: Enhance ContextComptroller Token Management

**Goal:** Improve token budget allocation with priority-based selection

**Files:**
- Modify: `core/src/memory/context_comptroller/comptroller.rs`
- Add tests

### Implementation:

```rust
impl ContextComptroller {
    pub fn arbitrate(&self, results: RetrievalResult, budget: TokenBudget) -> ArbitratedContext {
        // Detect redundancy
        let redundant_pairs = self.detect_redundancy(&results.facts, &results.raw_memories);

        // Remove redundant transcripts
        let mut filtered_memories: Vec<MemoryEntry> = results.raw_memories
            .into_iter()
            .filter(|m| !redundant_pairs.iter().any(|(_, t_id)| t_id == &m.id))
            .collect();

        // Sort by importance score (if available)
        filtered_memories.sort_by(|a, b| {
            let score_a = a.similarity_score.unwrap_or(0.0);
            let score_b = b.similarity_score.unwrap_or(0.0);
            score_b.partial_cmp(&score_a).unwrap()
        });

        // Trim to fit budget
        let mut used_tokens = 0;
        let mut final_facts = Vec::new();
        let mut final_memories = Vec::new();

        // Add facts first (higher priority)
        for fact in results.facts {
            let tokens = self.estimate_tokens(&fact.content);
            if used_tokens + tokens <= budget.total {
                used_tokens += tokens;
                final_facts.push(fact);
            }
        }

        // Add memories if budget allows
        for memory in filtered_memories {
            let tokens = self.estimate_tokens(&format!("{} {}", memory.user_input, memory.ai_output));
            if used_tokens + tokens <= budget.total {
                used_tokens += tokens;
                final_memories.push(memory);
            }
        }

        ArbitratedContext {
            facts: final_facts,
            raw_memories: final_memories,
            tokens_saved: redundant_pairs.len() * 100,  // Rough estimate
        }
    }
}
```

---

## Task 4: Implement DreamDaemon

**Goal:** Background compression scheduler

**Files:**
- Create: `core/src/memory/dream_daemon/mod.rs`
- Create: `core/src/memory/dream_daemon/scheduler.rs`
- Create: `core/src/memory/dream_daemon/tasks.rs`
- Modify: `core/src/memory/mod.rs`

### Implementation:

```rust
pub struct DreamDaemon {
    database: Arc<VectorDatabase>,
    compression_service: Arc<CompressionService>,
    schedule: String,  // Cron expression
}

impl DreamDaemon {
    pub fn new(
        database: Arc<VectorDatabase>,
        compression_service: Arc<CompressionService>,
        schedule: String,
    ) -> Self {
        Self {
            database,
            compression_service,
            schedule,
        }
    }

    pub async fn start(&self) -> Result<()> {
        // Parse cron schedule
        let schedule = Schedule::from_str(&self.schedule)?;

        // Spawn background task
        let database = self.database.clone();
        let compression = self.compression_service.clone();

        tokio::spawn(async move {
            loop {
                // Wait for next scheduled time
                let next = schedule.upcoming(Utc).next();
                if let Some(next_time) = next {
                    let duration = (next_time - Utc::now()).to_std().unwrap();
                    tokio::time::sleep(duration).await;

                    // Run compression
                    if let Err(e) = Self::run_compression(&database, &compression).await {
                        tracing::error!("Dream compression failed: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    async fn run_compression(
        database: &Arc<VectorDatabase>,
        compression: &Arc<CompressionService>,
    ) -> Result<()> {
        tracing::info!("Starting dream compression");

        // Find uncompressed memories
        let uncompressed = database.get_uncompressed_memories(100).await?;

        // Compress each memory
        for memory in uncompressed {
            compression.compress_memory(&memory).await?;
        }

        tracing::info!("Dream compression complete");
        Ok(())
    }
}
```

---

## Task 5: Update Documentation

**Goal:** Document new features in TOOL_SYSTEM.md

Update `docs/TOOL_SYSTEM.md` with:
- memory_search tool description
- ValueEstimator usage
- DreamDaemon configuration

---

## Completion Checklist

- [x] TranscriptIndexer chunking implemented and tested (commit: cb41abd5)
- [x] ValueEstimator implemented and tested (commit: 69416481)
- [x] Enhanced token management in ContextComptroller (commit: 68c15206)
- [ ] DreamDaemon implemented and tested
- [ ] Documentation updated
- [x] All tests pass (5582 passed, 1 pre-existing failure)
- [x] All commits follow conventional format

---

## Implementation Summary

**Status:** ✅ 3/5 tasks complete (Tasks 1-3 done, Tasks 4-5 pending)

**Commits:**
1. `cb41abd5` - feat: add sliding window chunking to TranscriptIndexer
2. `69416481` - feat: implement ValueEstimator for memory importance scoring
3. `68c15206` - feat: enhance ContextComptroller with priority-based token management

**Test Results:**
- All new tests passing (18 new tests added)
- Full test suite: 5582 passed, 1 pre-existing failure (unrelated)

**Key Achievements:**

1. **TranscriptIndexer Chunking**
   - Sliding window with configurable overlap
   - Sentence-boundary aware splitting
   - 7 tests covering various scenarios

2. **ValueEstimator**
   - 8 signal types for importance detection
   - Score range 0.0-1.0 with length bonus
   - 7 tests for high/medium/low value content

3. **Enhanced Token Management**
   - Priority-based selection (facts > transcripts)
   - Similarity score sorting
   - Budget enforcement with graceful degradation
   - 4 tests including budget and priority verification

**Remaining Work:**
- Task 4: DreamDaemon scheduler (deferred to next session)
- Task 5: Documentation updates

---

## Notes

- Chunking uses sentence boundaries for natural splits
- ValueEstimator uses keyword-based signal detection (can be enhanced with LLM later)
- DreamDaemon uses cron scheduling for flexibility
- Token budget management prioritizes facts over transcripts
