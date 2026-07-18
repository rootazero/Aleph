---
title: "Memory Evolution Spec 2: Reflect / Synthesis"
date: 2026-04-13
status: approved
parent: docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md
related_refs:
  - docs/reference/memory/NOTES.md
  - docs/reference/memory/RETRIEVAL.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md
---

# Spec 2: MemoryReflector

Add a synthesis layer on top of `HybridAssembler` that turns "top-K hit list" retrieval into a coherent LLM-synthesised answer with cited sources. Expose both as a new builtin tool (`memory_reflect`) and as a core `MemoryReflector` API for internal callers.

---

## 1. Problem

Every memory tool in Aleph today (`memory_search`, `memory_explore`, `memory_browse`, `memory_timeline`, `recall_context`) returns **hits** — the LLM then has to piece a coherent answer out of a pile of fragments. This wastes turn budget and pushes the "distillation" work onto every calling agent.

Hermes-agent's `reflect` operation solves this by synthesising an answer across all memories. Aleph already has a richer retrieval stack (multi-layer hybrid + assembler + rerank) than hermes — so Aleph's reflect can deliver **better** synthesis:

- pre-ranked context instead of flat vector hits
- per-note metadata (category, tags, wikilinks) for richer prompt framing
- event-sourcing + namespace scope retained through the pipeline

---

## 2. Non-goals

- Not iterative-expansion retrieval (call → expand wikilinks → re-call). YAGNI; Spec 4 or later.
- Not a reflect cache. YAGNI.
- Not a `confidence` / `contradictions` structured-field output — the LLM expresses these in natural language inside `text`. R8 sovereignty: don't force it to fill a score it can't really compute.
- Not cross-agent reflection. Namespace boundaries from Spec 1 preserved.
- Not auto-wiring `session_complete` → reflect. Spec 1 left that gap; a later spec can connect them once this core API exists.

---

## 3. Architecture

### 3.1 Data flow

```
memory_reflect tool (LLM-facing)               MemoryReflector (core API)
────────────────────────────                   ─────────────────────────────

{ query: string }                              reflect(query, opts)
        │                                              │
        ▼                                              ▼
┌──────────────────────────────┐            ┌──────────────────────────────┐
│ handler:                     │────────────►│  1. assembler.assemble(q)   │
│   build ReflectOpts from ctx │            │     → packet { notes, ... } │
│   call reflector.reflect()   │            │                              │
│   serialise Synthesis → JSON │            │  2. if packet empty:         │
└──────────────────────────────┘            │       return stub Synthesis  │
        ▲                                   │                              │
        │                                   │  3. build synthesis prompt   │
        │                                   │     + cite-hint packet text  │
        │ Synthesis { text, sources }       │                              │
        │                                   │  4. provider.process(SYSTEM  │
        │                                   │     = PROMPT_SYNTHESIS,      │
        │                                   │     user = query+packet)    │
        │                                   │                              │
        │                                   │  5. parse JSON response     │
        │                                   │     overlay NoteRef titles  │
        │                                   │     from packet             │
        │                                   │                              │
        │                                   │  6. record recall_signals    │
        │                                   │     (channel = "reflect")   │
        │                                   │                              │
        └───────────────────────────────────│  7. return Synthesis         │
                                            └──────────────────────────────┘
```

### 3.2 Key invariants

- **Reuse, do not reimplement retrieval.** `HybridAssembler::assemble` is the single retrieval entry. Reflect is a consumer, not a sibling.
- **Empty packet → zero LLM tokens.** Short-circuit with a stable `Synthesis { text: "No relevant memories found.", sources: [] }`.
- **Source titles are code-authoritative.** LLM outputs `path` + `relevance`; code joins `title` from the assembler packet so a bad LLM can't invent fake titles.
- **Recall signals fire for every note fed into the prompt**, not only those cited in the LLM output. The consumer view is: "these notes were used to form the answer, whether or not the LLM mentioned them" — consistent with `memory_search` semantics.

---

## 4. Types

`src/memory/reflector/mod.rs`:

```rust
pub struct MemoryReflector {
    assembler: Arc<HybridAssembler>,
    provider: Arc<dyn AiProvider>,
    recall_store: Arc<dyn RecallSignalStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Synthesis {
    pub text: String,
    pub sources: Vec<NoteRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteRef {
    pub path: String,     // "wiki/rust-ownership"
    pub title: String,    // human-readable; filled by code, not LLM
    pub relevance: f32,   // assembler rerank score
}

#[derive(Debug, Clone)]
pub struct ReflectOpts {
    pub agent_id: String,
    pub namespace: crate::memory::namespace::NamespaceScope,
    pub max_tokens: Option<usize>,                // assembler budget
    pub time_range: Option<(i64, i64)>,           // future internal callers
}

impl MemoryReflector {
    pub fn new(
        assembler: Arc<HybridAssembler>,
        provider: Arc<dyn AiProvider>,
        recall_store: Arc<dyn RecallSignalStore>,
    ) -> Self { ... }

    pub async fn reflect(
        &self,
        query: &str,
        opts: ReflectOpts,
    ) -> Result<Synthesis, AlephError> { ... }
}
```

Concrete trait dependencies (`HybridAssembler`, `RecallSignalStore`, `AiProvider`) already exist; Spec 2 introduces no new traits.

