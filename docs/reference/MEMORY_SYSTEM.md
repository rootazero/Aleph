# Memory System

> Persistent knowledge for LLM conversations via markdown-first notes, ephemeral raw memories, and offline consolidation.

## 1. Purpose

Aleph's memory system gives the LLM durable knowledge across sessions. Conversations and attachments land in an ephemeral raw-memory buffer (L0); a realtime compressor distills them into human-readable markdown notes (L1). An offline daemon periodically consolidates, synthesizes, and prunes those notes so retrieval stays sharp as the corpus grows.

## 2. Design Principles

- **L0 (raw, ephemeral) → L1 (notes, persistent) separation.** Transcripts are transient; knowledge lives as markdown.
- **Markdown is the source of truth; SQLite is a rebuildable index.** Every table can be reconstructed from the `.md` files on disk.
- **One trait per storage concern.** No monolithic `MemoryStore` — each capability is its own trait so callers depend only on what they use.
- **LLM sovereignty.** Classification, merge, and synthesis decisions go to the model, not to regex or keyword heuristics.
- **Real filesystem over VFS abstractions.** Notes are ordinary files a human can `cat`, `grep`, back up, and version-control.

## 3. Two-Layer Data Model

```
Gateway / Agent Loop
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│ raw_memories (SQLite)                                        │
│   sessions · transcripts · attachment_text                   │
│   consumed by CompressionService (is_processed flag)         │
└─────────────────────────────────────────────────────────────┘
    │  CompressionService (realtime)
    ▼
┌─────────────────────────────────────────────────────────────┐
│ notes (Markdown files + SQLite index)                        │
│   ~/.aleph/memory/note/{agent}/{category}/*.md               │
│   notes_index · notes_links · notes_fts · notes_vec_{dim}    │
└─────────────────────────────────────────────────────────────┘
    │  Dream Daemon (offline, idle-only)
    ▼
   consolidate / drift / synthesis / lint / decay / digest
    │
    ▼
 queries: NoteFactRetrieval.retrieve() → ScoredFact<MemoryFact>
```

Gateway writes conversation turns to `raw_memories` through `RawMemoryStore`. `CompressionService` periodically drains unprocessed rows, asks an LLM to extract durable facts, and writes them back as markdown notes under `~/.aleph/memory/note/{agent}/{category}/`. `NoteIndexer` keeps the SQLite side (index, wikilinks, FTS5, per-dimension vec0 tables) in sync on every write. Offline, the Dream Daemon consolidates clusters, resolves drift, synthesizes insights, lints schemas, decays weak notes, and emits digests. Queries flow through `NoteFactRetrieval` and return `ScoredFact<MemoryFact>` — see [RETRIEVAL.md](memory/RETRIEVAL.md).

## 4. Storage Traits

| Trait | File | Purpose | Primary caller |
|---|---|---|---|
| `NoteStore` | `src/memory/notes/store.rs` | Notes index, wikilinks, FTS, vector search | `NoteFactRetrieval`, `NoteIndexer` |
| `RawMemoryStore` | `src/memory/store/raw_memory.rs` | Raw memory CRUD + is_processed flag | `CompressionService`, `SessionCompactor` |
| `DreamStore` | `src/memory/store/mod.rs` | Dream status + daily insights | `DreamDaemon` |
| `CompressionStore` | `src/memory/store/mod.rs` | Compression-run audit metadata | `CompressionService` |

All four are implemented by `SqliteMemoryBackend`, wrapped in `MemoryBackend = Arc<SqliteMemoryBackend>`.

## 5. Scratchpad

The scratchpad (`src/memory/scratchpad/`) is an in-session working-memory buffer — orthogonal to L0/L1. It is per-session and non-archival: when a session ends, the scratchpad is discarded (not compressed into notes).

Key types:

- `ScratchpadConfig` (`manager.rs`) — filename, history filename, backup-on-write flag.
- `SessionHistory` (`history.rs`) — append-only log of completed plan items.
- `ScratchpadManager` (`manager.rs`) — writes `scratchpad.md` + `session_history.log` under `~/.aleph/projects/<project_id>/`.

Scratchpad lives at the session level (active plan, current step), whereas L0 raw memory is the session **archive** and L1 notes are cross-session knowledge. The three layers do not overlap: a scratchpad entry never becomes a note directly, and notes never flow back into the scratchpad.

## 6. Interfaces

The memory system is exposed to the LLM through five built-in tools. Each links to the relevant subdocument.

| Tool | Purpose | Doc |
|---|---|---|
| `note_manage` | CRUD on notes (unified skill/wiki/other) | [NOTES.md §11](memory/NOTES.md) |
| `memory_search` | Hybrid retrieval | [RETRIEVAL.md §11.1](memory/RETRIEVAL.md) |
| `memory_browse` | Filesystem browser over notes | [RETRIEVAL.md §11.2](memory/RETRIEVAL.md) |
| `memory_explore` | Multi-hop (Ripple) exploration | [RETRIEVAL.md §11.3](memory/RETRIEVAL.md) |
| `recall_context` | Session raw-data restore | [RETRIEVAL.md §11.4](memory/RETRIEVAL.md) |

## 7. TOML Configuration

Keys below are the subset of `MemoryConfig` (`src/config/types/memory.rs`) that operators actually tune. Defaults are shown inline.

