# Raw Memory (L0)

> Short-lived, high-volume conversation and attachment data consumed by the compression pipeline and session-context restore.

## 1. Role

Raw memory is the L0 **ephemeral** layer of Aleph's memory stack. Every conversation turn, every session-compaction summary (d0/d1/d2), and every attachment text extraction lands here first as a row in the SQLite `raw_memories` table. Rows are short-lived: once the `CompressionService` has folded a batch of rows into Notes (L1), the rows are flipped to `is_processed = 1` and no longer participate in downstream distillation. They remain queryable by path prefix so session-scoped tools (`recall_context`, `SessionSummarySource`) can still reach them for as long as the row is in the database.

Everything here is **structured, indexed SQLite** — not a pile of markdown files. The anti-example is a per-turn file on disk: that burns filesystem I/O, fragments the agent's memory, and forces the compression pipeline to re-parse human-unreadable noise. A single append-only SQLite table with a compound `(is_processed, created_at)` partial index gives the compactor an O(1) batch-fetch and lets raw data age out cleanly.

## 2. When to Use raw_memories vs Notes

| Data kind                              | Storage        | Why |
|----------------------------------------|----------------|-----|
| Conversation turn transcripts          | `raw_memories` | High volume, ephemeral, consumed by `TranscriptIndexer` and compression. |
| Session d0/d1/d2 hierarchical summaries | `raw_memories` | Session-scoped; read back by `SessionSummarySource` for zero-cost compaction. |
| Attachment extracted text (PDF/Word/image OCR) | `raw_memories` (`attachment_text` column) | Travels with its originating turn; consumed by compression. |
| Tool outputs that should feed compression | `raw_memories` | `RawMemorySource::ToolOutput`; optional input to Note extraction. |
| User preferences, stable decisions     | Notes (L1)     | Long-lived; belong in versioned, human-readable markdown. |
| Synthesized insights / distilled facts | Notes (L1)     | Output of `CompressionService`; survives `raw_memories` cleanup. |

The rule of thumb: **if the row only has to live long enough to be compressed, it belongs in `raw_memories`**. Anything a human might want to read directly a week later belongs in Notes.

## 3. `raw_memories` Schema

Verbatim from `src/memory/store/sqlite/schema.rs` (`CREATE_RAW_MEMORIES`):

```sql
CREATE TABLE IF NOT EXISTS raw_memories (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    source          TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    session_id      TEXT,
    path            TEXT,
    layer           TEXT,
    attachment_text TEXT,
    is_processed    INTEGER DEFAULT 0,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_raw_unprocessed ON raw_memories(is_processed, created_at)
    WHERE is_processed = 0;
CREATE INDEX IF NOT EXISTS idx_raw_agent ON raw_memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_raw_session ON raw_memories(session_id);
```

Column notes:

- `id` — UUID string, primary key.
- `content` — the raw text body (transcript turn, d-summary, tool output).
- `source` — discriminator matching `RawMemorySource` (`session_compressed` | `transcript` | `tool_output` | `attachment`).
- `agent_id` — agent/workspace scope; `CompressionService` consumes one agent at a time.
- `session_id` — nullable; populated for transcript and session-compressed rows.
- `path` — VFS-style traceability pointer (see §6 for path conventions).
- `layer` — reserved column (currently unused by writers; summary depth is encoded inside `path` as `d0`/`d1`/`d2`).
- `attachment_text` — extracted text from file attachments, injected into the prompt during compression.
- `is_processed` — `0` = pending compression, `1` = consumed. The partial index only covers `is_processed = 0`, keeping the pending-work query cheap as the table grows.
- `created_at` — Unix epoch seconds.

## 4. `RawMemory` + `RawMemorySource`

From `src/memory/store/raw_memory.rs`:

```rust
/// Source of raw memory data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawMemorySource {
    SessionCompressed,
    Transcript,
    ToolOutput,
    Attachment,
}

/// A raw memory record — ephemeral data consumed by CompressionService.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    pub content: String,
    pub source: RawMemorySource,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub path: Option<String>,
    pub layer: Option<String>,
    pub attachment_text: Option<String>,
    pub is_processed: bool,
    pub created_at: i64,
}
```

