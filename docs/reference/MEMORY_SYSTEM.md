# Memory System

> Persistent knowledge for LLM conversations via markdown-first notes, ephemeral raw memories, and offline consolidation.

## 1. Purpose

Aleph's memory system gives the LLM durable knowledge across sessions. Conversations and attachments land in an ephemeral raw-memory buffer (L0); a realtime compressor distills them into human-readable markdown notes (L1). Notes are linked via Obsidian-compatible `[[wikilinks]]` forming a traversable knowledge graph. An offline daemon periodically consolidates, synthesizes, and prunes those notes so retrieval stays sharp as the corpus grows.

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

### 3.1 Cognitive-layer view (working → episodic → semantic → raw)

The two physical tiers above *realise* a human-memory-style four-layer model; [`CognitiveLayer`](../../src/memory/context/enums.rs) is the canonical labelling, derived (not stored) at render time by `assembler::render::cognitive_layer`:

| Cognitive layer | Realised by | Decay |
|---|---|---|
| **Working** (current task) | live session / scratchpad + raw items in the `session_recent` slot | n/a (ephemeral) |
| **Episodic** (experiences, causal chains) | session summaries, `transcript` / `subagent-*` notes | retrieval recency + dream decay |
| **Semantic** (facts & rules) | distilled `preference` / `learning` / `skill` / `reference` notes | severity-floored confidence decay |
| **Raw** (audit / 回溯 base) | `raw_memories` rows behind every distillation | retention sweep only |

Retrieved items are stamped with their layer in the assembled memory context (markdown header + XML `layer="…"` attribute) so the model perceives the structure. Notes flagged `permanent: true` (or tagged `permanent` / `pinned`), and categories in `memory.memory_decay.protected_types`, are exempt from decay/archival — the "永久核心知识不受影响" guarantee (see [DREAM_DAEMON.md](memory/DREAM_DAEMON.md) §5.5).

**Hot-surfacing & time-decay at recall** default **on** (`memory.retrieval_scoring`): frequently-recalled notes bubble up (`reinforcement`, from `recall_signals` counts), stale ones fade (`recency`); both read existing data with no extra model calls. See [RETRIEVAL.md](memory/RETRIEVAL.md).

**Embeddings** are pluggable via the `EmbeddingProvider` trait (OpenAI / SiliconFlow / Ollama through the OpenAI-compatible `RemoteEmbeddingProvider`); the **data-stays-local** requirement is met today by pointing it at a local **Ollama**. A *bundled* in-binary ONNX backend is intentionally **not** added — it would pull a heavy single-purpose native dependency into core (violates redline R3 核心轻量化) for a privacy goal the local-Ollama path already satisfies. It remains a clean future addition behind the same trait if the trade-off changes.

## 4. Storage Traits

| Trait | File | Purpose | Primary caller |
|---|---|---|---|
| `NoteStore` | `src/memory/notes/store.rs` | Notes index, wikilinks, FTS, vector search | `NoteFactRetrieval`, `NoteIndexer` |
| `RawMemoryStore` | `src/memory/store/raw_memory.rs` | Raw memory CRUD + is_processed flag | `CompressionService`, `SessionCompactor` |
| `DreamStore` | `src/memory/store/mod.rs` | Dream status + daily insights | `DreamDaemon` |
| `CompressionStore` | `src/memory/store/mod.rs` | Compression-run audit metadata | `CompressionService` |

All four are implemented by `SqliteMemoryBackend`, wrapped in `MemoryBackend = Arc<SqliteMemoryBackend>`.

## 5. Working Memory Assembler

The `WorkingMemoryAssembler` trait (`src/memory/assembler/mod.rs`) produces a `MemoryEnvelope` before every LLM call. This is the bridge between retrieval and prompt injection:

```rust
#[async_trait]
pub trait WorkingMemoryAssembler: Send + Sync {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
        filter: FactSourceFilter,
    ) -> Result<MemoryEnvelope, AlephError>;
}
```

`HybridAssembler` is the production implementation. It:
1. Calls `NoteFactRetrieval::retrieve` for hybrid search
2. Optionally runs LLM re-ranking (`AiProviderReranker`)
3. Hydrates results into `EnvelopeItem`s
4. Applies registered `MemoryExtension::on_retrieve` hooks
5. Renders the envelope to XML via `render_with(&env, RenderStyle::Xml)`