```toml
[memory]
enabled = true                          # master switch
max_context_items = 5                   # max memories injected per turn
retention_days = 90                     # 0 = never delete
vector_db = "sqlite-vec"                # only backend today
similarity_threshold = 0.3              # min score (1/(1+L2)) to include

ai_retrieval_enabled = true             # LLM-picked vs pure-vector selection
ai_retrieval_timeout_ms = 3000          # cap on LLM selection call
ai_retrieval_max_candidates = 20        # pre-LLM candidate pool size
ai_retrieval_fallback_count = 3         # fallback when LLM selection fails

compression_enabled = true              # raw-memory → note pipeline
compression_idle_timeout_seconds = 300  # idle seconds before a run
compression_turn_threshold = 20         # turn count that also triggers a run
compression_interval_seconds = 3600     # background poll cadence
compression_batch_size = 50             # max raws processed per run
conflict_similarity_threshold = 0.85    # dedupe/conflict cutoff
max_facts_in_context = 5                # notes injected per turn
raw_memory_fallback_count = 3           # raws used if notes are insufficient

rrf_k = 60                              # Reciprocal Rank Fusion constant
bm25_bonus_weight = 0.15                # extra BM25 lift in fusion
query_expansion_enabled = false         # synonym expansion (off by default)
dedup_similarity_threshold = 0.95       # storage-time dedupe
backup_enabled = true                   # JSONL backup of notes
backup_max_files = 7                    # backup retention

[memory.dreaming]
enabled = true                          # offline consolidation daemon
idle_threshold_seconds = 900            # system idle before a run (15 min)
window_start_local = "02:00"            # allowed run window start
window_end_local = "05:00"              # allowed run window end
max_duration_seconds = 600              # hard cap per run (10 min)
weekly_enabled = true                   # weekly deep synthesis
weekly_interval_days = 7
cluster_dbscan_eps = 0.3                # DBSCAN epsilon (cosine distance)
cluster_dbscan_min_samples = 2
drift_similarity_threshold = 0.85
drift_max_pairs_per_run = 20
synthesis_min_cluster_size = 3
synthesis_max_insights = 10

[memory.memory_decay]
half_life_days = 30.0                   # note-strength half-life
access_boost = 0.2                      # bump on successful recall
min_strength = 0.1                      # prune threshold
protected_types = ["personal"]          # never decayed

[memory.reflection]
enabled = false                         # session-end reflection pass
min_turns = 5
min_user_chars = 200
cooldown_minutes = 30
```

Embedding provider, rerank, scoring pipeline, and noise filter live in dedicated subtables — see [RETRIEVAL.md](memory/RETRIEVAL.md).

## 8. Subdocument Navigation

- [Notes (L1)](memory/NOTES.md) — markdown-first persistent knowledge, indexing, `note_manage` tool.
- [Raw Memory (L0)](memory/RAW_MEMORY.md) — ephemeral session data, compression input.
- [Dream Daemon](memory/DREAM_DAEMON.md) — 6-stage offline notes consolidation.
- [Retrieval](memory/RETRIEVAL.md) — hybrid search, scoring, tools, audit.

## 9. Troubleshooting

### High memory / disk usage

Symptom: `~/.aleph/memory/note/` or the SQLite database grow unbounded.

1. Shorten retention: `memory.retention_days = 30`.
2. Raise the decay prune floor: `memory.memory_decay.min_strength = 0.2`.
3. Shrink the decay half-life: `memory.memory_decay.half_life_days = 14.0` so stale notes fade faster.
4. Cap backups: `memory.backup_max_files = 3` (or `memory.backup_enabled = false`).
5. Tighten storage dedupe: `memory.dedup_similarity_threshold = 0.9`.

### Slow memory search

Symptom: `memory_search` latency exceeds ~1s.

1. Reduce fan-out: `memory.max_context_items = 3` and `memory.max_facts_in_context = 3`.
2. Raise the cutoff: `memory.similarity_threshold = 0.6` to drop weak candidates earlier.
3. Shrink the LLM selection pool: `memory.ai_retrieval_max_candidates = 10`.
4. Tighten the LLM timeout: `memory.ai_retrieval_timeout_ms = 1500` (fallback to pure vector faster).
5. Turn off LLM selection entirely: `memory.ai_retrieval_enabled = false`.

### Missing relevant notes

Symptom: a note you know exists does not surface in search results.

1. Lower `memory.similarity_threshold = 0.2` to admit more candidates.
2. Raise `memory.max_context_items` and `memory.max_facts_in_context` together.
3. Enable query expansion: `memory.query_expansion_enabled = true`.
4. Increase BM25 weight when the target note is a good lexical match: `memory.bm25_bonus_weight = 0.3`.
5. Use `memory_explore` for multi-hop traversal when single-shot retrieval keeps missing the wikilink neighborhood — see [RETRIEVAL.md](memory/RETRIEVAL.md).

## Orientation layer (Spec 5, shipped 2026-04-14)

Aleph maintains three LLM-readable markdown files per agent —
`SCHEMA.md`, `index.md`, `log.md` — and a `NoteOrientation` trait that
projects the live `notes_index` into them. The note orientation layer is
injected into the prompt in Context/Hybrid modes and available as the
`note_orient` tool in Tools/Hybrid modes. Schema mutation goes through
the always-registered `note_schema` tool with optimistic concurrency via
content hashes. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §2](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md)
for the design; the four new markdown files now live alongside the
existing per-category note directories.

## Query filed-back (Spec 8, shipped 2026-04-17)

High-value `memory_reflect` answers are automatically archived as
`query/` category notes. A two-tier gate (cheap: ≥3 sources + ≥200 chars;
LLM: novel synthesis check) decides filing. The `query_filed` SQLite
table deduplicates by `sha256(query)`. `NoteSynthesis` weekly stage
excludes `query/` to prevent recursion. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §5](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md).
