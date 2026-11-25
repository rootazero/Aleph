# Memory Retrieval

> `NoteFactRetrieval` — hybrid search over notes, scoring, context assembly, tools, and audit.

## 1. Entry Points

`NoteFactRetrieval` is the single retrieval entry point in the current codebase. It reads from `notes_index` + `notes_vec_{dim}` + `notes_fts` via the `NoteStore` trait, fuses the two ranked lists with Reciprocal Rank Fusion, expands the candidate pool with associative graph-related peers (see Associative Graph Expansion), then hands results to the scoring pipeline (§4) or a tool caller. The earlier split retrieval implementation was removed; `NoteFactRetrieval` returns `Vec<ScoredFact>` so downstream consumers (Smart Recall, `ContextComptroller`, `memory_search`) kept their signatures. Paths on results use the `note://{category}/{filename}` scheme.

| Method | Source | Scope |
|---|---|---|
| `retrieve(query, agent_id, limit)` | `src/memory/note_retrieval/single_agent.rs` | Hybrid vector + FTS, one agent |
| `vector_retrieve(query, agent_id, limit)` | `src/memory/note_retrieval/single_agent.rs` | Pure vector, one agent |
| `text_retrieve(query, agent_id, limit)` | `src/memory/note_retrieval/single_agent.rs` | Pure FTS, rank-scored |
| `retrieve_multi_agent(query, agents, limit)` | `src/memory/note_retrieval/multi_agent.rs` | Hybrid across N agents, top-k merged |

⚠️ **`retrieve_all_agents` is gone and must not come back** (it was in this table
until 2026-08-23, four months after its deletion). It enumerated every corpus on
disk and retrieved across all of them, which is a visibility hole with a
convenient name: a caller cannot ask "who is asking" of a signature that does
not take it. Callers now enumerate with `project_scope::list_note_corpora`,
filter with `visibility::partition_visible_to`, and pass the survivors to
`retrieve_multi_agent` — the same thing minus the hole. The reasoning is kept
verbatim at the deletion site in `multi_agent.rs`.

**Module layout** (`NoteFactRetrieval`'s methods, split 2026-08-23 — pure motion,
no behaviour change): `builder.rs` construction and configuration ·
`single_agent.rs` and `multi_agent.rs` the entry points above ·
`pipeline.rs` what happens to a candidate pool before it is returned
(`fetch_limit`, `apply_scoring`, `surface_relations`, `apply_rerank`) ·
`signals.rs` recall-signal writes and the reinforcement counts they feed back ·
`mod.rs` the struct and the constants. They are inherent-impl blocks on one
type, so there is no delegation layer; the only cost of the split is that a
method one stage calls on another is `pub(super)` rather than private — the same
visibility it had when they shared a file.

The struct:

```rust
pub struct NoteFactRetrieval<S: NoteStore + Send + Sync + 'static> {
    indexer: Arc<NoteIndexer<S>>,
    embedder: Arc<dyn EmbeddingProvider>,
}
```

Core signatures:

```rust
pub async fn retrieve(
    &self,
    query: &str,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<ScoredFact>, AlephError>;

pub async fn vector_retrieve(
    &self,
    query: &str,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<ScoredFact>, AlephError>;
```

`retrieve` embeds the query once, infers `dim` from the embedding length, delegates to `NoteStore::hybrid_search_notes`, and maps each `NoteSearchResult` to a `ScoredFact` via `to_scored_fact(agent_id)`. `vector_retrieve` skips FTS and calls `vector_search_notes_with_content`. The return type is `Vec<ScoredFact>` where `ScoredFact { fact: MemoryFact, score: f32 }` — the scoring pipeline consumes this directly.

**Degradation (P7).** The vector leg can be absent three ways, and the reason to keep going is the same for all of them: the notes and the FTS index are both local and intact. No embedder configured is a steady state (skip quietly); `embed()` failing means the endpoint is unreachable; and the store itself can fail *after* a successful embed, most often because the provider's dimension has no `notes_vec_{dim}` table (§8). Until 2026-08-05 only the first two were covered — the third propagated with `?`. That mattered most here, because `retrieve_inner` is the auto-recall path: a broken vector leg silently emptied `<memory-context>` on every turn, three lines below a comment promising exactly this degradation. `retrieve_inner`, `retrieve_multi_agent`, and `note_manage`'s `search_notes` now all fall back to the keyword path.

## 2. Working Memory Assembler

Before retrieval results reach the LLM, they pass through the `WorkingMemoryAssembler` (`src/memory/assembler/mod.rs`). `HybridAssembler` is the production implementation:

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