The `MemoryEnvelope` structure (`src/memory/assembler/envelope.rs`) carries schema version, query, slots (each with a `SlotKind`: `UserProfile` / `SessionRecent` / `RelevantNotes` / `Feedback` / `RawFragments` / `Nudges`), and metadata:

```rust
pub struct MemoryEnvelope {
    pub schema_version: String,
    pub generated_at: i64,
    pub query: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub slots: Vec<EnvelopeSlot>,
    pub meta: EnvelopeMeta,
}
```

## 6. Scratchpad

The scratchpad (`src/memory/scratchpad/`) is an in-session working-memory buffer — orthogonal to L0/L1. It is per-session and non-archival: when a session ends, the scratchpad is discarded (not compressed into notes).

Key types:

- `ScratchpadConfig` (`manager.rs`) — filename, backup-on-write flag.
- `ScratchpadManager` (`manager.rs`) — writes `scratchpad.md` under `~/.aleph/workspaces/<agent_id>/` (see `default_workspace_root`; per-run project overrides do NOT relocate the scratchpad — runtime working memory stays bound to the agent).

Scratchpad lives at the session level (active plan, current step), whereas L0 raw memory is the session **archive** and L1 notes are cross-session knowledge. The three layers do not overlap: a scratchpad entry never becomes a note directly, and notes never flow back into the scratchpad.

The scratchpad's `## Plan` section is also the agent's **execution list**
(todo/plan): `- [ ]` / `- [~]` / `- [x]` three-state checkboxes written by the
model through the `scratchpad` tool, projected onto the wire as
`aleph_protocol::plan::PlanSnapshot`, and consumed by the run-start
`<execution_plan>` prompt block, the compaction carry
(`context::compact::plan_carry`), the `ScratchpadGoalVerifier` stop guard, the
channel progress push, and the Panel Todo strip. That whole surface — including
the write semantics (whole-list replace with status inheritance, single
in-progress enforced in code, self-healing section upsert) — is documented in
FEATURE_LOCATOR §3.13; this section covers only where the bytes live.

## 7. Memory Event Sourcing

Every mutation to a note is captured as an immutable `MemoryEvent` wrapped in a `MemoryEventEnvelope`. This provides an audit trail and enables time-travel queries.

Events are classified as:
- **Skeleton** — structural mutations persisted immediately (`NoteCreated`, `NoteContentUpdated`, `NoteInvalidated`, `NoteRestored`, `NoteDeleted`, `NoteConsolidated`, `NoteMigrated`)
- **Pulse** — high-frequency observations buffered before persist (`NoteAccessed`)

