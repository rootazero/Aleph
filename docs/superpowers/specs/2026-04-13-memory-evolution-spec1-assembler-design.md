# Memory Evolution — Spec 1: Working Memory Assembler + Envelope

> **Spec 1 of a 4-part memory evolution roadmap.** Defines the portable `MemoryEnvelope` data contract and the `WorkingMemoryAssembler` that produces it on every LLM call. No evolution loop, no MCP server — those are Spec 2 and Spec 3.

**Date:** 2026-04-13
**Status:** Draft (design phase, not implemented)
**Author:** brainstormed with user, pinned to LLM-sovereignty principles

---

## 1. Context & Motivation

### 1.1 Current State

Aleph's memory stack has already been through a sovereignty cleanup:

- **L0 `raw_memories`** — ephemeral SQLite buffer; consumed by `CompressionService`.
- **L1 Notes** — persistent markdown files + SQLite indexes (`notes_index`, `notes_fts`, `notes_vec_{768,1024,1536}`, `notes_links`).
- **Retrieval** — `NoteFactRetrieval` with hybrid RRF (vector + FTS) and a 6-stage scoring pipeline (cosine_rerank, recency_boost, length_normalization, time_decay, hard_min_score, mmr_diversity).
- **Dream Daemon** — offline consolidation (consolidate, drift, synthesis, lint, decay, daily_digest).
- **Removed** — `tier`, `scope`, `strength`, `confidence`, `ImportanceWeight`, `ValueEstimator`, `ApplyDecayCommand`, `MemoryScope`.

The missing piece: **every turn's actual context injection is still dumb.** `ContextComptroller::arbitrate` only sorts retrieved facts by similarity and trims by token budget — no slot semantics, no LLM judgment about what the current query needs, no portable output format.

### 1.2 User Goals

1. **Maximize short-term memory precision** — what actually reaches the LLM context window each turn.
2. **Make the memory system portable** — exportable / callable by other agents (mem0, Honcho, Claude Code, Cursor, custom MCP clients).

### 1.3 Anchoring Decisions (from brainstorm)

The following were decided collaboratively and now anchor this spec:

| Decision | Choice | Rationale |
|---|---|---|
| "Short-term memory" scope | **Working Memory (context window)** | Where LLM's perceived "short-term" actually lives; highest leverage; L0/C are well-served already |
| Portability target | **P3 (MCP server) as main line**, P1 (internal) as subset, P2 (mem0/Honcho adapters) deferred | MCP is de facto standard; aligns with R7 (one-core, many-shells) and R9 (everything is a tool) |
| Portability delivery priority | **i (export format) first → ii (callable) follows** | Schema is the true artifact; transport (MCP/CLI) is interchangeable |
| Assembly strategy | **B (retrieval + LLM re-rank) with C (deterministic skeleton) fallback** | Keeps retrieval's proven cost/quality; LLM only in last-mile decision; fallback guarantees liveness |
| Evolution feedback signals (Spec 2) | S1 LLM citation + S3 re-retrieval + S5 existing `recall_signals` + S4 user feedback; S2 follow-up deferred | Closed loop: S1 "was it right?" + S3 "was it enough?"; S5 as baseline; S4 as authoritative override |
| Citation collection (Spec 2) | **③ Dream Daemon replay** | Zero prompt pollution; consistent with offline-evolution pattern |
| Evolution weight granularity (Spec 2) | **β (query-category buckets) + γ (per-note assembly value)** | α is already what the scoring pipeline does; β and γ are the orthogonal evolution axes |
| Scope decomposition | **4-spec breakdown**, this doc is Spec 1 | Each sub-spec is independently valuable and testable; a single spec would be multi-month |

---

## 2. Spec 1 Scope

### 2.1 In Scope

1. **`MemoryEnvelope` v1.0 schema** — the portable data contract, with `schema_version`, `slots`, `items`, `meta`.
2. **`WorkingMemoryAssembler` trait** + default implementation `HybridAssembler`.
3. **B-strategy assembly flow** — Candidate Gather → LLM Re-rank → Content Hydration.
4. **C-strategy skeleton fallback** — deterministic slot budgets, triggered by timeout / parse error / empty pool / config flag.
5. **Envelope renderer** — pure function `render_envelope(&MemoryEnvelope) -> String` with XML-tagged markdown default.
6. **Integration point** — replace `ContextComptroller::arbitrate` call site in `src/agent_loop/adapters/memory_adapter.rs`; keep `ContextComptroller` as delegate shell for backward compat.
7. **Configuration** — new `AssemblerConfig` + `FallbackSkeleton` in `src/config/types/memory.rs`.
8. **Observability** — structured `tracing` event per assembly + optional `assembly_logs` SQLite table (schema defined, default disabled).
9. **Forward-compat promises** to Spec 2 (evolution) and Spec 3 (MCP server) — schema stability, extension points, trait stability.