The assembly pipeline:

1. **Retrieve** — calls `NoteFactRetrieval::retrieve` for hybrid search
2. **Re-rank** — optionally runs `AiProviderReranker` for LLM-based re-ranking.
   It pins a `"respond only with strict JSON"` system message (the rerank parser
   accepts nothing else); prose consumers take `AiProviderSummaryLlm` — see
   [MEMORY_SYSTEM.md](../MEMORY_SYSTEM.md)
3. **Hydrate** — converts `NoteSearchResult`s into `EnvelopeItem`s
4. **Extend** — applies registered `MemoryExtension::on_retrieve` hooks
5. **Render** — serializes to XML via `render_with(&env, RenderStyle::Xml)`

The `MemoryEnvelope` structure:

```rust
pub struct MemoryEnvelope {
    pub schema_version: String,      // "1"
    pub generated_at: i64,
    pub query: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub slots: Vec<EnvelopeSlot>,    // Each slot has a SlotKind
    pub meta: EnvelopeMeta,
}
```

Slots are typed by `SlotKind` (`src/memory/assembler/envelope.rs`):
- `RelevantNotes` — retrieved notes from hybrid search
- `UserProfile` — synthesized user model (pre-populated, never dropped by re-rank)
- `Feedback` — user-taught rules/corrections distilled into `feedback/` notes (pre-populated like `UserProfile`)
- `SessionRecent` — recent-session material (prior-session snapshot, daily digest)
- `RawFragments` — raw-memory fallback fragments
- `Nudges` — proactive nudge items

The rendered XML is injected as a `role=user` message containing a fenced `<MemoryEnvelope>` block. All user-supplied fields are `xml_escape`d. See [MEMORY_SYSTEM.md §5](../MEMORY_SYSTEM.md) for the full assembler architecture.

## 3. Hybrid Search Algorithm

`SqliteMemoryBackend::hybrid_search_notes` in `src/memory/store/sqlite/notes/store_impl.rs` is the concrete implementation behind `NoteStore::hybrid_search_notes`. It takes the query embedding, the raw query text, an `agent_id`, a `dim_hint`, and a `limit`, and returns a `HybridSearchOutcome { results, vector_candidates, fts_candidates }` with full content loaded from disk (concurrently — a result set's wall time used to be the sum of its disk reads). The per-leg counts exist so a caller can report what actually ran: `"hybrid"` with `vector_candidates == 0` means the semantic half did not participate at all, which is a different instruction to the model than a semantic search that simply found nothing. `note_manage(query)` surfaces this as `SearchAdvisory`.

The algorithm has four steps:

1. **Parallel ranked retrieval.** The backend issues two lookups, each capped at `limit * 2`:
   - `vector_search(embedding, dim_hint, agent_id, limit * 2)` against `notes_vec_{dim}` via `sqlite-vec`.
   - `search_notes_fts(query_text, agent_id, limit * 2)` against the `notes_fts` virtual table.
2. **Reciprocal Rank Fusion.** Both ranked lists are merged into a single `HashMap<String, f32>` keyed by note path, using the RRF formula with `k = 60.0` (the canonical Cormack constant):

   ```rust
   let k = 60.0_f32;
   let rrf = 1.0 / (k + (rank as f32) + 1.0);
   *scores.entry(path.clone()).or_insert(0.0) += rrf;
   ```

   Paths that appear in both lists accumulate scores from each — this is why hybrid beats either ranker in isolation. ⚠️ This paragraph used to end by naming a shared `rrf_fuse` helper in `note_retrieval/hybrid.rs`: that file does not exist, and the `rrf_fuse` that does (`src/context/retrieval/content_index.rs`) fuses the porter and trigram FTS indexes for the *context* content index — same math, different subsystem, never called from here. The fusion below is the only one on this path.
3. **Sort and truncate.** The fused map is converted to a vector, sorted descending by score, and truncated to `limit`.
4. **Content hydration.** For each surviving path the backend calls `get_note_index(path, agent_id)` and `load_note_content_from_disk(entry, agent_id)` to read the markdown file at `memory/note/{agent_id}/{category}/{filename}.md`. The result is assembled into a `NoteSearchResult` carrying `path`, `filename`, `category`, `tags`, `content`, `score`, `created_at`, `updated_at`.

```text
 query_text ─┬─► notes_fts (BM25)        ─► [path@rank] ─┐
             │                                            ├─► RRF k=60 ─► top-k paths ─► disk load
 embedding ──┴─► notes_vec_{dim} (L2)    ─► [path@rank] ─┘                   │
                                                                              ▼
                                                                   Vec<NoteSearchResult>
```

Both lists over-fetch `limit * 2` to give RRF enough signal for the final top-k pick.

## Associative Graph Expansion (pre-rerank)

Between `hybrid_search_notes` and the cross-encoder rerank, `retrieve()` runs
`note_retrieval::expansion::graph_expand`. For the top `max_seeds` direct hits it
looks up each seed's strongest 5-signal related peers (`NoteStore::related_peers`,
materialized per dream cycle in `notes_graph_related`), dedups them against the
direct hits, hydrates their content (`NoteStore::get_notes_with_content`), and
adds them to the candidate pool with a propagated score
`seed.score * weight * (edge / seed_top_edge)` — scaled strictly below the seed,
so a peer can displace a *weak* direct hit but never a strong one. This is
associative / multi-hop recall: a note surfaces because it is tied to a
query-relevant note, even without lexical or semantic overlap with the query.