Key types (`src/memory/events/mod.rs`):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MemoryEvent {
    NoteCreated { note_path: String, content: String, note_type: NoteType, ... },
    NoteContentUpdated { note_path: String, old_content: String, new_content: String, reason: String },
    NoteAccessed { note_path: String, query: Option<String>, relevance_score: Option<f32>, ... },
    // ... other variants
}
```

The `MemoryCommandHandler` (`src/memory/events/handler.rs`) projects events into the notes layer: append to event log → fold events via `EventProjector` → write markdown → re-index. This is the "notes dual-write" pattern: event log is the audit source of truth; markdown files are the primary read surface.

## 8. Memory Extensions

The memory pipeline exposes three hook points through the `MemoryExtension` trait (`src/memory/extensions/traits.rs`):

| Hook | When | Purpose |
|------|------|---------|
| `on_retrieve` | After assembly, before XML rendering | Augment/filter/reorder the envelope |
| `on_capture` | Before `insert_raw_memory` | Inspect/redact/block raw memories |
| `produce` | Dedicated scheduler tick | Produce raw memories from external sources |

Dispatch semantics (`src/memory/extensions/registry.rs`):
- `on_retrieve` — sequential broadcast, 2s timeout per plugin
- `on_capture` — chained pipeline, 3s timeout, `Block` short-circuits
- `produce` — parallel per-plugin, 30s timeout

First-party extensions implement `MemoryExtension` directly in Rust; third-party plugins implement the same hooks over MCP via `McpMemoryExtension` (`src/memory/extensions/mcp_adapter.rs`). Both register to the same `MemoryExtensionRegistry`.

The `MemoryProducerScheduler` (`src/memory/extensions/scheduler.rs`) ticks every 10s, calling `produce` on registered extensions and routing results through `insert_with_capture_filter` so producer-generated memories still pass `on_capture`.

## 9. Interfaces

The memory system is exposed to the LLM through built-in tools. Each links to the relevant subdocument.

| Tool | Purpose | Doc |
|---|---|---|
| `note_manage` | CRUD on notes (unified skill/reference/other) | [NOTES.md §11](memory/NOTES.md) |
| `memory_search` | Hybrid retrieval | [RETRIEVAL.md §12.1](memory/RETRIEVAL.md) |
| `memory_browse` | Filesystem browser over notes | [RETRIEVAL.md §12.2](memory/RETRIEVAL.md) |
| `memory_explore` | Multi-hop (Ripple) exploration | [RETRIEVAL.md §12.3](memory/RETRIEVAL.md) |
| `recall_context` | Session raw-data restore | [RETRIEVAL.md §12.4](memory/RETRIEVAL.md) |
| `memory_reflect` | LLM synthesis over retrieved memories | [RETRIEVAL.md §14](memory/RETRIEVAL.md) |
| `session_complete` | Mark task complete, trigger session-end capture | [RAW_MEMORY.md §6.4](memory/RAW_MEMORY.md) |
| `remember` | Curated `MEMORY.md` hot zone (add / replace / remove / atomic batch) | §17 below |
| `flag_user_correction` | Record a user correction — entry point of the correction rail | §17 below |

## 10. TOML Configuration

Keys below are the subset of `MemoryConfig` (`src/config/types/memory/`) that operators actually tune. Defaults are shown inline.

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
enabled = false                # master switch — flipping this ONE flag lights the
                               # whole lessons + open-loops pipeline (LLM call per
                               # substantive session end, so it stays opt-in)
min_turns = 5                  # substance gate: skip trivial sessions
min_user_chars = 200
cooldown_minutes = 30          # per-agent throttle; the watermark persists to the
                               # compression_metadata table (consumer key
                               # "session_reflection") so restarts don't reset it
open_loop_tracking = true      # extract unresolved questions / promised follow-ups
                               # in the SAME reflection call → OPEN_LOOPS.md
open_loop_inject_prompt = true # inject persisted open loops into the next
                               # session's curated context (R5 — AI 主动到达)
```

Compression **scheduling** lives under `[policies]`, not `[memory]` (`src/config/types/policies/memory.rs`):

```toml
[policies.memory.compression]
turn_threshold = 20                # conversation turns before a compression run
background_interval_seconds = 3600 # hourly background drain cadence
```

The live compression triggers are: turn threshold, hourly background interval, session-end flush, correction flush (`flag_user_correction`), and the `memory.compress` RPC. The former idle-timeout trigger (and its `idle_timeout_seconds` knob) was removed — it never had a reachable production path; old config files carrying the key still parse (unknown keys ignored). Batch size is fixed at 50 raws per run.

Embedding provider, rerank, scoring pipeline, and noise filter live in dedicated subtables — see [RETRIEVAL.md](memory/RETRIEVAL.md).

## 11. Knowledge Graph (Wikilinks)

Notes form an Obsidian-compatible knowledge graph through `[[wikilink]]` syntax:

- **Extraction**: `extract_wikilinks()` parses `[[note-name]]` from markdown bodies
- **Resolution**: `resolve_wikilink()` follows Obsidian rules — exact path match if `/` present, global filename search otherwise
- **Bidirectional linking**: `note_manage` tool supports `link` operations that create reciprocal connections
- **Graph traversal**: `memory_explore` performs multi-hop Ripple exploration across the wikilink graph
- **Maintenance**: Dream Daemon's `note_lint` stage repairs broken wikilinks and rewrites renamed targets

The `notes_links` SQLite table stores outgoing links per note, enabling fast graph traversal without re-parsing markdown.

## 12. Provenance Chain (Evidence Drill-Down)

Evidence traceability from user profile → synthesized notes → distilled notes → raw source data is enforced via four-level provenance metadata:

**L3 (Session-level):** `USER.md` contains a machine-readable `## Sources` block listing, per profile section (Identity, Communication Style, etc.), which session IDs last modified that section. Example: `- Identity: sid_abc, sid_def`.