### 2.2 Out of Scope (deferred)

| Deferred Item | Where |
|---|---|
| Evolution loop — S1/S3/S5 signal aggregation, β/γ weight learning | **Spec 2** |
| `AssemblyFeedbackStage` in Dream Daemon | **Spec 2** |
| Citation replay / re-retrieval telemetry | **Spec 2** |
| `Nudges` slot actual population | **Spec 2** |
| MCP Memory Server shell | **Spec 3** |
| CLI `aleph memory export/import` | **Spec 3** |
| P1 cross-agent envelope migration | **Spec 3** |
| mem0 / Honcho / supermemory adapters | **Spec 4 (on demand)** |
| S2 follow-up signal (user re-asks same topic) | **v2+** |

### 2.3 Non-goals

- Not replacing `NoteFactRetrieval` — assembler **consumes** retrieval.
- Not replacing the scoring pipeline (cosine_rerank, recency_boost, etc.) — those still run inside retrieval.
- Not writing notes, not mutating L0/L1 — pure read-side.
- Not making `memory_search` tool obsolete — it stays as explicit LLM-initiated recall; assembler is implicit framework-driven assembly.

---

## 3. Architecture

### 3.1 Module Placement

New crate module: `src/memory/assembler/`, sibling to `context_comptroller/`, `note_retrieval/`, `dreaming/`.

```text
src/memory/assembler/
├── mod.rs              # public exports, trait, AssemblerConfig re-export
├── envelope.rs         # MemoryEnvelope, EnvelopeSlot, EnvelopeItem, SlotKind, ItemSource, EnvelopeMeta
├── hybrid.rs           # HybridAssembler (default impl) — B + C flow
├── gather.rs           # Candidate gather (fan-out to retrieval / session / raw / profile)
├── rerank.rs           # LLM re-rank prompt, response parsing, validation
├── fallback.rs         # Deterministic skeleton fallback packer
├── hydration.rs        # Content load + UTF-8 safe truncation + token estimation
├── render.rs           # Pure render_envelope function + RenderStyle enum
└── tests/              # module-level integration tests
```

### 3.2 Trait

```rust
#[async_trait]
pub trait WorkingMemoryAssembler: Send + Sync {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
    ) -> Result<MemoryEnvelope, AlephError>;
}

pub struct AssemblyBudget {
    pub total_tokens: u32,
}
```

**Contract: `assemble` never returns `Err` for LLM-assist failures.** Internal errors (retrieval failure, rerank timeout, hydration miss) are caught and degraded — envelope is always produced, possibly with empty slots. `Err` only surfaces for system-level misconfiguration (missing AppContext dependencies). Rationale: memory assembly is auxiliary to LLM response; its failure must not block the user reply. Worst case: empty envelope → LLM answers without memory context.

### 3.3 Placement in Call Graph