Controlled by `memory.expansion` (`ExpansionConfig`: `enabled`, `max_seeds`,
`peers_per_seed`, `max_expanded`, `weight`). Default-on and conservative. A cold
graph cache (pre-first-dream) makes `related_peers` empty, so expansion is a
no-op and ranking is byte-for-byte legacy; `enabled = false` does the same. Store
errors inside expansion are swallowed (logged) — a graph-cache problem never
fails core recall. The same stage runs per-agent in `retrieve_multi_agent`.

## 4. Bridge to Legacy Types

`NoteSearchResult` lives in `src/memory/notes/search_result.rs`. It is the hybrid-search native type and the bridge that lets notes-sourced results flow into code that still types against the legacy `MemoryFact` / `ScoredFact` DTOs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSearchResult {
    pub path: String,
    pub filename: String,
    pub category: String,
    pub tags: Vec<String>,
    pub content: String,
    pub score: f32,
    pub created_at: i64,
    pub updated_at: i64,
}

impl NoteSearchResult {
    pub fn to_memory_fact(&self, agent_id: &str) -> MemoryFact {
        let note_type = NoteType::from_str_or_other(&self.category);
        let mut fact = MemoryFact::new(self.content.clone(), note_type, self.tags.clone());
        fact.id = self.path.clone();
        fact.path = format!("note://{}", self.path);
        fact.agent = agent_id.to_string();
        fact.created_at = self.created_at;
        fact.updated_at = self.updated_at;
        fact.is_valid = true;
        fact
    }

    pub fn to_scored_fact(&self, agent_id: &str) -> ScoredFact {
        ScoredFact {
            fact: self.to_memory_fact(agent_id),
            score: self.score,
        }
    }
}
```

Downstream consumers still receive `ScoredFact<MemoryFact>`, but the path is now `note://{category}/{filename}` and `is_valid` is hard-coded to `true`. Category tags ride along as `source_memory_ids` on the fact. The legacy `tier` / `scope` / `strength` / `confidence` assignments were removed as part of the memory sovereignty cleanup — those fields no longer exist on `MemoryFact`.

## 5. Scoring Pipeline

After hybrid fusion and any configured cross-encoder rerank, `NoteFactRetrieval` can adjust ranking with the following stages. The implementation is in `src/memory/note_retrieval/pipeline.rs` and `src/memory/note_retrieval/scoring.rs`; the config and defaults are in `src/config/types/memory/retrieval.rs`.

| Stage | Effect | Defaults |
|---|---|---|
| Recency | Blends relevance with an exponential decay based on the note's update time | `recency_enabled` on, `recency_half_life_days` 90, `recency_weight` 0.3 |
| Reinforcement | Boosts relevance sublinearly with historical recall count | `reinforcement_enabled` on, `reinforcement_weight` 0.3 |
| Diversity | Greedy MMR uses token overlap to reorder similar candidates; smaller lambda favors diversity | `mmr_enabled` off, `mmr_lambda` 0.7 |

The old six-stage scoring pipeline and its tuning knobs were removed. Salience factors above adjust ranking; they do not assign durable confidence or importance to a note.

## 6. Reranker (Optional, configured)

`src/memory/rerank/` implements HTTP cross-encoder reranking against five providers. The trait (from `provider.rs`):

```rust
#[async_trait]
pub trait RerankProvider: Send + Sync {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError>;
    fn provider_id(&self) -> &str;
}
```

`build_provider(&RerankConfig) -> Box<dyn RerankProvider>` dispatches on `RerankProviderType`:

| Provider file | `provider_id()` | Default model |
|---|---|---|
| `src/memory/rerank/jina.rs` | `jina` | `jina-reranker-v2-base-multilingual` |
| `src/memory/rerank/siliconflow.rs` | `siliconflow` | *(from `RerankConfig.models[0]`)* |
| `src/memory/rerank/voyage.rs` | `voyage` | *(from `RerankConfig.models[0]`)* |
| `src/memory/rerank/pinecone.rs` | `pinecone` | *(from `RerankConfig.models[0]`)* |
| `src/memory/rerank/vllm.rs` | `vllm` | *(from `RerankConfig.models[0]`)* |

Config (`RerankConfig` in `provider.rs`) carries `enabled: bool` (default `false`), `provider`, `api_base`, `api_key` (vault-backed, never serialized), `models: Vec<String>`, `timeout_ms: 5000`, `rerank_weight: 0.6`. The `blend_scores` helper (`src/memory/rerank/mod.rs`) computes `final = rerank_weight * rerank_score + (1 - rerank_weight) * original_score` for sorted pairing. ⚠️ This paragraph claimed until 2026-08-23 that reranking was **not** wired into `NoteFactRetrieval`, and pointed at a file that does not exist. It is wired: `with_reranker` / `with_rerank_config` install it and `pipeline.rs::apply_rerank` is the stage, over-fetching by `RERANK_CANDIDATE_MULTIPLIER` (capped at `RERANK_MAX_CANDIDATES`) so the cross-encoder has a pool to reorder.

## 7. Query Expander (Removed)

The unwired query expander was removed. It exported `fn expand(query: &str) -> ExpandedQuery { original, bm25_query }`. When the input contains any CJK Unified Ideograph (`\u{4E00}..=\u{9FFF}`), the expander appends known Chinese synonyms (`喜欢 → 偏好 倾向 爱好`, `问题 → bug 错误 故障 缺陷`, and ten other groups from the static `SYNONYMS` table) to the BM25 query. The `original` field is left verbatim for vector search. No config key — the synonym table is hardcoded. It was never wired into `NoteFactRetrieval`; hybrid search currently passes the raw query to both the FTS and embedding lookups.

## 8. Embedding Provider

### 7.1 Trait

From `src/memory/embedding_provider.rs`:

```rust
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
    fn provider_id(&self) -> &str;
}
```

### 7.2 RemoteEmbeddingProvider and `create_provider`

`RemoteEmbeddingProvider` is the only production implementation. It wraps a `reqwest::Client` with a per-request timeout, stores `api_base`, `api_key`, `model`, `dimension`, `batch_size`, and `provider_id`, and posts to the OpenAI-compatible `/v1/embeddings` endpoint. `call_api` normalizes the URL (appending `/v1` if missing), sets `Authorization: Bearer {api_key}` when the key is non-empty, sends `{"input": texts, "model": self.model, "dimensions": self.dimension}`, verifies the response content-type is JSON (catches misconfigured base URLs), and runs each returned vector through `truncate_and_normalize` to trim oversized vectors down to the configured dimension with L2 renormalization.

`pub fn create_provider(config: &EmbeddingProviderConfig) -> Result<Arc<dyn EmbeddingProvider>, AlephError>` is the single factory — it just wraps `RemoteEmbeddingProvider::from_config(config)` in an `Arc`. There is no local-process fallback; all embeddings are remote.

### 7.2.1 Local-first `auto` resolution

`src/memory/embedding_resolver.rs` maps `EmbeddingSettings.active_provider_id` onto a concrete provider before `create_provider` is called. `EmbeddingManager::init` uses `resolve(&settings) -> EmbeddingDecision { requested_id, effective, reason }`:

- A **pinned** id (anything other than `""`/`auto`) wins by exact match against an *enabled* provider (`ExactMatch`). A pinned-but-missing/disabled id stays `Unresolved` — it never silently swaps in a different backend (`也可按需切到 OpenAI / Ollama` stays explicit).
- An **empty or `auto`** id triggers local-first selection: the first enabled provider whose preset is `Ollama` (`EmbeddingLocality::Local`, 数据不出本机) is preferred (`AutoLocalFirst`), else the first enabled provider (`AutoRemoteFallback`).
- No usable provider → `Unresolved`; semantic memory degrades to keyword-only (the shipped default — empty providers + empty id — resolves here, byte-identical to the prior behaviour).