`RawMemory::new(content, source)` defaults `agent_id = "default"`, generates a UUID, and stamps `created_at`. Builder methods `with_agent`, `with_session`, `with_path`, `with_layer`, and `with_attachment_text` decorate the record before insertion. `RawMemorySource::as_str` / `from_str` map the enum to/from the on-disk `source` column.

## 5. `RawMemoryStore` Trait

Trait defined in `src/memory/store/raw_memory.rs`; implemented by the SQLite backend in `src/memory/store/sqlite/raw_memories.rs`.

| Method | Purpose |
|---|---|
| `async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError>` | Persist a single record. Used by every writer in §6. |
| `async fn get_unprocessed_raw_memories(&self, agent_id: &str, limit: usize) -> Result<Vec<RawMemory>, AlephError>` | Batch-fetch pending rows, ordered by `created_at ASC`. Backed by the `idx_raw_unprocessed` partial index. |
| `async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError>` | Flip rows to `is_processed = 1` after `CompressionService` consumes them. Returns affected row count. |
| `async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError>` | Backpressure / scheduling signal for compression triggers. |
| `async fn get_raw_by_path_prefix(&self, path_prefix: &str, agent_id: &str, limit: usize) -> Result<Vec<RawMemory>, AlephError>` | Path-scoped lookup used by session-context tooling (e.g. `aleph://session/{id}/`). |

## 6. Writers

### 6.1 SessionCompactor

`src/memory/session_compactor/mod.rs` owns the hierarchical session summary pipeline. On every post-turn tick it semantic-chunks compressible messages, generates a d0 (leaf) summary per chunk, then condenses d0→d1 when count ≥ `d1_min_fanout` (default 4) and d1→d2 when count ≥ `d2_min_fanout` (default 3). Each summary is written with `RawMemorySource::SessionCompressed` at path `aleph://session/{session_id}/d{depth}/{seq}`. In addition, `store_raw_chunk` writes the verbatim pre-compression conversation chunk at `aleph://session/{session_id}/raw/{seq}` (also `SessionCompressed`) so `recall_context` can recover the originals after the context window has been replaced.

Relevant config fields on `SessionCompactorConfig`: `d1_min_fanout = 4`, `d2_min_fanout = 3`, `session_fact_retention_hours = 24`, plus `max_summary_depth` (gates the d1→d2 step).

### 6.2 TranscriptIndexer

`src/memory/transcript_indexer/indexer.rs` handles near-realtime per-turn indexing. `index_turn_text(session_key, seq, user_input, ai_output, namespace, agent)` combines the `[user]` / `[assistant]` pair, chunks it through `chunk_text` when estimated tokens exceed 800, and inserts one `RawMemory` per chunk at path `aleph://transcript/{session_key}/{seq}` (or `…_chunk{i}` for multi-chunk turns). `source = "transcript"`.

Config keys on `TranscriptIndexerConfig` (`src/memory/transcript_indexer/config.rs`): `max_tokens_per_chunk = 400`, `overlap_tokens = 80`, `enable_chunking = true`. A separate `SemanticChunkConfig` (`similarity_threshold = 0.85`, `min_chunk_size = 50`, `max_chunk_size = 400`) governs the semantic-boundary chunker when embeddings are available.

### 6.3 Gateway Media Pipeline (attachment_text)

The `attachment_text` column and the `RawMemory::with_attachment_text` builder exist so that extracted text from PDF / Word / image OCR travels alongside its originating turn rather than becoming a standalone row. The consumer is `CompressionService`, which in `src/memory/compression/service.rs` reads `raw.attachment_text` and injects up to 2000 characters of it into the per-memory prompt as `"{user_input}\n[Attachment]: {att_preview}"`. `RawMemorySource::Attachment` is reserved for the case of attachment-only rows (e.g. an uploaded document with no accompanying turn). As of this writing the production gateway write sites that populate `attachment_text` during normal message ingestion are not present in `src/gateway/`; the field is wired end-to-end through schema, store, and compression, and is exercised in `src/memory/compression/service.rs` tests (`with_attachment_text(...)`).