```text
┌─ agent loop (per turn, before LLM call) ──────────────────────┐
│                                                                │
│  memory_adapter::build_memory_context(query, agent_id, ...)   │
│         │                                                      │
│         ▼                                                      │
│  assembler.assemble(query, agent_id, session_id, budget)      │
│         │                                                      │
│    ┌────┴──── Stage 1: Candidate Gather (tokio::join!) ──┐    │
│    │                                                      │    │
│    ├─► NoteFactRetrieval::retrieve(query, limit=20)       │    │
│    ├─► SessionSummarySource::latest(session_id, 3)        │    │
│    ├─► RawMemoryStore::get_raw_by_path_prefix(...)        │    │
│    └─► UserProfileLoader::load(agent_id)                  │    │
│         │                                                      │
│         ▼                                                      │
│    ┌──── Stage 2: LLM Re-rank (timeout 800ms) ──────────┐    │
│    │ fast-model call returning strict JSON               │    │
│    │ timeout / parse fail / empty pool ──► Stage 2'      │    │
│    └──────────────────────────────────────────────────────┘    │
│         │                                                      │
│         ▼                                                      │
│    ┌──── Stage 2' (Fallback C): Skeleton Packer ─────────┐    │
│    │ fixed slot budgets, greedy pack by (relevance *     │    │
│    │ recency_boost)                                       │    │
│    └──────────────────────────────────────────────────────┘    │
│         │                                                      │
│         ▼                                                      │
│    ┌──── Stage 3: Hydration + Token Pack ────────────────┐    │
│    │ load content from disk, UTF-8 safe truncate, fill   │    │
│    │ EnvelopeItem.content, tally tokens_used per slot    │    │
│    └──────────────────────────────────────────────────────┘    │
│         │                                                      │
│         ▼                                                      │
│    MemoryEnvelope ──► render_envelope() ──► prompt_block       │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

### 3.4 Replaced / Retained Modules

| Module | Action |
|---|---|
| `ContextComptroller::arbitrate` | **Retained as delegate shell** — internally calls assembler and wraps output in legacy `ArbitratedContext`. Allows `memory_search` and any remaining callers to keep their signatures during migration. Removal deferred to Spec 2+ once no callers remain. |
| `NoteFactRetrieval` | **Retained unchanged** — assembler is its consumer. |
| Scoring pipeline stages | **Retained unchanged** — run inside retrieval. |
| `memory_adapter.rs` | **Modified** — swaps `arbitrate` call for `assembler.assemble` + `render_envelope`. |
| `memory_search` tool | **Internally migrated** (optional in Spec 1) — may continue using retrieval directly; if migrated, shares the same assembler for consistency between explicit recall and implicit assembly. |

---

## 4. Data Model: `MemoryEnvelope` v1.0

All types live in `src/memory/assembler/envelope.rs`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryEnvelope {
    pub schema_version: String,          // "1.0"
    pub generated_at: i64,               // unix seconds
    pub query: String,                   // the triggering query (diagnostic)
    pub agent_id: String,
    pub session_id: Option<String>,
    pub slots: Vec<EnvelopeSlot>,        // ordered = injection order
    pub meta: EnvelopeMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeSlot {
    pub kind: SlotKind,
    pub items: Vec<EnvelopeItem>,        // ordered = priority within slot
    pub tokens_used: u32,
    pub tokens_budget: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotKind {
    UserProfile,
    SessionRecent,
    RelevantNotes,
    RawFragments,
    Nudges,                              // v1: budget=0 placeholder; Spec 2 fills
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeItem {
    pub id: String,                      // e.g., "note://wiki/rust-ownership"
    pub title: String,
    pub content: String,                 // already truncated to fit slot budget
    pub source: ItemSource,
    pub relevance: f32,                  // 0.0–1.0, assembler-assigned
    pub tokens: u32,                     // estimated
    pub updated_at: i64,                 // unix seconds
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemSource {
    Note    { path: String, category: String },
    Raw     { raw_id: String, session_id: String },
    Summary { layer: String, session_id: String },  // "d0" | "d1" | "d2"
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeMeta {
    pub strategy: String,                // "hybrid_v1" | "skeleton_fallback_v1"
    pub candidates_considered: usize,
    pub used_fallback: bool,
    pub fallback_reason: Option<String>, // "llm_timeout" | "llm_parse_error" | "empty_pool" | "forced"
    pub llm_rerank_latency_ms: Option<u64>,
    pub total_latency_ms: u64,
}
```

### 4.1 ID Conventions (stable across v1.x)

| Source | ID format |
|---|---|
| L1 Note | `note://{category}/{filename}` (no `.md`) |
| L0 Raw fragment | `aleph://session/{session_id}/raw/{raw_id}` |
| Session summary | `aleph://session/{session_id}/{layer}` (layer = `d0`/`d1`/`d2`) |
| User profile | `note://personal/profile` (convention — profile is a specific note) |

### 4.2 Version Policy

- **`schema_version = "1.0"`** from day 1.
- v1.x: **additive only** — new fields on existing types, new `SlotKind` / `ItemSource` variants, new `EnvelopeMeta` fields. Deserializers must accept unknown fields (serde default).
- v2.0+: breaking changes (removed fields, renamed fields, type changes). New major version.
- External consumers (Spec 3 MCP, exports) gate on major version.

### 4.3 `extra` Extension Point

Free-form JSON map on each `EnvelopeItem`. Spec 1 never reads or writes `extra` itself; it exists exclusively for:

- Spec 2 evolution: attach learned weights, citation counts, feedback tags.
- Spec 3 adapters: attach mem0/Honcho-specific metadata during export.
- User debugging: inject custom annotations via future tools.

Framework treats `extra` as opaque passthrough.

---

## 5. Assembly Flow Details

### 5.1 Stage 1: Candidate Gather

Concurrent fan-out via `tokio::join!`. Each source's failure is caught and degraded to empty:

| Source | Call (real types) | Default limit |
|---|---|---|
| Relevant notes | `Arc<NoteFactRetrieval<SqliteMemoryBackend>>::retrieve(query, agent_id, candidate_pool_limit)` | 20 |
| Session snapshot | `SnapshotReader::load_latest(session_id)` → `Option<SessionSnapshot>` | 1 (most recent) |
| Raw fragments | `MemoryBackend::get_raw_by_path_prefix(prefix, agent_id, limit)` via `Arc<SqliteMemoryBackend>` | 5 |
| User profile | `UserProfileLoader` (new, thin wrapper over `tokio::fs::read_to_string` of `memory/note/{agent_id}/personal/profile.md`) | 1 |