**L2 (Synthesis-level):** Synthesis notes (output of the Dream Daemon consolidation phase) store `source_notes` — an array of note paths that were clustered/merged to produce the synthesis. The field persists in note frontmatter as `source_notes: [path/to/note1, path/to/note2]`.

**L1 (Distillation-level):** Regular notes produced by `CompressionService` store per-fact provenance via two mechanisms:
- `source_notes` field: list of raw-memory IDs or notes that fed this note's creation.
- `fact_provenance` field: per fact (parsed from inline HTML comments `<!-- src: <id>, origin: raw_source, inferred: false -->`) tracing individual facts back to raw-memory rows.

Both are materialized into SQLite tables `notes_sources` and `notes_provenance` by the `NoteIndexer` via `index_note()`.

**L0 (Raw source):** `raw_memories` rows are the authoritative source. Read APIs:
- `RawMemoryStore::get_raws_by_ids(ids)` — fetch raw rows by ID.
- `RawMemoryStore::get_raws_by_session(session_id)` — fetch all raws in a session.
- `NoteStore::notes_citing(raw_id)` — fetch all notes that reference a raw via `notes_sources` or `notes_provenance`.

**Consumer:** The `memory_trace` builtin tool and the `memory.trace` gateway RPC expose the drill-down chain:
- Kind `profile_section` → session IDs from `USER.md ## Sources`.
- Kind `note` → `source_notes` list (L2→L1) and per-fact `fact_provenance`.
- Kind `raw` → raw-memory content from `raw_memories`.

Missing or pruned raws are gracefully degraded with `pruned: true` marker; the chain never errors.

## 13. Subdocument Navigation

- [Notes (L1)](memory/NOTES.md) — markdown-first persistent knowledge, indexing, `note_manage` tool, wikilink graph, event sourcing.
- [Raw Memory (L0)](memory/RAW_MEMORY.md) — ephemeral session data, compression input, capture hooks.
- [Dream Daemon](memory/DREAM_DAEMON.md) — 6-stage offline notes consolidation.
- [Retrieval](memory/RETRIEVAL.md) — hybrid search, scoring, tools, audit, reflection.
- [Extensions](memory/EXTENSIONS.md) — pluggable memory hooks (retrieve, capture, produce).

## 13. Troubleshooting

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

## 14. Orientation layer (Spec 5, shipped 2026-04-14)

Aleph maintains three LLM-readable markdown files per agent —
`SCHEMA.md`, `index.md`, `log.md` — and a `NoteOrientation` trait that
projects the live `notes_index` into them. The orientation envelope
(schema + note index + recent-log tail) is injected at prompt build:
`src/orchestrator/harness_bridge/prompt_build.rs` calls
`MemoryContextProvider::build_orientation_user_message` (which returns
`None` in Tools mode or when no wiki handle is registered) and merges
the resulting XML with the curated-memory envelope via
`merge_stable_memory_envelopes`; the merged string rides the stable
prompt prefix that `CuratedMemoryLayer` injects verbatim, so
deployments without notes stay byte-identical. The same data is
available as the `note_orient` tool in Tools/Hybrid modes. Schema mutation goes through
the always-registered `note_schema` tool with optimistic concurrency via
content hashes. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §2](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md)
for the design; the four new markdown files now live alongside the
existing per-category note directories.

## 15. User profile (Spec 7, shipped 2026-04-17)

`USER.md` is a dialectic, session-end-synthesised user model with six
fixed sections (Identity, Communication Style, Motivations, Current
Focus, Stance Shifts, Open Questions). `ProfileSynthesizer` fires after
each `SessionEnd` raw is processed — the LLM merges session insights
into the existing profile with hash-guarded atomic writes. The profile
is injected as a `<UserProfile>` XML envelope on first turn and every
N turns thereafter (configurable). The `user_profile` tool exposes
read access in Tools/Hybrid mode. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §4](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md).

## 16. Query filed-back (Spec 8, shipped 2026-04-17)

High-value `memory_reflect` answers are automatically archived as
`query/` category notes. A two-tier gate (cheap: ≥3 sources + ≥200 chars;
LLM: novel synthesis check) decides filing. The `query_filed` SQLite
table deduplicates by `sha256(query)`. `NoteSynthesis` weekly stage
excludes `query/` to prevent recursion. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §5](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md).

## 17. Correction rail, destination ladder & acknowledgment contract