`reason.as_str()` is emitted in the init log so the chosen backend and *why* are observable. This is pure deterministic routing (no heavy dependency, no LLM reasoning) — the R3-safe half of "本地嵌入": Ollama already keeps data on-device, and `auto` makes it the default without manual config. A bundled in-process ONNX backend remains deliberately out of core (see [MEMORY_SYSTEM.md](../MEMORY_SYSTEM.md) §Embeddings, redline R3).

### 7.3 Provider Presets

From `src/config/types/memory/embed.rs`:

| Preset | `api_base` | Default model | Dimensions |
|---|---|---|---|
| SiliconFlow | `https://api.siliconflow.cn/v1` | `BAAI/bge-m3` | 1024 |
| OpenAI | `https://api.openai.com/v1` | `text-embedding-3-small` | 1536 |
| Ollama | `http://localhost:11434/v1` | `nomic-embed-text` | 768 |
| Custom | *(user-supplied)* | *(user-supplied)* | *(user-supplied)* |

**Multi-dimension rationale.** The notes store keeps one virtual table per supported dimension so that switching providers does not invalidate existing embeddings. A query carries its `dim` hint, looks up in the matching table, and never mixes spaces. The supported set is the single source `vec::EMBEDDING_DIM_TABLES` — **384, 768, 1024, 1536, 3072** — from which table creation, dimension lookup, and the delete-path sweep are all derived.

Until 2026-08-05 the set was spelled out in five places and stopped at {768, 1024, 1536}. A 384-dim provider (`all-MiniLM-L6-v2`, the most common local embedder) or a 3072-dim one (`text-embedding-3-large`) produced a deployment with **no note vectors at all**: every `upsert_embedding` failed into a swallowed `warn!` and every hybrid read failed outright. The two halves read together as "semantic search is broken" rather than "a table is missing". `CREATE VIRTUAL TABLE IF NOT EXISTS` is idempotent, so an existing database picks up a newly supported dimension on the next open.

**Embed freshness.** `notes_vec_map` carries `embedded_hash` + `embedded_at`: the note's `content_hash` at the moment its vector was computed. Embed-on-write logs and swallows its failures by design (the note is already on disk), so without this nothing could tell a current vector from one left behind by a network blip, and `reembed_all` had to redo the whole corpus to be sure. `NoteStore::stale_vector_paths` is the read side; `reembed_all` skips fresh notes and `full_rebuild` reports `IndexStats.stale_vectors`. An empty hash means provenance unknown and reads as stale, so a caller that cannot vouch for what it embedded errs toward re-embedding. The skip is suppressed whenever the embedding **signature** (provider + model + dimension) changed: an equal content hash then says the text is unchanged, which is not the question once the vector space itself has moved.

## 9. Context Assembly

### 9.1 Context budget trimming

`memory_search` uses `ContextComptroller` to trim results to a caller-supplied token budget. The current implementation is under `src/memory/context_comptroller/`; its config has no retention-mode, deduplication-threshold, or token-budget setting.

## 10. Removed LLM-only retrieval experiment

The experiment asked a model to select IDs from a supplied list of candidates and fell back to recent candidates on failure. It had no production consumer. Use the note retrieval entry points in §1.

## 11. RippleTask

`src/memory/ripple/` performs local knowledge-graph expansion around seed facts by BFS over vector similarity.

```rust
pub struct RippleConfig {
    pub max_hops: usize,                // default 2
    pub max_facts_per_hop: usize,       // default 5
    pub similarity_threshold: f32,      // default 0.7
    pub enable_tunnels: bool,           // default true (stub)
    pub max_tunnel_hops: usize,         // default 1 (stub)
}

pub struct RippleResult {
    pub seed_facts: Vec<MemoryFact>,
    pub expanded_facts: Vec<MemoryFact>,
    pub total_hops: usize,
    pub tunnel_facts: Vec<MemoryFact>,
}
```

`RippleTask::explore(seed_facts)` maintains a `HashSet<String>` of visited IDs seeded from the input, then iterates up to `max_hops`. At each hop, for every fact with an embedding it calls `NoteStore::vector_search_notes_with_content(embedding, agent_id, dim, max_facts_per_hop)`, converts hits to `MemoryFact` with `similarity_score` attached, drops anything already visited, and keeps those above `similarity_threshold` as the next level. BFS stops when `current_level` is empty. The current implementation does not do a separate tunnel expansion.

## 12. Memory Tools

### 11.1 `memory_search`

Source: `src/builtin_tools/memory_search.rs`. The primary recall tool. Args: `query`, `max_results` (default 10), optional `workspace` / `workspaces` / `cross_workspace` (single / list / all), and `scope: "all" | "current_session" | "both"`.