**UserProfile is always injected**, bypasses Stage 2 — persona is stable, LLM need not re-decide each turn. If the file does not exist, the slot is empty (no error surfaced).

**Note on `SessionSnapshot`:** Aleph does not have a distinct `SessionSummarySource` type. Session continuity data lives in `src/memory/session_resume/` as `SessionSnapshot { session_id, summary, key_decisions, active_files, pending_tasks }` read by `SnapshotReader`. The assembler uses the snapshot's `summary` + `key_decisions` fields as `SessionRecent` slot content.

**Note on raw memory access:** Raw fragments are accessed directly through `SqliteMemoryBackend` (no separate `RawMemoryStore` trait exists); the concrete backend is injected as `Arc<SqliteMemoryBackend>` via the same DI pattern used by `MemoryContextProvider`.

### 5.2 Stage 2: LLM Re-rank (B strategy)

**Model selection:** `AssemblerConfig.rerank_model` is the explicit override. Resolution order:

1. If `rerank_model` is `Some(name)`, use that model identifier with the default provider.
2. Else, use the configured primary conversation model, but issue a `tracing::warn!` once per process — the operator is advised to configure a cheaper fast model (Haiku / Qwen-fast / DeepSeek-v3 class) to avoid round-trip cost amplification.

This avoids introducing a new "fast model" config concept in Spec 1 (would touch `ProviderConfig` — out of scope); operators opt in by setting `rerank_model` explicitly.

**Prompt shape** (in `rerank.rs`, versioned as `RERANK_PROMPT_V1`):

```text
You are a Working Memory Assembler. Given the user's current query and a pool
of memory candidates, decide which to include and allocate a token budget
across slots: session_recent, relevant_notes, raw_fragments.
(UserProfile is pre-included; nudges are reserved for future use.)

Query: {query}

Total budget: {budget_tokens} tokens.
Reserve at least 30% headroom for the LLM's reply; your slot sum must be
<= {budget_tokens * 0.7}.

Candidates (id | title | relevance | summary):
  [note://wiki/rust-ownership]   | "Rust ownership rules" | 0.82 | <=30-char abstract
  [aleph://session/abc/raw/xyz]  | "<raw fragment>"       | 0.55 | <=30-char abstract
  ...

Return strict JSON matching this schema:
{
  "slots": [
    {"kind": "relevant_notes",  "item_ids": [...], "tokens_budget": N},
    {"kind": "session_recent",  "item_ids": [...], "tokens_budget": N},
    {"kind": "raw_fragments",   "item_ids": [...], "tokens_budget": N}
  ],
  "reasoning": "one-line explanation (optional)"
}

If you would not include a slot, omit it from the array.
Only use item_ids from the candidate list. Order within item_ids is priority
(most important first).
```

**Post-validation:**
- `item_ids` must be a subset of candidate IDs — unknown IDs dropped silently.
- Sum of `tokens_budget` must be ≤ `total_budget * 0.7`; if exceeded, scale proportionally.
- Empty slot arrays allowed; absent `slots` array → fallback.
- `SlotKind` must be a known variant; unknown → drop slot.

**Timeout:** 800ms hard wrap via `tokio::time::timeout`. Timeout → fallback.

### 5.3 Stage 2' (C): Skeleton Fallback

Triggered when:
- Stage 2 timeout
- Stage 2 returns invalid JSON
- Stage 2 returns zero valid slots
- Candidate pool total < 3 items
- `AssemblerConfig.force_fallback = true`

**Default skeleton** (all configurable):

| Slot | Default tokens |
|---|---|
| `UserProfile` | 200 |
| `SessionRecent` | 1500 |
| `RelevantNotes` | 5000 |
| `RawFragments` | 1000 |
| `Nudges` | 0 |

**Packing strategy:** For each slot, sort candidates assigned to that slot descending by `relevance * recency_factor`, greedy-fill until slot budget is exhausted. No cross-slot budget borrowing.