## 7. Readers

### 7.1 CompressionService

`src/memory/compression/service.rs` drives the L0→L1 distillation. Per workspace tick it calls `get_unprocessed_raw_memories(workspace_id, batch_size)`, filters out `RawMemorySource::Transcript` rows (transcripts are already chunk-indexed; they do not need a second distillation pass), converts each remaining row into a `MemoryEntry` — injecting `attachment_text` when present — runs note extraction, writes/updates markdown notes under `<memory_dir>/<workspace>/<category>/<file>.md`, and finally calls `mark_raw_as_processed(consumed_ids)`. The latest `created_at` of the consumed batch becomes the new `last_compression_timestamp`.

### 7.2 recall_context

`src/builtin_tools/recall_context.rs` is the LLM-facing tool that lets the model recover pre-compression conversation details. `RecallContextTool::call_impl` builds the path prefix `aleph://session/{session_id}/raw/` and calls `get_raw_by_path_prefix(prefix, "default", args.max_results)`. Each returned `RawMemory` becomes a `RecalledFragment { content, relevance_score: 1.0, source_path: r.path }`. The `aleph://session/{id}/raw/{seq}` convention is produced by `SessionCompactor::store_raw_chunk` (see §6.1); this is the only path scheme `recall_context` reads.

### 7.3 session_summary_source

`src/agent_loop/compaction/session_summary_source.rs` implements zero-cost context compaction. When the agent loop needs to shrink its window, `SessionSummarySource::try_reuse` scans `aleph://session/{session_id}/` (any depth), sorts highest-depth-first (`d2` > `d1` > `d0`, then newest within a depth), and assembles summaries into a single synthetic `[Context Summary (from session memory)]` user message within a token budget of `tokens_before / 2`. No LLM call is required — the d-summaries produced by `SessionCompactor` are reused in place.

## 8. Lifecycle

Rows move through four phases:

```
+--------+     +-----------+     +----------+     +----------------------+
| insert | --> | queryable | --> | consumed | --> | retained-but-inert   |
+--------+     +-----------+     +----------+     +----------------------+
                is_processed=0                      is_processed=1
                (visible to                         (invisible to
                 CompressionService                  CompressionService;
                 and path readers)                   still visible to
                                                     path-prefix readers)
```

1. **Insert** — writers in §6 call `insert_raw_memory`.
2. **Queryable** — `CompressionService` batches the row via `get_unprocessed_raw_memories`; path-prefix readers (`recall_context`, `SessionSummarySource`, `SessionCompactor::count_valid_facts_at_depth`) can see it regardless of `is_processed`.
3. **Consumed** — on successful Note write, `mark_raw_as_processed(ids)` flips `is_processed = 1`. The row drops out of the `idx_raw_unprocessed` partial index but stays readable by path.
4. **Retained-but-inert** — the row persists. There is **no time-based eviction of `raw_memories`** in the current codebase: no `DELETE FROM raw_memories`, no TTL constant, no scheduled cleanup job. `SessionCompactorConfig::session_fact_retention_hours = 24` governs session-summary retention semantics higher up the stack, not row deletion here. Housekeeping is currently manual/external; a dedicated GC is future work.

The partial index (`WHERE is_processed = 0`) ensures that even with unbounded historical rows, pending-compression queries stay cheap. Path-prefix queries used by §7.2 and §7.3 rely on the prefix match over `path`; they are bounded by the caller's `limit` (typically 50–500) rather than a dedicated index, which is acceptable because the session-scoped prefix is highly selective.

Operational implication: if rows must be truly deleted (privacy, disk pressure), the correct escape hatch today is an explicit `DELETE FROM raw_memories WHERE is_processed = 1 AND created_at < ?` run out-of-band. The memory subsystem does not issue such a DELETE on its own.

## See Also

- [Notes (L1)](NOTES.md) — where processed raw rows land after distillation.
- [Retrieval](RETRIEVAL.md) §11.4 — `recall_context` tool wiring and session-scoped lookup semantics.
