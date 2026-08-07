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
    // Legacy — keep for backward compatibility.
    SessionCompressed,
    Transcript,
    ToolOutput,
    Attachment,

    // Spec 1 — Memory Capture Hooks.
    PreCompress,
    Delegation { child_agent_id: String },
    SessionEnd { reason: SessionEndReason },

    // Phase 3 self-evolution — user-correction signal.
    Correction { severity: String, suggested_rule: Option<String> },

    // Spec 3 (Dream signals) — one row per tool invocation. `content` carries
    // a short human-readable summary; structured stats live in `source_detail`.
    ToolInvocation { tool_name: String, success: bool, duration_ms: u64 },

    // Batch 2 (session-end reflection) — a first-person "lessons learned"
    // distillation produced once a substantive session ends. Already condensed;
    // ingestable (compound ingestor turns it into feedback/lessons notes).
    Reflection,
}

/// Sub-reason for `RawMemorySource::SessionEnd`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    /// Gateway close or idle timeout.
    Disconnect,
    /// LLM called the `session_complete` tool.
    TaskDone,
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
    pub attachment_text: Option<String>,
    pub is_processed: bool,
    pub created_at: i64,
}
```

`RawMemory::new(content, source)` defaults `agent_id = "default"`, generates a UUID, and stamps `created_at`. Builder methods `with_agent`, `with_session`, `with_path`, and `with_attachment_text` decorate the record before insertion.

> **Removed 2026-08-06:** the `layer` column / field and its `with_layer` builder. It had zero producers and zero consumers — every row ever written carried `NULL` — while nine SELECT/INSERT statements still named it. Summary depth was always encoded in `path` (`d0`/`d1`/`d2`); "reserved for later" outlived the later. The column is nullable with no default, so dropping it from the DDL and the statements leaves existing databases readable (the stale column is simply never touched again) — the migration this was deferred over did not exist.

**Persistence format.** `RawMemorySource` uses `to_persisted()` → `(&'static str, Option<String>)` for SQLite storage, where the optional JSON detail carries variant-specific data:

| Variant | Token | Detail JSON |
|---|---|---|
| `SessionCompressed` | `"session_compressed"` | `None` |
| `Transcript` | `"transcript"` | `None` |
| `ToolOutput` | `"tool_output"` | `None` |
| `Attachment` | `"attachment"` | `None` |
| `PreCompress` | `"pre_compress"` | `None` |
| `Delegation { child_agent_id }` | `"delegation"` | `{ "child_agent_id": "..." }` |
| `SessionEnd { reason }` | `"session_end"` | `{ "reason": "disconnect" \| "task_done" }` |
| `Correction { severity, suggested_rule }` | `"correction"` | `{ "severity": "...", "suggested_rule": "..." }` |
| `ToolInvocation { tool_name, success, duration_ms }` | `"tool_invocation"` | `{ "tool_name": "...", "success": true, "duration_ms": 42 }` |
| `Reflection` | `"reflection"` | `None` |

`from_persisted(token, detail)` reconstructs the enum. Unknown tokens fall through to `ToolOutput` for backward compatibility.

## 5. `RawMemoryStore` Trait

Trait defined in `src/memory/store/raw_memory.rs`; implemented by the SQLite backend in `src/memory/store/sqlite/raw_memories.rs`.

| Method | Purpose |
|---|---|
| `async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError>` | Persist a single record. Used by every writer in §6. |
| `async fn insert_raw_memory_or_ignore(&self, raw: &RawMemory) -> Result<(), AlephError>` | Like `insert_raw_memory`, but silently discards unique-constraint violations (first writer wins). SQLite backend uses `INSERT OR IGNORE`. |
| `async fn get_unprocessed_raw_memories(&self, agent_id: &str, limit: usize) -> Result<Vec<RawMemory>, AlephError>` | Batch-fetch pending rows, ordered by `created_at ASC`. Backed by the `idx_raw_unprocessed` partial index. |
| `async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError>` | Flip rows to `is_processed = 1` after `CompressionService` consumes them. Returns affected row count. |
| `async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError>` | Backpressure / scheduling signal for compression triggers. |
| `async fn unprocessed_agent_ids(&self) -> Result<Vec<String>, AlephError>` | Distinct `agent_id` values with pending work. Used by `CompressionService` to fan out across agents. |
| `async fn get_raw_by_path_prefix(&self, path_prefix: &str, agent_id: &str, limit: usize) -> Result<Vec<RawMemory>, AlephError>` | Path-scoped lookup used by session-context tooling (e.g. `aleph://session/{id}/`). |
| `async fn get_raw_by_path_prefix_since(&self, path_prefix: &str, agent_id: &str, since_created_at: i64, limit: usize) -> Result<Vec<RawMemory>, AlephError>` | Like `get_raw_by_path_prefix`, but only returns rows with `created_at > since`. Used by watermark-based consumers. |
| `async fn find_by_path(&self, path: &str, agent_id: &str) -> Result<Option<RawMemory>, AlephError>` | Exact-match lookup by `(agent_id, path)`. SQLite backend uses indexed exact-match SELECT. |
| `async fn get_raw_by_source(&self, source: RawMemorySource, agent_id: &str, limit: usize) -> Result<Vec<RawMemory>, AlephError>` | Filter by source type. Used for cross-session retrieval of specific source kinds. |

## 6. Writers

### 6.1 SessionCompactor

`src/memory/session_compactor/mod.rs` owns the hierarchical session summary pipeline. On every post-turn tick it semantic-chunks compressible messages, generates a d0 (leaf) summary per chunk, then condenses d0→d1 when count ≥ `d1_min_fanout` (default 4) and d1→d2 when count ≥ `d2_min_fanout` (default 3). Each summary is written with `RawMemorySource::SessionCompressed` at path `aleph://session/{session_id}/d{depth}/{seq}`. In addition, `store_raw_chunk` writes the verbatim pre-compression conversation chunk at `aleph://session/{session_id}/raw/{seq}` (also `SessionCompressed`) so `recall_context` can recover the originals after the context window has been replaced.

Relevant config fields on `SessionCompactorConfig`: `d1_min_fanout = 4`, `d2_min_fanout = 3`, plus `max_summary_depth` (gates the d1→d2 step).

### 6.2 TranscriptIndexer

`src/memory/transcript_indexer/indexer.rs` handles near-realtime per-turn indexing. `index_turn_text(session_key, seq, user_input, ai_output, namespace, agent)` combines the `[user]` / `[assistant]` pair, chunks it through `chunk_text` when estimated tokens exceed 800, and inserts one `RawMemory` per chunk at path `aleph://transcript/{session_key}/{seq}` (or `…_chunk{i}` for multi-chunk turns). `source = "transcript"`.

Config keys on `TranscriptIndexerConfig` (`src/memory/transcript_indexer/config.rs`): `max_tokens_per_chunk = 400`, `overlap_tokens = 80`, `enable_chunking = true`. A separate `SemanticChunkConfig` (`similarity_threshold = 0.85`, `min_chunk_size = 50`, `max_chunk_size = 400`) governs the semantic-boundary chunker when embeddings are available.

### 6.3 Gateway Media Pipeline (attachment_text)

The `attachment_text` column and the `RawMemory::with_attachment_text` builder exist so that extracted text from PDF / Word / image OCR travels alongside its originating turn rather than becoming a standalone row. The consumer is `CompressionService`, which in `src/memory/compression/service.rs` reads `raw.attachment_text` and injects up to 2000 characters of it into the per-memory prompt as `"{user_input}\n[Attachment]: {att_preview}"`. `RawMemorySource::Attachment` is reserved for the case of attachment-only rows (e.g. an uploaded document with no accompanying turn). As of this writing the production gateway write sites that populate `attachment_text` during normal message ingestion are not present in `src/gateway/`; the field is wired end-to-end through schema, store, and compression, and is exercised in `src/memory/compression/service.rs` tests (`with_attachment_text(...)`).

### 6.4 Capture Hooks (Spec 1)

Three additional producers feed `raw_memories` via the Spec 1 memory capture hooks. Each writes rows with a dedicated `RawMemorySource` variant; `CompressionService::compress_to_notes` groups the drained batch per source and hands each group to `CompoundIngestor::ingest_batch`, which derives the group's specialised system prompt via `memory::compression::source_prompts::prompt_for`.

- **`PreCompress`** — emitted by `SessionCompactor` before a session chunk is dropped to summary. The pre-drop raw text lands in `raw_memories` so the RESCUE prompt can extract durable knowledge before the chunk is gone. Producer: `src/memory/session_compactor/mod.rs` (production path) and `src/components/session_compactor/compactor.rs::replace_with_summary` (event-driven variant). Writer injected via `SessionCompactor::with_raw_memory_writer(..)`.
- **`Delegation { child_agent_id }`** — emitted by `A2ASubAgent::execute` (`src/a2a/sub_agent.rs`) on the success branch just before `Ok(SubAgentResult)`. Content carries delegation prompt + sub-agent summary; the LESSON prompt distills durable parent-agent lessons (tool patterns, gotchas, failure modes). Parent `agent_id` lifted from `request.execution_context.metadata["parent_agent_id"]` (falls back to `"default"`).
- **`SessionEnd { reason }`** — two flavours:
  - `reason = Disconnect`: emitted by `SessionManager::close_session` (`src/gateway/session_manager/ops/emit.rs`) with the conversation tail (up to 64 most-recent messages). The row passes the memory-extension `on_capture` filters (registry resolved from the startup-registered `SESSION_END_MCP` cell; no registry → direct insert). DIGEST prompt distills user preferences, project progress, unfinished items.
  - `reason = TaskDone`: emitted by the new `session_complete` builtin tool (`src/builtin_tools/session_complete.rs`) when the LLM marks a self-contained task complete. RETRO prompt captures transferable lessons — the R8 LLM-sovereignty path for task-boundary detection.

All producers are synchronous-write / async-extract: each writes exactly one `raw_memories` row and returns; extraction runs later in `CompressionService` per its normal schedule. Emission is fire-and-forget (`tokio::spawn`) so hook overhead is kept out of the hot path.

See: [docs/superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md](../../../superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md)

## 7. Readers

### 7.1 CompressionService

`src/memory/compression/service.rs` drives the L0→L1 distillation. Per workspace tick it calls `get_unprocessed_raw_memories(workspace_id, batch_size)`, then partitions the batch: `RawMemorySource::ToolInvocation` rows are per-call **telemetry** (consumed by the insights aggregator and dream signal metrics by source, independent of `is_processed`), not knowledge — they never enter the note-extraction LLM batch and are marked processed immediately so the unprocessed queue stays bounded. `Transcript` rows **are** ingested alongside SessionEnd / PreCompress / Delegation / Reflection (the historical filter that excluded them assumed a separate per-turn pipeline that never landed; excluding them starved L1 — Spec 1 G3-B). `Correction` rows are primarily consumed by the `FeedbackDistill` dream stage via the `aleph://correction/` path prefix, isolated from the `is_processed` flag this pipeline owns.

The remaining ingestable rows are grouped by prompt-affecting source identity (a mixed drain MUST be split — the ingestor derives its per-source prompt from `raws[0].source`, so ungrouped Reflection/SessionEnd/Delegation rows would silently degrade to whichever source was fetched first). Each group goes through `CompoundIngestor::ingest_batch`, which writes/updates markdown notes under `<memory_dir>/<workspace>/<category>/<file>.md`; rows are marked processed per group right after that group's ingest settles (a failed group stays unprocessed and retries; an empty plan defers rows still within a 6-hour grace window). The latest `created_at` of the drained batch becomes the new `last_compression_timestamp`.

#### Partitions a flush is responsible for

`raw_memories` is keyed by `agent_id`, and **one session writes into more than one partition**: turn-level rows go through `project_scope::session_write_id` (composed with the project namespace and/or the session's personal scope when either axis is active) while the SessionEnd digest is filed under the bare agent id. The hourly background tick iterates `unprocessed_agent_ids()` and so covers all of them; the real-time flush (`memory::flush::flush_agent_memory`, Pillar 2) drained only the id it was handed.

`flush_partitions` now resolves the base id **plus every composed sibling** (`{base}__…`) that currently holds unprocessed rows, and drains each. Without it, with project or personal scoping enabled, "real-time flush" was real time for the digest and up to an hour for everything the session actually said — while `FlushRegistry::await_ready`, keyed on the base id, told the next session the consolidation it was waiting for had finished.

Matched on the `{base}__` prefix, never a bare `starts_with(base)`: `main` and `mainframe` are unrelated corpora, not parent and child. A failure to enumerate degrades to the base partition alone rather than skipping the flush.

### 7.2 recall_context

`src/builtin_tools/recall_context.rs` is the LLM-facing tool that lets the model recover pre-compression conversation details. `RecallContextTool::call_impl` builds the path prefix `aleph://session/{session_id}/raw/` and calls `get_raw_by_path_prefix(prefix, "default", args.max_results)`. Each returned `RawMemory` becomes a `RecalledFragment { content, relevance_score: 1.0, source_path: r.path }`. The `aleph://session/{id}/raw/{seq}` convention is produced by `SessionCompactor::store_raw_chunk` (see §6.1); this is the only path scheme `recall_context` reads.

### 7.3 session_summary_source

`src/memory/session_compactor/session_summary_source.rs` implements zero-cost context compaction. When the agent harness needs to shrink its window, `SessionSummarySource::try_reuse` scans `aleph://session/{session_id}/` (any depth), sorts highest-depth-first (`d2` > `d1` > `d0`, then newest within a depth), and assembles summaries into a single synthetic `[Context Summary (from session memory)]` user message within a token budget of `tokens_before / 2`. No LLM call is required — the d-summaries produced by `SessionCompactor` are reused in place.

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
4. **Retained-but-inert** — the row persists. There is **no time-based eviction of `raw_memories`** in the current codebase: no `DELETE FROM raw_memories`, no TTL constant, no scheduled cleanup job. Nor does one exist further up the stack: this paragraph used to cite `SessionCompactorConfig::session_fact_retention_hours = 24` as governing session-summary retention "higher up", but that field was read by nothing anywhere in the workspace and was removed 2026-08-04 — the retention it named never existed at any level. Housekeeping is currently manual/external; a dedicated GC is future work, and would introduce its own knob.

The partial index (`WHERE is_processed = 0`) ensures that even with unbounded historical rows, pending-compression queries stay cheap. Path-prefix queries used by §7.2 and §7.3 rely on the prefix match over `path`; they are bounded by the caller's `limit` (typically 50–500) rather than a dedicated index, which is acceptable because the session-scoped prefix is highly selective.

Operational implication: if rows must be truly deleted (privacy, disk pressure), the correct escape hatch today is an explicit `DELETE FROM raw_memories WHERE is_processed = 1 AND created_at < ?` run out-of-band. The memory subsystem does not issue such a DELETE on its own.

## 9. Retention Invariant (Provenance Chain Protection)

**Any future time-based or size-based garbage-collection sweep of `raw_memories` must exclude rows referenced by the provenance chain.** Concretely: before deleting a row, verify that `notes_citing(raw_id)` is empty — i.e. the row is not cited as a `source_notes` entry in any note, and is not referenced in any `notes_provenance` or `notes_sources` table. If a row is still referenced, it must be retained even if it falls outside the retention window.

Today (as of 2026-06-27) no time-based sweep exists; this invariant documents the pin to honor when one is added. The provenance chain — linking notes back to their raw-memory sources across three levels (L3 user-profile sessions → L2 synthesized notes → L1 distilled-note facts → L0 raw rows) — creates a bidirectional dependency: just as CompressionService produces notes from raws, the evidence chain allows drill-down queries to recover the source raw memories that fed a given fact or profile section. Deleting a raw while notes still point to it breaks the chain and orphans evidence.

See [MEMORY_SYSTEM.md §12](../MEMORY_SYSTEM.md) for the full provenance chain documentation.

## See Also

- [Notes (L1)](NOTES.md) — where processed raw rows land after distillation.
- [Retrieval](RETRIEVAL.md) §12.4 — `recall_context` tool wiring and session-scoped lookup semantics.