**Correction rail (显式纠错链).** When the user corrects the model, the
`flag_user_correction` tool writes a `RawMemorySource::Correction` row at
path `aleph://correction/{id}` (severity-tagged, optional
`suggested_rule`) and kicks an **immediate** compress→link drain off the
critical path — the model's own "the user corrected me" judgement replaces
the old keyword `SignalDetector` (R7). The `FeedbackDistill` dream stage
(`src/memory/dreaming/stages/feedback_distill.rs`) later reads corrections
via the `aleph://correction/` path prefix (own `feedback_distill` watermark
on `compression_metadata`; runs on **both** the Consolidate and Synthesize
strategies) and asks the LLM to pick `New` / `Strengthen` / `Supersede` /
`Skip` per signal — High/Critical severities bypass the batch quorum. The
output is `feedback/` notes, surfaced two ways: normal relevance retrieval,
plus the always-on `FeedbackFloorLoader`
(`src/memory/assembler/feedback_floor.rs`) which unconditionally promotes up
to 6 High/Critical rules into the envelope's `Feedback` slot (pre-populated
like `UserProfile`, never dropped by re-rank). Corrections must NOT be
hand-written as `feedback/` notes — the distillation gate deduplicates and
strengthens them.

**Curated hot zone (`remember`).** The `remember` tool is the sole writer
of the per-agent `MEMORY.md` hot zone (see [NOTES.md](memory/NOTES.md)
sibling-concept section): `add` / `replace` / `remove`, plus an atomic
`batch` action — several operations applied all-or-nothing, with the char
budget validated on the **final** state only and duplicate `add`s inside a
batch skipped idempotently.

**Destination ladder (D1).** The single authoritative "where does a new
memory go" ladder lives in the memory-protocol prompt layer
(`src/thinker/layers/memory_protocol.rs`), first matching rung wins:
1. durable preference / identity fact / standing instruction → `remember`
(HOT); 2. user corrected you → `flag_user_correction` (self-discovered
lessons instead go to `note_manage` as `lesson` notes); 3. reusable domain
knowledge → `note_manage` (DURABLE); 4. transient task state → scratchpad,
never a memory tool. Update-over-create is preferred throughout.

**Acknowledgment contract (D4).** Successful writes return a `destination`
receipt (`remember` names the MEMORY.md hot zone; `flag_user_correction`
names the `aleph://correction/{id}` row and the feedback-distillation
rail). The prompts instruct the model to close its reply with ONE short
sentence, in the user's language, saying what was recorded and to which
tier — never quoting the stored content back verbatim, and treating the
tool's success response as terminal (no repeated writes, no re-echo into
another memory tool). This replaces the earlier "silent logging" design.

## Panel 呈现面与 RPC 形状

The desktop Panel's Memory Vault tab (`interfaces/webchat/src/platform/wide/views/memory/`, see [FEATURE_LOCATOR.md §6.7](FEATURE_LOCATOR.md)) is a pure-I/O consumer (R4) of five gateway RPCs. Each one now has exactly **one shape** — the 2026-07-26 refactor's core fix was that `memory.search` used to also answer queries with note rows, which the desktop view rendered into the Raw table (wrong data, undeletable rows, no error surfaced).

| RPC | 返回什么 | 谁消费 |
|---|---|---|
| `memory.listFacts` | 笔记页 + `total`（含 tags / link_count / updated_at） | Panel 笔记层、phone Vault |
| `memory.search` | **只有**原始对话行（`query` 做 content LIKE 过滤） | Panel Raw 层、CLI `memory search` |
| `graph.search` | **只有**笔记 FTS 命中（完整索引行，`SearchResultDto` 9 字段） | Panel SearchHits 层、星系高亮、抽屉 wikilink 解析 |
| `memory.stats` | 单一 scope 的四项计数 + `scope` 字段 | Panel 统计卡、CLI `memory stats` |
| `memory.trace` | 证据链（notes + evidence，含 `pruned`） | `memory_trace` 工具、Panel 抽屉溯源区（`provenance.rs`，2026-07-26 前零消费者） |

`graph.neighbors` is intentionally absent from this table — it was cut on 2026-07-26 (zero callers repo-wide; see FEATURE_LOCATOR.md §6.3/附录 A #7). `NoteStore::get_neighbors` remains as a Rust-level API for `note_graph_query`.