---

## 5. Synthesis prompt

`src/memory/reflector/prompts.rs::PROMPT_SYNTHESIS` (text stored as `include_str!("snapshots/synthesis.txt")` for regression-testable review):

```
You are a memory synthesis assistant. Below are notes retrieved from the
user's long-term memory, ranked by relevance. Compose a coherent answer
to the user's question using ONLY information in those notes.

RULES:
1. Only use information from the provided notes. Do not invent facts.
2. If the notes don't cover the question, say so in plain language —
   do NOT make up an answer. An honest "my memory has no information on
   this" is valuable.
3. Express uncertainty, contradictions, or caveats in natural language
   inside `text`. Do NOT split them into separate structured fields.
4. Every note you cite must appear in `sources` with its exact `path`
   and a `relevance` score copied from the `[score=...]` tag in the
   input. Do not hallucinate paths or scores.

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "text": "Synthesised answer as continuous prose. Can be multiple sentences.",
  "sources": [
    { "path": "exact/path/from/input", "relevance": 0.81 }
  ]
}
```

The code layer fills `NoteRef::title` by joining `path` against the assembler packet (the LLM is asked only for path + relevance; titles cannot be fabricated).

---

## 6. `memory_reflect` tool

`src/builtin_tools/memory_reflect.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryReflectArgs {
    /// Natural-language question to synthesise an answer for.
    pub query: String,
}

pub const TOOL_NAME: &str = "memory_reflect";

pub const TOOL_DESCRIPTION: &str = "Synthesise a coherent answer from your \
long-term memory. Use this when you want a distilled response (vs \
memory_search, which returns raw hits). Returns answer text + cited note paths.";
```

Handler reads the current agent_id + namespace from the tool context, builds `ReflectOpts`, calls `reflector.reflect(query, opts)`, and serialises the resulting `Synthesis` to JSON for the LLM.

---

## 7. Side effects

Each `reflect()` call writes **one `recall_signals` row per note fed into the synthesis prompt**, using the existing `RecallSignalStore`:

- `note_path` — the note's path
- `query_hash` — first 8 hex of SHA-256(query)
- `query_text` — the raw query (truncated to some existing max)
- `channel` — `"reflect"` (new literal value; the column is free-form `TEXT`, no enum changes needed)
- `score` — assembler rerank score
- `session_id` / `namespace` — from opts

The dream daemon's existing decay / drift logic observes these signals and treats reflect-touched notes as active memory (same as notes accessed via `memory_search`).

---

## 8. Empty / weak retrieval

If `HybridAssembler::assemble(query)` returns an empty packet (zero hydrated notes, or budget collapsed to zero envelopes), the reflector **returns without calling the LLM**:

```rust
Synthesis {
    text: "No relevant memories found.".to_string(),
    sources: vec![],
}
```

No recall signals are written. This is intentional — absence of memories isn't a "recall event".

---

## 9. Wiring at startup

The server builder (same place Spec 1 wired the Spec 1 capture hooks — `src/bin/aleph-server/commands/start/builder/`) constructs a shared `Arc<MemoryReflector>` and:

1. Passes it to the tool-context assembly so `memory_reflect` handler can reach it.
2. Registers `memory_reflect` in `src/executor/builtin_registry/registry.rs` alongside the other memory tools.

Already-existing `Arc<HybridAssembler>`, `Arc<dyn AiProvider>` (for fact extraction), and `Arc<dyn RecallSignalStore>` are reused — no new dependencies.

---

## 10. Testing strategy

- **Unit**: `MemoryReflector::reflect` with mocked `HybridAssembler` (returns fixed packet) + `RecordingMockProvider` (returns canned JSON) + `FakeRecallSignalStore`. Assert:
  - Empty packet → zero-LLM short-circuit, no recall signals.
  - Non-empty packet → LLM call happened with `PROMPT_SYNTHESIS`.
  - `NoteRef.title` is code-overlaid (test LLM returning wrong title is ignored).
  - One recall signal per note in the packet.
- **Prompt snapshot**: `prompts/snapshots/synthesis.txt` regression test for `len > 200` and `"JSON"` containment.
- **Integration**: `tests/memory_reflect_integration.rs` — full pipeline against in-memory SQLite, fixture notes, real (mocked-provider) assembler → reflector. Verify end-to-end Synthesis + recall_signals row count.

---

## 11. Compliance with architectural redlines

| Redline | Check |
|---------|-------|
| R3 Core minimalism | 1 new module + 1 tool + 1 prompt file. No new deps. |
| R8 LLM sovereignty | Synthesis / source-selection / uncertainty expression all LLM-side. |
| R9 Everything is a tool | `memory_reflect` exposed as tool. |
| R10 Intelligence in the prompt | `PROMPT_SYNTHESIS` is the sole driver of synthesis quality. |

No redline violated.

---

## 12. Open questions (resolve in plan phase)

- Exact `query_text` truncation length in `recall_signals` — check existing schema default.
- How to represent `[score=...]` tags inside the assembler packet text for the LLM — may already be present via `render.rs`, else add a light overlay.
- Whether the tool context has direct access to `Arc<MemoryReflector>` or needs a new context field (follow Task 10 pattern from Spec 1).