**`recency_factor`** within fallback (independent from retrieval scoring pipeline's stages):
```text
recency_factor = 0.5 + 0.5 * exp(-age_days / 14.0)   // [0.5, 1.0]
```

### 5.4 Stage 3: Hydration + Token Pack

```rust
for slot in &mut envelope.slots {
    let mut used = 0u32;
    for item in &mut slot.items {
        let full_content = loader.load(&item.id).await.unwrap_or_default();
        let truncated = truncate_utf8_safe(&full_content, (slot.tokens_budget - used) * 4);
        item.tokens = estimate_tokens(&truncated);
        item.content = truncated;
        used += item.tokens;
        if used >= slot.tokens_budget { break; }
    }
    slot.tokens_used = used;
}
```

- **`truncate_utf8_safe`** — uses `char_indices()` to avoid splitting code points (P7 defensive design).
- **`estimate_tokens`** — `(content.chars().count() / 4).max(1)` for v1 (matches existing `ContextComptroller` estimate). Pluggable real tokenizer in v1.1.
- **Content loader** — per-call `HashMap<String, String>` dedup cache; no cross-call cache (Dream Daemon updates must be immediately visible).

---

## 6. Integration

### 6.1 Call Site Change

The real pre-prompt memory fetch lives in `src/thinker/memory_context_provider.rs::MemoryContextProvider::fetch()`. That function currently calls `NoteFactRetrieval::vector_retrieve` directly, builds a legacy `MemoryContext { facts, memory_summaries, structured_index }`, and truncates by char budget. The assembler replaces the internals of that function without changing its external signature:

```rust
// Before (conceptual, in memory_context_provider.rs):
let facts = self.note_retrieval.vector_retrieve(query, agent_id, max_facts).await?;
let mut ctx = MemoryContext { facts, memory_summaries: vec![], structured_index: None };
self.truncate_to_budget(&mut ctx);
ctx

// After:
let envelope = self.assembler.assemble(
    query, agent_id, session_id,
    AssemblyBudget { total_tokens: self.config.max_output_chars as u32 / 4 },
).await?;
// Keep MemoryContext as the wire format to PromptLayer; populate it from the envelope:
let ctx = memory_context_from_envelope(&envelope);
// Alternatively (preferred once PromptLayer is ready): PromptLayer consumes
// render_envelope(&envelope) directly and MemoryContext bridging is removed.
ctx
```

**Note on `ContextComptroller`.** The only remaining in-tree caller of `ContextComptroller` is `src/builtin_tools/memory_search.rs` (the explicit LLM-facing recall tool). It is **not** on the auto-inject path. Spec 1 leaves `memory_search.rs` unchanged. Migrating `memory_search` to also use the assembler is an optional follow-up in Spec 2 (so implicit and explicit recall share a single policy surface) and is explicitly not required for Spec 1 DoD.

### 6.2 Dependency Injection

Aleph has no centralized `AppContext` container — it uses Arc-based DI, constructing and threading `Arc<T>` / `Arc<dyn Trait>` down from server startup (following the pattern used by `MemoryContextProvider`). The assembler follows the same convention:

```rust
// At server startup (or wherever MemoryContextProvider is currently built):
let assembler: Arc<dyn WorkingMemoryAssembler> = Arc::new(HybridAssembler::new(
    note_retrieval.clone(),     // Arc<NoteFactRetrieval<SqliteMemoryBackend>>
    snapshot_reader.clone(),    // Arc<SnapshotReader>
    memory_backend.clone(),     // Arc<SqliteMemoryBackend>
    profile_loader.clone(),     // Arc<UserProfileLoader>
    ai_provider.clone(),        // Arc<dyn AiProvider>
    assembler_config.clone(),
));

// MemoryContextProvider now holds Arc<dyn WorkingMemoryAssembler> instead of
// Arc<NoteFactRetrieval<...>>.
let provider = MemoryContextProvider::new_with_assembler(assembler, config);
```

Collaborators are held as `Arc<T>` (concrete) or `Arc<dyn Trait>` (where a trait exists); `HybridAssembler::new` takes them all — supports mockall in tests for the trait-typed collaborators (`dyn AiProvider` especially). For concrete types without traits, tests use a thin in-memory stub in the test module.

### 6.3 Renderer

```rust
pub fn render_envelope(env: &MemoryEnvelope) -> String {
    render_with(env, RenderStyle::MarkdownV1)
}

pub fn render_with(env: &MemoryEnvelope, style: RenderStyle) -> String { ... }

pub enum RenderStyle { MarkdownV1, Xml, Json }
```

**Default `MarkdownV1` output:**

```markdown
<memory>

<user_profile>
{profile.md content}
</user_profile>

<session_recent>
## [d1 @ 2026-04-12]
{summary body}

## [d1 @ 2026-04-11]
{summary body}
</session_recent>

<relevant_notes>
## [note://wiki/rust-ownership] (updated 2026-04-01)
{note body, truncated}

---

## [note://plan/march-goals] (updated 2026-03-15)
{note body, truncated}
</relevant_notes>

<raw_fragments>
## [raw @ session abc123, t=2026-04-13T02:14Z]
{raw content}
</raw_fragments>

</memory>
```

**Empty slots** are fully omitted — no opening tag, no closing tag. Fully empty envelope renders as empty string.

**Pure function** — no I/O, no async, deterministic given the same envelope. Spec 3 MCP server reuses this function for `get_envelope_text`.

---

## 7. Configuration

New in `src/config/types/memory.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssemblerConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,                          // default: true
    #[serde(default = "default_total_budget")]
    pub total_budget_tokens: u32,               // default: 8000
    #[serde(default = "default_pool_limit")]
    pub candidate_pool_limit: usize,            // default: 20
    #[serde(default = "default_rerank_timeout")]
    pub rerank_timeout_ms: u64,                 // default: 800
    #[serde(default)]
    pub rerank_model: Option<String>,           // None → fast provider model
    #[serde(default)]
    pub render_style: RenderStyle,              // default: MarkdownV1
    #[serde(default)]
    pub force_fallback: bool,                   // default: false (test/offline)
    #[serde(default)]
    pub fallback_skeleton: FallbackSkeleton,
    #[serde(default)]
    pub assembly_log: AssemblyLogConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FallbackSkeleton {
    pub user_profile_tokens: u32,   // default 200
    pub session_recent_tokens: u32, // default 1500
    pub relevant_notes_tokens: u32, // default 5000
    pub raw_fragments_tokens: u32,  // default 1000
    pub nudges_tokens: u32,         // default 0
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssemblyLogConfig {
    pub enabled: bool,              // default: false
    pub retention_days: u32,        // default: 14
}
```

TOML example:

```toml
[memory.assembler]
enabled = true
total_budget_tokens = 8000
rerank_timeout_ms = 800
rerank_model = "claude-haiku-4-5-20251001"

[memory.assembler.fallback_skeleton]
relevant_notes_tokens = 6000

[memory.assembler.assembly_log]
enabled = false
retention_days = 14
```

When `enabled = false`, `memory_adapter` bypasses assembler entirely and falls back to the legacy `ContextComptroller::arbitrate` path — full kill-switch.

---

## 8. Error Model

```rust
#[derive(Debug, thiserror::Error)]
pub enum AssemblerError {
    #[error("retrieval failed: {0}")]
    Retrieval(#[source] AlephError),

    #[error("llm rerank timeout after {0}ms")]
    RerankTimeout(u64),

    #[error("llm rerank returned invalid json: {0}")]
    RerankParse(String),

    #[error("content load failed for {id}: {source}")]
    Hydration { id: String, #[source] source: AlephError },
}
```

**Visibility:** `AssemblerError` is **internal to `src/memory/assembler/`**. Never surfaces to callers. All variants are caught at the `HybridAssembler::assemble` boundary and mapped:

| Internal error | External behavior |
|---|---|
| `Retrieval` | That source contributes 0 candidates; other sources proceed; `tracing::warn`. |
| `RerankTimeout` | Go to Stage 2'; `meta.fallback_reason = "llm_timeout"`. |
| `RerankParse` | Go to Stage 2'; `meta.fallback_reason = "llm_parse_error"`. |
| `Hydration` | Drop that item from slot; `tracing::warn`. |

Callers see `Result<MemoryEnvelope, AlephError>` where `Err` only surfaces for system-level failure (e.g., `AppContext` missing required dependency at construction — which should never happen at runtime).

---

## 9. Observability

### 9.1 Structured Tracing (always on)

Per-assembly event:

```rust
tracing::info!(
    target = "memory.assembler",
    query_hash = %sha256_hex(&query),
    agent_id = %agent_id,
    session_id = ?session_id,
    strategy = %envelope.meta.strategy,
    used_fallback = envelope.meta.used_fallback,
    fallback_reason = ?envelope.meta.fallback_reason,
    candidates = envelope.meta.candidates_considered,
    llm_rerank_ms = ?envelope.meta.llm_rerank_latency_ms,
    total_ms = envelope.meta.total_latency_ms,
    slot_count = envelope.slots.len(),
    total_tokens = envelope.slots.iter().map(|s| s.tokens_used).sum::<u32>(),
    "assembly completed"
);
```

- **`query_hash`** not raw query — privacy + log size.
- **Per-slot detail** logged at `debug` level to avoid info-level bloat.

### 9.2 Optional Persistent Log

When `AssemblyLogConfig.enabled = true`, write to new SQLite table:

```sql
CREATE TABLE IF NOT EXISTS assembly_logs (
    id                 TEXT PRIMARY KEY,
    agent_id           TEXT NOT NULL,
    session_id         TEXT,
    query_hash         TEXT NOT NULL,
    strategy           TEXT NOT NULL,
    used_fallback      INTEGER NOT NULL DEFAULT 0,
    fallback_reason    TEXT,
    candidates_count   INTEGER NOT NULL,
    selected_item_ids  TEXT NOT NULL,          -- JSON array of EnvelopeItem.id
    total_tokens       INTEGER NOT NULL,
    rerank_latency_ms  INTEGER,
    total_latency_ms   INTEGER NOT NULL,
    created_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_assembly_logs_agent_created
    ON assembly_logs(agent_id, created_at);
```

Schema defined in Spec 1, default disabled. Spec 2 reads this table to correlate with citation/re-retrieval signals.

Retention: a periodic cleanup task (reusing Dream Daemon's idle window) deletes rows older than `retention_days`. This cleanup is the only write Spec 2 may need to add; Spec 1 does not implement the cleanup (table grows unbounded with default `enabled = false` — zero rows).

---

## 10. Forward-Compatibility Contracts

These are stability promises Spec 1 makes for Spec 2 and Spec 3.

### 10.1 Schema Stability (to Spec 3)

- `MemoryEnvelope` and all nested types serialize via serde with `schema_version = "1.0"`.
- v1.x changes are **additive only** — new fields, new enum variants; existing deserializers continue to work with `#[serde(default)]` on new fields and `#[serde(other)]`-tolerant unknown variants where appropriate.
- The `serde_json::to_string(&envelope)` → `serde_json::from_str(&json)` round-trip is a tested invariant (integration test).
- Consumers (MCP server, CLI export) gate on major version only.

### 10.2 Trait Stability (to Spec 2)

- `WorkingMemoryAssembler` trait signature is frozen for v1.x.
- Spec 2's `EvolvingAssembler` wraps `HybridAssembler` as a decorator, not a replacement. The decorator pattern is explicitly supported: `Arc<dyn WorkingMemoryAssembler>` can be layered.
- `EnvelopeItem.extra` map is reserved as the evolution annotation channel — Spec 1 never reads or writes it.

### 10.3 ID Stability (to Spec 2 feedback loop + Spec 3 export)

- `EnvelopeItem.id` format is documented (§4.1) and will not change within v1.x.
- Stable IDs enable citation signals (Spec 2) and cross-agent transfer (Spec 3) without re-keying.

### 10.4 Log Schema Stability (to Spec 2)

- `assembly_logs` columns are the anchor for Spec 2's `citation_signals` / `rerefetch_signals` tables via `assembly_logs.id` FK.
- Columns are additive in v1.x.

---

## 11. Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| **LLM re-rank cost** — extra call per turn | Cumulative LLM spend | Default fast model (Haiku-class); `enabled = false` master kill; config per-session-class; Spec 2 will learn when to skip |
| **Latency regression** — perceptible in Halo popup / quick interactions | Worsened UX for short-turn use cases | 800ms hard timeout; P99 latency in canary metrics; async-platform cases (Telegram) are insensitive; legacy path preserved as fallback |
| **Candidate pool limit=20 under-recall** | Important note not visible to LLM rerank | Respects existing hybrid retrieval order (best first); limit is configurable; Spec 2 tunes per category |
| **Fallback triggered too often** | Effectively degrades to current `ContextComptroller` | Metrics emit fallback rate; > 20% triggers alert; reason codes let us diagnose (timeout vs parse vs empty pool) |
| **Prompt injection via note content** | Malicious note text manipulates LLM | `<memory>` wrapper tags + system-prompt declaration that memory is not instruction; not a new risk introduced here but centralized to a single boundary |
| **Schema v1.0 commitment too early** | Future v1.1 migration pain | `schema_version` on every envelope; strict additive-only rule; version gate in consumers |
| **Overlap with `memory_search` tool** | Users/LLMs confused about two paths | `memory_search` = explicit (LLM asks); assembler = implicit (framework auto-inject); document the split; share underlying retrieval |
| **`assembly_logs` table built but unused** | YAGNI violation | Default disabled, zero rows at rest; schema cost is ~30 LoC migration; avoiding a later migration when Spec 2 lands |

---

## 12. Testing Strategy

### 12.1 Unit Tests

| Module | Coverage |
|---|---|
| `envelope.rs` | serde round-trip for all `SlotKind` / `ItemSource` variants; unknown-field tolerance; `extra` map passthrough |
| `render.rs` | MarkdownV1 snapshot for empty / full / partial envelopes; XML and Json styles; empty slot omission |
| `hydration.rs` | `truncate_utf8_safe` proptest — any input + any n → valid UTF-8 and `.len() <= n`; `estimate_tokens` deterministic |
| `fallback.rs` | Skeleton packer with candidate pools of sizes 0, 1, 3, 20, 100; budget never exceeded; recency_factor bounded [0.5, 1.0] |
| `rerank.rs` | Response validation: valid JSON / malformed JSON / hallucinated IDs / over-budget scaling / unknown `SlotKind` variants |

### 12.2 Integration Tests (`src/memory/assembler/tests/`)

Mock `NoteFactRetrieval`, `AiProvider`, `RawMemoryStore`, `SessionSummarySource`, `UserProfileLoader` via mockall. Five core paths:

1. **Happy B-path** — LLM returns valid JSON → envelope matches expected slots, `used_fallback = false`.
2. **LLM timeout** — fake 2s sleep → falls back → `used_fallback = true`, `fallback_reason = "llm_timeout"`.
3. **LLM hallucinated IDs** — response contains unknown IDs → filtered; envelope valid; real IDs preserved.
4. **Tiny pool** — only 2 candidates → direct fallback, no LLM call made.
5. **Total retrieval failure** — all sources error → envelope with empty slots, `Ok(envelope)` returned (no `Err`).

### 12.3 Property Test

```rust
#[proptest]
fn envelope_total_tokens_never_exceed_budget(
    candidates in arb_candidate_pool(),
    budget in 100u32..16000u32,
) {
    let envelope = assembler.assemble_sync_for_test(candidates, budget);
    let total: u32 = envelope.slots.iter().map(|s| s.tokens_used).sum();
    prop_assert!(total <= budget);
}
```

### 12.4 Observation Tests

Using `tracing-test`: assert that every successful `assemble` emits exactly one `"assembly completed"` event with required fields. Spec 2 depends on this contract.

### 12.5 E2E Smoke

Add one test to existing agent-loop E2E suite asserting that a conversation turn's final prompt contains a `<memory>` block when notes exist for the agent, and contains no `<memory>` (or empty) when the note store is empty.

---

## 13. Definition of Done

Spec 1 is complete when all are true:

1. **Functional:** all unit, integration, property tests pass.
2. **Performance:** assembly P50 ≤ 600ms, P99 ≤ 1200ms on canary workload (current retrieval is ~200ms baseline; rerank adds ~400ms budget).
3. **Resilience:** with `AiProvider` stubbed to always fail, all tests still pass; `assemble` never returns `Err`.
4. **Observability:** `"assembly completed"` tracing event emitted on every call; with `assembly_log.enabled = true`, rows land correctly in `assembly_logs`.
5. **Backward compat:** existing `memory_search`, `recall_context`, and E2E conversation tests show no regression.
6. **Round-trip:** `serde_json::to_string(&envelope)` + `serde_json::from_str` preserves content equality (tested).
7. **Zero stubs:** no `todo!()` / `unimplemented!()` / placeholder values in the shipped code.
8. **Configuration:** `memory.assembler.enabled = false` is a full kill-switch; legacy path verified still working.

---

## 14. Open Questions / Decisions Deferred

The following were considered and intentionally **left for future specs / PRs**:

- Whether `SessionRecent` and `RawFragments` should merge into a single `SessionHistory` slot. Decision: keep separate for clarity; adapter layer (Spec 3) can merge on export if needed.
- Real tokenizer integration (tiktoken / bpe). Deferred to v1.1 — `estimate_tokens` uses `chars/4` for consistency with current estimator.
- `ContextComptroller` full deletion. Deferred to Spec 2 once no callers reference it.
- Per-agent envelope overrides (an agent wants a different skeleton). Deferred — configurable globally for now.
- Envelope persistence beyond tracing (full snapshots for replay). Deferred; Spec 2 will evaluate.

---

## 15. References

- `docs/reference/memory/RETRIEVAL.md` — current hybrid retrieval, consumer interface.
- `docs/reference/memory/NOTES.md` — L1 note model, stable path IDs.
- `docs/reference/memory/RAW_MEMORY.md` — L0 raw memories and session summaries.
- `docs/reference/memory/DREAM_DAEMON.md` — offline stage pattern that Spec 2 extends.
- `src/memory/context_comptroller/comptroller.rs` — the module this spec replaces behaviorally.
- `src/agent_loop/adapters/memory_adapter.rs` — the integration point.
- `CLAUDE.md` R8 (LLM Sovereignty), R9 (Everything is a Tool), R10 (Intelligence in the Prompt), P1–P8 design principles — the north star this spec obeys.

---

## 16. Roadmap Context

This is **Spec 1 of 4** in the memory evolution roadmap:

- **Spec 1 (this doc):** `MemoryEnvelope` contract + `WorkingMemoryAssembler`. Runtime foundation.
- **Spec 2:** Evolution loop. Signal capture (S1 citation via Dream Daemon replay, S3 re-retrieval telemetry, S4 explicit feedback, S5 existing recall_signals). β query-category + γ per-note weights. `AssemblyFeedbackStage` in Dream Daemon. Consumes `assembly_logs` built here.
- **Spec 3:** MCP Memory Server. Expose assembler as MCP tools (`assemble_envelope`, `cite`, `feedback`). CLI `aleph memory export/import`. P1 cross-agent migration. Reuses `render_envelope` and the schema defined here.
- **Spec 4 (on demand):** Adapters — `Envelope → mem0.Memory[]`, `Envelope → Honcho representation`, etc.

Each sub-spec is independently valuable and shippable.