Execution: (1) If scope includes `current_session`, queries `RawMemoryStore::get_raw_by_path_prefix("aleph://session/{session_key}/", agent_id, ...)` with a case-insensitive substring filter — this is the session-local path that replaced the deleted facts table. (2) If scope includes `all`, calls `NoteFactRetrieval::retrieve(query, agent_id, max_results)`, optionally via Smart Recall (Phase 1 primary workspace → Phase 2 `retrieve_multi_agent` over the visibility-narrowed corpora when primary count falls below `min_primary_results`), and additionally fills the `transcripts` leg via `RawMemoryStore::search_raw_by_path_prefix(TRANSCRIPT_PATH_PREFIX, agent, query, ...)` — newest-first substring recall over the rows `TranscriptIndexer` writes, agent-scoped, capped at 5 results with per-field char truncation announced in-band. (3) The fact paths feed a `ContextComptroller` with a 100 000-token budget for arbitration. (4) `cluster_facts_by_path` groups results by path with `threshold = 3` to surface hot regions.

Output: `MemorySearchOutput { facts, transcripts, query, tokens_saved, path_clusters, cross_workspace, smart_recall_triggered }`.

### 11.2 `memory_browse`

Source: `src/builtin_tools/memory_browse.rs`. Filesystem-only browser, no database. Args: `action: "list" | "read"` and optional `path`.

- `action=list` with no `path`: returns top-level category directories under `memory/note/{agent_id}/` (skipping hidden and `archive`).
- `action=list` with `path=<category>`: returns `.md` filenames in that category with the extension stripped.
- `action=read` with `path=<category>/<filename>`: returns the markdown body of `memory/note/{agent_id}/{category}/{filename}.md`.

Output: `MemoryBrowseOutput { success, message, entries, content }`. This replaces the old VFS-backed browse model — notes are real files, so listing and reading are plain `tokio::fs` calls.

### 11.3 `memory_explore`

Source: `src/builtin_tools/memory_explore.rs`. Wraps `RippleTask` for multi-hop discovery. Args: `query`, `max_hops` (default 2, clamped to 4), `max_per_hop` (default 5, clamped to 10).

Execution: embed query → `NoteStore::vector_search_notes_with_content` for three seed notes → load each seed's stored embedding via `NoteStore::get_embedding` so RippleTask has vectors to expand with → `RippleTask::new(backend, RippleConfig { max_hops, max_facts_per_hop: max_per_hop, similarity_threshold: 0.7, .. }, agent_id).explore(seeds)`. Output: `MemoryExploreOutput { seed_facts, expanded_facts, hops_performed, summary }` where each `ExploredFact` carries `{ id, content, path, relevance_score }`.

### 11.4 `recall_context`

Source: `src/builtin_tools/recall_context.rs`. Retrieves pre-compression conversation raw chunks. Args: `query` (string for the LLM's reference, not used as a filter) and `max_results` (default 3). Execution: builds `path_prefix = "aleph://session/{session_id}/raw/"` and calls `RawMemoryStore::get_raw_by_path_prefix(prefix, "default", max_results)`. Output: `RecallContextResult { fragments: Vec<RecalledFragment { content, relevance_score, source_path }>, query }`. This is how the LLM fetches specific code snippets or error text that existed in the transcript before session compression. See `RAW_MEMORY.md` §7.2 for the raw-memory store contract this tool sits on top of.

## 13. Audit and Explainability

The former per-fact audit store and its unused `AuditEntry` types were removed. The current fact-lifecycle read model lives in `src/memory/explain.rs`; the audit types below are historical.

```rust
pub struct AuditEntry {
    pub id: String,
    pub fact_id: String,
    pub action: AuditAction,
    pub reason: Option<String>,
    pub actor: AuditActor,
    pub details: Option<AuditDetails>,
    pub created_at: i64,
}
```

`AuditAction` covers `Created | Accessed | Updated | Invalidated | Restored | Deleted`. `AuditActor` covers `Agent | User | System | Decay`. `AuditDetails` is a `#[serde(tag = "type")]` enum with payloads per action — `Created { source, extraction_context }`, `Accessed { query, relevance_score, used_in_response }`, `Updated { old_content, new_content, reason }`, `Invalidated { reason }`, `Restored { new_strength }`, `Deleted { reason, days_in_recycle_bin }`.

The former audit vocabulary described two derived views. `FactExplanation { fact_id, content, is_valid, creation_source, access_count, invalidation_reason, events: Vec<ExplainedEvent> }` reconstructs a fact's lifecycle timeline. `ExplainedEvent { timestamp, action, description, actor }` is one line of that timeline. `ForgettingExplanation { fact_id, reason, actor, timestamp, days_since_creation, explanation }` is the specialized view for "why did this disappear?".

**Former audit design (not a current explain path).** To trace why a note was returned or dropped: look up the fact by `fact_id`, materialize `Vec<AuditEntry>` ordered by `created_at`, fold them into a `FactExplanation`, and for each `Accessed` event inspect the `relevance_score` and `query`. For forgetting, materialize a `ForgettingExplanation` from the final `Invalidated` entry. This is the read side of memory observability; the write side — event-sourced note mutations — is covered in `NOTES.md` §12.

## 14. Reflection / Synthesis (Spec 2)

`MemoryReflector` at `src/memory/reflector/` composes the hybrid assembler with an LLM synthesis pass. Given a natural-language query, it:

1. calls `HybridAssembler::assemble(query, agent_id, session_id, budget)` for retrieval
2. returns a stub `Synthesis { text: "No relevant memories found.", sources: [] }` immediately if the envelope has zero items (no LLM cost)
3. otherwise formats the envelope into a user prompt via `envelope_to_synthesis_context`, calls the LLM with `PROMPT_SYNTHESIS`, parses the JSON response, **overlays canonical titles from the lookup** (so LLM-fabricated paths are dropped and titles cannot be hallucinated), and returns a `Synthesis { text, sources }` where each `NoteRef = { path, title, relevance }`.

Side effect: one `recall_signals` row per note in the synthesis context, `channel = "reflect"`. Failures are logged but swallowed — recall-signal write errors must never fail a `reflect()` call. The dream-daemon decay logic observes these signals and treats reflect-touched notes as active memory on par with the primary-path recall hits described in §14.1.

### 14.1 Hot-floating recall loop (`recall_signals` producer + consumer)

The "热门记忆浮顶" / reinforcement-salience behaviour is a closed producer→consumer loop over the `recall_signals` table:

- **Consumer** — `NoteFactRetrieval::fetch_reinforcement_counts` reads `NoteStore::recall_hit_counts` (SQLite `aggregate_for_facts`, `signal_count` per note) and `scoring::apply_reinforcement` boosts each candidate by `1 + w·ln(1 + hits)` (default `reinforcement_weight = 0.3`, default-on). This was always wired.
- **Producer** — every primary retrieval (`NoteFactRetrieval::retrieve` and `retrieve_multi_agent`, used by the `memory_search` tool *and* the proactive `MemoryContextProvider` injection) records the *surfaced* notes (after rerank/scoring/truncation) via `NoteStore::record_recall_hits` → the existing `record_signals` writer, `channel = "auto-recall"`. Best-effort (write failures are logged at `debug` and never break recall) and gated on `reinforcement_enabled`, so disabling hot-floating also stops recording.

> **Signals are filed per owning namespace, never under a "representative" label.** `retrieve_multi_agent` serves the project-scoped read union, and `project_scope::read_scope_ids` returns `[base, scoped]` — so labelling every hit with `agent_ids.first()` (as this path did until 2026-08-01) filed *every project note's* hit under the base namespace. Both downstream consumers read under a **specific** id: `NoteDecay`'s `access_weight` and the evolution recall-evidence gate run with the dream context's *scoped* id. The result was project notes that looked never-recalled (and were archived early) while the base namespace accrued phantom heat for notes it does not own. `to_scored_fact` already stamps the true owner onto `fact.agent`; group by it (`record_recall_by_owner`). Reinforcement counts are keyed `(owner, path)` — two namespaces can hold notes at the same relative path, and a bare-path map lets one namespace's heat leak into the other's ranking.
>
> **A hit that contributes nothing to the prompt must not earn a signal.** The FTS-only leg (`text_retrieve`, used when no embedder is configured *and* when the embed endpoint is down) built facts from index rows, which carry no body — so the model received titles with empty content while the recall signal was still written, durably teaching reinforcement that empty notes are hot. It now hydrates through `get_notes_with_content` (the same trait method the hybrid path uses) and *skips* any hit whose body cannot be loaded.

Both halves dedup on `UNIQUE(note_path, query_hash, day_bucket, channel)`, so a note must surface across **distinct queries / days** to genuinely heat up — repeated recalls of the same note for the same query on the same day count once, which bounds the rich-get-richer feedback. The `auto-recall` channel is kept distinct from `reflect` so the two producers dedup independently. Prior to this wiring the boost was effectively inert: only the narrow `memory_reflect` synthesis path wrote signals, so notes recalled through the primary paths never accrued hotness.

LLM-facing entry: the `memory_reflect` builtin tool (`src/builtin_tools/memory_reflect.rs`), schema `{ "query": string }`, returns `Synthesis` as JSON.

Internal-caller entry: `MemoryReflector::reflect(query, ReflectOpts) -> Result<Synthesis, AlephError>`. `ReflectOpts` carries `agent_id`, `namespace`, optional `max_tokens` / `time_range` / `session_id`.

See `docs/superpowers/specs/2026-04-13-memory-evolution-spec2-reflector-design.md`.

## 15. Context Fencing + Injection Modes (Spec 3)

Recalled memory is injected into the LLM prompt as an independent `role=user` message containing a fenced XML envelope:

```xml
<MemoryEnvelope>
  <schema_version>1</schema_version>
  <query>...</query>
  <slot kind="...">
    <item id="..."><title>...</title><content>...</content></item>
  </slot>
</MemoryEnvelope>
```

All user-supplied fields are `xml_escape`d so evil content in a note cannot break the fence. A unit test invariant verifies that exactly one `</MemoryEnvelope>` appears in the rendered output regardless of content.

`MemoryConfig.injection_mode` controls the surface:

| Mode      | Auto-inject | `memory_*` retrieval tools registered |
|-----------|-------------|---------------------------------------|
| `Context` | yes         | no                                    |
| `Tools`   | no          | yes                                   |
| `Hybrid`  | yes         | yes                                   |

Default is `Hybrid` (pre-Spec-3 behaviour). The six retrieval tools gated by mode are: `memory_search`, `memory_reflect`, `recall_context`, `memory_browse`, `memory_explore`, `memory_timeline`. `note_manage` and `session_complete` are always registered — they are write-side and task-boundary tools unaffected by retrieval mode.

`MemoryContextProvider::build_memory_user_message` renders the envelope using the assembler's configured style, wraps the rendered result with `wrap_memory_context` and returns a `UnifiedMessage::user`. See `src/thinker/memory_context_provider/memory.rs` and `src/thinker/layers/memory_window.rs` for the injection boundary.

See `docs/superpowers/specs/2026-04-13-memory-evolution-spec3-fencing-modes-design.md`.

## 16. Pluggable Memory Extensions (Spec 4)

The memory pipeline exposes three hook points — `on_retrieve`, `on_capture`, and `produce` — through the `MemoryExtension` trait. First-party Aleph code registers implementations in-process; third-party plugins register over MCP through the existing plugin manifest by declaring a `[memory]` section. Dispatch semantics: `on_retrieve` broadcasts (2s per-plugin timeout); `on_capture` chains with fail-safe Block on error/timeout (3s); `produce` runs per-plugin with 30s timeout under a dedicated scheduler. See `docs/reference/memory/EXTENSIONS.md` for full details.

## Appendix: Retrieval Tuning Tips

- Tune `recency_half_life_days` and `recency_weight` if recent notes should have more or less influence.
- Turn on `mmr_enabled` and lower `mmr_lambda` to favor diversity when top results repeat the same phrasing.
- Measure latency and retrieval quality before turning on a cross-encoder: `RerankConfig` controls its timeout and score blend, and the reranker adds a network round trip.
- `RippleConfig.similarity_threshold` controls the reach of multi-hop vector exploration. Raise it for a tighter cluster.
## 17. Cross-session summary retrieval (Spec B)

The `session_search` tool returns one synthesized excerpt per matched session,
plus 0-2 raw evidence quotes for grounding. Summaries come from three
coordinated paths: existing compactor d0/d1/d2 facts, the on_session_end
backstop, and a lazy on-read fallback for short in-flight sessions. All three
write the same canonical fact at `aleph://session/{sid}/end-summary` (compactor
variants live at `aleph://session/{sid}/d{depth}/{seq}`). The wiki/note
retrieval path (default `FactSourceFilter::Any`) is unaffected — the new
filter is only passed by `session_search` itself, leaving every other
HybridAssembler caller's behaviour byte-identical.

## See Also

- [Knowledge Notes (L1)](NOTES.md) — the markdown + SQLite substrate that hybrid search reads from.
- [Raw Memory Store (L0)](RAW_MEMORY.md) — the session-scoped chunk store behind `recall_context` and `memory_search`'s session path.
- [Dream Daemon](DREAM_DAEMON.md) — the offline consolidation pipeline that writes the notes retrieval reads.
