---
title: Spec B — Session Search Summarization Pipeline
date: 2026-05-01
status: draft
owner: @user
related_refs:
  - docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md
  - docs/superpowers/specs/2026-05-01-memory-evolution-spec-a-curated-hot-snapshot-design.md
  - docs/reference/memory/NOTES.md
  - docs/reference/memory/RETRIEVAL.md
  - src/builtin_tools/session_search.rs
  - src/memory/session_compactor/
  - src/memory/assembler/
---

# Spec B — Session Search Summarization Pipeline

> Hermes-inspired follow-up to the 4-spec memory evolution roadmap. Spec A
> (curated hot memory + frozen snapshot + `remember` tool) shipped at
> `a035e76a7` on 2026-05-01. Spec B closes the next gap: cross-session
> retrieval today returns raw FTS5 message fragments rather than digestible
> synthesized excerpts. This spec wires the existing `session_compactor`
> outputs and a new session-end fallback into the `session_search` tool path
> so agents get summary + evidence quotes per session, not raw turn fragments.

---

## 1. Motivation

### 1.1 Current state

`session_search` (a built-in tool exposed to LLMs) performs FTS5 full-text
search over the raw transcript table (`messages`). For each match it returns
`{ session_key, agent_id, topic, role, content, timestamp }` — where
`content` is a single raw message body. The tool is A2A-policy filtered and
over-fetches by 4× to compensate for ACL drops.

`session_compactor` (intra-session context-window pressure) already produces
hierarchical summaries during long sessions: `L2Detail` (d0) / `L1Overview`
(d1) / `L0Abstract` (d2+) facts written to `memory_facts` with
`fact_source = SessionCompressed` at path
`aleph://session/{sid}/d{depth}/{seq}`.

These two systems are **disconnected**. Cross-session retrieval ignores
the summaries the compactor has already produced, and short sessions (that
never trigger compaction) have no summaries at all.

### 1.2 User pain (acknowledged in Q1)

- **Quality**: agents get 200-char raw mid-turn snippets without surrounding
  context. They have to reason from fragments instead of from a coherent
  summary of what the past session was about.
- **Cost / latency**: when the LLM does want to understand a past session,
  it ends up doing follow-up tool calls or extra reasoning to digest raw
  hits. A summary at retrieval time saves that work.

### 1.3 Hard constraint

**Quality > cost.** When the two conflict, prefer the higher-quality path
(extra LLM call, larger response) over the cheaper path.

---

## 2. Architecture

### 2.1 Diagram

```
   ┌─────────────────────────┐
   │  session_search Tool    │
   │  (builtin_tools)        │
   └──────────┬──────────────┘
              │
              │ ① summary primary search (fact_source=SessionCompressed)
              ▼
   ┌─────────────────────────┐         ┌──────────────────────────┐
   │ HybridAssembler         │ ◄────── │ memory_facts             │
   │ (FTS5+vec+RRF+rerank)   │         │ (compactor d0/d1/d2 +    │
   └──────────┬──────────────┘         │  /end-summary)           │
              │                        └──────────────────────────┘
              │ ② per-session evidence_quotes lookup
              ▼
   ┌─────────────────────────┐         ┌──────────────────────────┐
   │ session_store           │ ◄────── │ messages FTS5 table      │
   │ .search_messages()      │         │ (raw transcripts)        │
   └──────────┬──────────────┘         └──────────────────────────┘
              │
              │ ③ matched session has no summary fact?
              ▼            → SummarySynthesizer.lazy_for() (1 LLM call)
   ┌─────────────────────────┐
   │ SummarySynthesizer      │ — reuses compactor build_summary_prompt
   │ (new module, ~150 LOC)  │
   └─────────────────────────┘

   Independent path: on_session_end hook (Spec 1, already shipped)
   ──────────────────────────────────────────────────────────────
   session truly ends → SessionEndSummarizer.produce() → write fact:
     path  = aleph://session/{sid}/end-summary
     layer = L1Overview
     fact_source = SessionCompressed
```

### 2.2 Physical boundaries

- **No new tables**, no new FTS5 indices.
- **No replacement** of `session_compactor` internals (it is purely a
  producer that Spec B reads from).
- New code lives in:
  - `src/memory/session_search_summary/` — new module (synthesizer, end-hook
    glue, per-session dedup logic)
  - `src/builtin_tools/session_search.rs` — retrieval path rewrite
  - `src/memory/assembler/` — additive `FactSourceFilter` parameter
- Reuses Spec 1's `on_session_end` hook (already shipped).
- HybridAssembler gains an additive filter parameter; default behaviour
  for all existing callers (note retrieval, etc.) is unchanged.

### 2.3 Information-flow budget per tool call

- 99 % of calls: 2 storage queries (HybridAssembler + `search_messages`),
  0 LLM calls.
- 1 % short-session lazy path: 2 storage queries + 1 LLM call.
- Once-per-session-end: 1 LLM call (skipped when compactor d* facts already
  exist for the session).
- A summary fact, once written, is never updated (path uniqueness).

---

## 3. Data Model & Schema

### 3.1 Reuse existing `memory_facts` table

| Producer | Path template | Layer | fact_source | Owner |
|---|---|---|---|---|
| compactor leaf | `aleph://session/{sid}/d0/{seq}` | `L2Detail` | `SessionCompressed` | shipped |
| compactor d1 | `aleph://session/{sid}/d1/{seq}` | `L1Overview` | `SessionCompressed` | shipped |
| compactor d2+ | `aleph://session/{sid}/d2/{seq}` | `L0Abstract` | `SessionCompressed` | shipped |
| session-end fallback | `aleph://session/{sid}/end-summary` | `L1Overview` | `SessionCompressed` | **Spec B** |
| lazy on-read fallback | `aleph://session/{sid}/end-summary` | `L1Overview` | `SessionCompressed` | **Spec B** |

Notes:

- No new `FactSource` enum variant — `SessionCompressed` already covers
  "any session-derived summary".
- session_end and lazy paths share the same fact path (`/end-summary`).
  `INSERT OR IGNORE` semantics make the **first writer win** permanently.
  Both paths use the same `build_summary_prompt(0)` template, so the two
  outputs are equivalent in quality — there is no "session_end overwrites
  lazy" upgrade. session_end is a backstop that fires only when lazy has
  not already covered the session; lazy is a backstop for in-flight
  sessions where session_end has not yet fired. Whichever wins is fine.

### 3.2 Tool schema (breaking change, single consumer)

Old:

```rust
pub struct SessionSearchHit {
    pub session_key: String,
    pub agent_id:    String,
    pub topic:       Option<String>,
    pub role:        String,        // REMOVED
    pub content:     String,        // REMOVED → split below
    pub timestamp:   i64,
}
```

New:

```rust
pub struct SessionSearchHit {
    pub session_key:     String,
    pub agent_id:        String,
    pub topic:           Option<String>,
    pub summary:         String,            // ≤ 1500 chars
    pub evidence_quotes: Vec<String>,       // 0..=2 raw FTS5 snippets, ≤ 200 chars each
    pub timestamp:       i64,
    pub source:          SummarySource,
}

pub enum SummarySource {
    Compactor,    // existing d0/d1/d2 fact reused
    SessionEnd,   // produced by on_session_end hook
    Lazy,         // synthesized at query time as fallback
}
```

`SessionSearchOutput { query, hits, total_hits }` is unchanged.

The single consumer is the LLM via tool calling — the JSON Schema is
regenerated automatically by `schemars` and the LLM sees the new shape on
first call after deploy. There are no programmatic Rust callers besides
existing tests.

### 3.3 Per-session result diversification

After HybridAssembler returns top-K candidate facts:

1. Group by `session_key`.
2. Within each group keep the highest-scoring fact only.
3. `max_results` counts unique sessions, not facts.
4. Result: a long session that produced 10 matching d0 chunks contributes
   exactly 1 hit, with the best-ranked chunk's text as `summary`.

### 3.4 Token budget per response

- Per hit: ~1500 char summary + 2 × 200 char evidence ≈ 1900 chars (~480
  tokens).
- Default `max_results = 5` → response ≤ 9 500 chars (~2 400 tokens).
- Summary text exceeding 1 500 chars is truncated with a trailing ellipsis.
- No "long mode" parameter in v1 (YAGNI).

---

## 4. Data Flow & Lifecycle

### 4.1 Three write paths

**Path 1 — Compactor inline (already shipped, untouched).**

```
agent loop runs → context window approaches budget →
  compactor triggers → chunk_messages() → build_summary_prompt(d0) →
  LLM call → summary_to_fact() → memory_store.write_fact()

path:    aleph://session/{sid}/d{depth}/{seq}
trigger: context_window pressure (compactor::trigger)
owner:   src/memory/session_compactor/   (Spec B does not modify)
```

**Path 2 — Session-end fallback (new).**

```
session truly ends → on_session_end hook fires →
  SessionEndSummarizer.produce(session_id, agent_id):
    0. Short-circuit: if /end-summary fact already exists for this
       session (lazy path beat us to it), skip everything.
    1. Query memory_facts for existing aleph://session/{sid}/d* facts.
    2. If any exist → copy the highest-depth fact's content into
       /end-summary (no LLM call).
    3. If none → load full raw transcript →
       build_summary_prompt(0) → LLM call → write /end-summary fact.
    4. memory_store.write_fact( path = /end-summary, layer = L1Overview )
       with INSERT OR IGNORE.

trigger: gateway::session_manager::ops::emit_session_end_raw_with_registry
owner:   src/memory/session_search_summary/end_hook.rs   (new)
```

**Path 3 — Lazy on-read fallback (new).**

```
LLM calls session_search(query) → match hits session S →
  retrieve_summary_fact(S) returns None →
    SummarySynthesizer.lazy_for(S, current_hits):
      load a windowed slice of session S's raw transcript:
        - upper bound: last 8 000 tokens of the session, OR
        - last 50 turns, whichever is smaller
        - this is enough context for build_summary_prompt(0) without
          unbounded transcript loads on huge sessions
      build_summary_prompt(0) → LLM call →
      write_fact(path = /end-summary, layer = L1Overview)
        with INSERT OR IGNORE  -- second concurrent caller skips and re-reads
    inject the fresh summary into this tool response.

trigger: inside session_search tool when summary lookup misses
owner:   src/memory/session_search_summary/synthesizer.rs   (new)
```

### 4.2 Read path (one tool call)

```
LLM calls session_search(query, max_results=5)
   │
   ├─ ① Primary retrieval:
   │      HybridAssembler.assemble_with_filter(
   │          query,
   │          FactSourceFilter::Only(SessionCompressed),
   │          top_k = max_results × 3)
   │      → up to 15 candidate facts mixing d0/d1/d2/end-summary
   │
   ├─ ② Per-session dedup (§3.3) → ≤ max_results survivors
   │
   ├─ ③ Evidence lookup: for each surviving session_key
   │      session_store.search_messages(query, fetch_limit=4, filter=session_key)
   │      → 1-2 best raw snippets as evidence_quotes
   │
   ├─ ④ Lazy fallback: if a candidate session_key has no summary fact
   │      (session is in-flight, on_session_end has not fired yet) →
   │      synthesize and inject (§4.1 path 3)
   │
   ├─ ⑤ A2A filter: drop any session_key whose owning agent_id is not
   │      reachable from caller_agent_id (existing is_accessible() check)
   │
   └─ ⑥ Build SessionSearchOutput { hits: [{summary, evidence_quotes,
        source, ...}], ... }
```

### 4.3 Concurrency & idempotence

- Multiple in-flight queries hit the same un-summarized session →
  each lazy synthesis attempt does `INSERT OR IGNORE` followed by a
  read-back. The losing call discards its in-memory summary and uses
  the winner's. A small race window can produce two LLM calls — the
  cost is acceptable given the rarity.
- session_end hook and lazy path can collide on the same path →
  whichever writes first wins, the second `INSERT OR IGNORE` skips.
- A `/end-summary` fact, once written, is never overwritten in v1.
  v2 may introduce a "refresh" workflow; out of scope here.

### 4.4 Failure handling

| Failure | Behaviour |
|---|---|
| compactor LLM call fails | Existing handling, not Spec B's concern. |
| session_end hook LLM call fails | Log + skip; do **not** block session close. The session will be covered by lazy path on first cross-session search. |
| Lazy LLM call fails | Tool returns the hit with `summary = "[summary unavailable]"` and full `evidence_quotes`. The agent can still reason from the raw quotes. |
| HybridAssembler error | Tool returns `ToolError::Execution`; do not silently swallow. |
| `search_messages` error during evidence lookup | Hit is returned with empty `evidence_quotes`; do not drop the hit. |

---

## 5. Boundary — Notes / Wiki / Knowledge Graph

> This section is a hard constraint: Spec B must not damage the
> incrementally-maintained personal-wiki layer described in
> `docs/reference/memory/NOTES.md` and the broader Aleph note retrieval
> stack.

### 5.1 Physical isolation

| Concept | Layer | Identifier | Spec B behaviour |
|---|---|---|---|
| Wiki entity / concept pages | **notes** | `fact_source != SessionCompressed` | **read-only never** (excluded by filter) |
| Wikilinks graph | **notes** | `wikilinks` subsystem | not touched |
| Namespace / agent-axis isolation | **notes** | `namespace.rs` | not touched |
| Note retrieval API | **notes** | trait + existing impls | not modified, only an additive filter param |
| Compactor d0/d1/d2 facts | **session summary** | `aleph://session/{sid}/d*/{seq}` | already exists, not modified |
| **`/end-summary` fact** | **session summary** | `aleph://session/{sid}/end-summary` | **new producer (Spec B)** |
| Raw transcripts | **session_store** | independent of memory_facts | read-only |

### 5.2 HybridAssembler change boundary

- Add a `FactSourceFilter` enum:
  ```rust
  pub enum FactSourceFilter {
      Any,                       // default, matches today's behaviour
      Only(FactSource),          // restrict to one source
      Excluding(FactSource),     // omit one source
  }
  ```
- All existing assemble call sites continue passing nothing (or `Any`
  via default). Behaviour is byte-for-byte identical. A snapshot test
  pins this.
- Only `session_search` passes `Only(SessionCompressed)`.
- Whether note retrieval (default `Any`) sees session-summary facts mixed
  in is **today's behaviour** and unchanged. Filtering session summaries
  out of note retrieval is out of scope; if it becomes a problem it is a
  separate RFC.

### 5.3 Path namespace discipline

Existing `src/memory/context/paths.rs` reserves prefixes:

```
aleph://session/   Session temporary data
aleph://note/      Wiki note pages
aleph://entity/    Knowledge-graph entities
aleph://...        others
```

Spec B writes only under `aleph://session/`:

- `aleph://session/{sid}/end-summary` (new)
- never writes `aleph://note/`, `aleph://entity/`, or any new prefix.

### 5.4 Failure cannot leak

- session_end hook failure → only logs; session_manager flow continues.
- Lazy synthesis failure → tool response degrades gracefully with raw
  evidence_quotes; does not surface as a tool error.
- Memory-store write failure → existing transactional retry/lock is
  inherited; Spec B does not bypass it.

### 5.5 Compliance with R8 / R10 / R11

- **R8 (LLM sovereignty)**: summary content is fully LLM-produced
  (existing `build_summary_prompt`). Code does scheduling, parsing,
  writes only. Per-session dedup is grouping + sort, not "judge which
  session is more relevant" (the reranker handles that).
- **R10 (Intelligence in the prompt)**: summary quality lives in the
  existing leaf/d1/d2 prompt templates, not in code heuristics.
- **R11 (Thin Harness)**: new code only schedules and coordinates.
  No intent classification, no "should I synthesize now?" decision tree —
  the only rule is "summary fact missing → lazy".

---

## 6. Acceptance Criteria

Spec B is complete when **all** of the following hold:

1. LLM calling `session_search(query)` receives hits where every entry has
   `summary`, `evidence_quotes`, and `source` populated.
2. A long session that has triggered the compactor reuses its existing
   `d0/d1/d2` facts as the summary source — zero extra LLM calls during
   search.
3. A short session (no compactor history): either
   (a) `on_session_end` fired → summary pre-generated and served from
       cache, or
   (b) `on_session_end` not yet fired → search-time lazy path synthesizes
       and writes the summary, served in this same tool response.
4. A session appears at most once in any single search response (per-session
   dedup).
5. Existing HybridAssembler usage (note retrieval, memory queries) is
   unaffected — pinned by a snapshot test against pre-Spec-B output.
6. A2A policy still filters cross-agent inaccessible sessions out of the
   results.
7. No fact whose `fact_source != SessionCompressed` ever appears in a
   `session_search` response.
8. A summary-synthesis failure degrades the hit (placeholder summary +
   evidence_quotes) rather than failing the tool call.
9. Manual smoke: with a real running server, three sessions seeded
   (one long-compacted, one short-ended, one short-in-flight), cross-session
   `session_search` results are subjectively higher quality than the
   pre-Spec-B raw FTS5 hits.

### Performance targets

- `session_search` P95 latency without lazy path: ≤ 200 ms (current
  ≈ 50 ms; the budget covers HybridAssembler + per-hit `search_messages`).
- Lazy synthesis P95 added latency: ≤ 5 s (matches compactor LLM budget).
- `on_session_end` hook is async and never blocks session close.

---

## 7. Test Strategy

### 7.1 Unit tests (in-module `#[cfg(test)]`)

| Module | Tests |
|---|---|
| `session_search_summary::end_hook` | hook fires → `/end-summary` fact written; idempotent on repeat fire; reuses existing d2 fact when present (no LLM call) |
| `session_search_summary::synthesizer` | lazy success path writes back; lazy failure returns placeholder summary; concurrent same-session calls produce 1 LLM call (mock counter) |
| `session_search_summary::dedup` | group-by-session keeps top score; max_results counts sessions; mixed d0/d1/end-summary collapses to one per session |
| `assembler::FactSourceFilter` | `Only(SessionCompressed)` excludes notes; `Any` is byte-for-byte identical to pre-change behaviour (snapshot) |
| `builtin_tools::session_search` | new schema fields present; old `content`/`role` absent; A2A filter still applies |

### 7.2 Integration tests (`tests/spec_b_e2e.rs`)

1. `fresh_short_session_lazy_synthesis` — session with raw transcripts but
   no d* facts. First search triggers lazy + writes back. Second search
   hits cache (LLM mock counter == 1).
2. `compactor_session_uses_compressed_facts` — session with d0/d1 facts.
   `source = Compactor`; no LLM call during search.
3. `session_end_hook_produces_summary` — fire `on_session_end`; verify
   `/end-summary` fact exists; subsequent search has `source = SessionEnd`.
4. `per_session_dedup` — one session with 5 matching d0 chunks; search
   `max_results = 10` returns exactly 1 hit for that session.
5. `a2a_filter_preserved` — caller A cannot reach agent B's session;
   summary fact for B's session does not appear in A's search results.
6. `note_retrieval_unchanged` — note retrieval (default `Any` filter)
   produces output identical to a snapshot taken before Spec B.

### 7.3 Property tests (`proptest`)

- Any candidate-fact set + any query → per-session dedup yields a result
  set where each `session_key` appears ≤ 1 time.
- `evidence_quotes` cardinality ∈ [0, 2]; each quote ≤ 200 chars.
- `summary` length ≤ 1 500 chars (truncation correctness).

---

## 8. Migration

### 8.1 Breaking-change inventory

- `SessionSearchHit::content` removed (split into `summary` +
  `evidence_quotes`).
- `SessionSearchHit::role` removed (no per-message role at session-summary
  granularity).
- `SessionSearchHit::source` added.
- `SessionSearchOutput` top-level fields unchanged.

### 8.2 Caller audit

- Single consumer: built-in tool calling by the LLM. The JSON Schema is
  regenerated by `schemars`; the LLM sees the new shape on first call.
- 2 test sites in `builtin_tools::session_search::tests` need updates.
- No other Rust code depends on the removed fields (verified by `grep`
  pre-implementation).

### 8.3 Cleanup actions in the same PR

- Delete the `content` and `role` fields and their references from tests.
- Update the system prompt (in `default_agents`) to teach the LLM the new
  response shape (`summary` + `evidence_quotes` semantics, when each is
  authoritative).

### 8.4 Rollback strategy

- Spec B lives in a new module + an additive HybridAssembler param.
  Reverting the PR cleanly removes all behaviour.
- `/end-summary` facts written under Spec B remain in `memory_facts` after
  revert. They are inert (no production consumer references them) and do
  not pollute note retrieval — verified because note retrieval today
  filters out the `aleph://session/` namespace from its display path
  (this assumption is rechecked in implementation Task 1).

---

## 9. Out of Scope (Explicit YAGNI)

| Tempting | Why we are not doing it |
|---|---|
| Idle pre-fetch lazy-synthesis for the N most recent unsummarized sessions | Lazy path frequency unknown until production. Add background pre-generation only if data shows it helps. |
| User-triggered "refresh this session's summary" | Path-immutable in v1 keeps the model simple and matches "summary is a frozen historical snapshot" semantics. |
| Cross-encoder rerank specific to summary facts | HybridAssembler's existing reranker is sufficient until proven otherwise. |
| Cross-agent summary federation (translate session B's summary for agent A) | Touches A2A policy core. A2A simply blocks today; that is correct for v1. |
| Multi-length summary fields (`summary_short` / `summary_long`) | Schema bloat; one summary suffices. |
| Topic / agent / time filters on `session_search` | v2; first ship the basic query path. |
| Dedicated FTS5 index for session summaries (bypassing HybridAssembler) | Conflicts with §3.2 and §5.2; revisit only if the assembler becomes a measured bottleneck. |
| `SummarySource`-weighted ranking (e.g. SessionEnd > Compactor > Lazy) | Hard-coded weighting violates R8; rely on the reranker. |

---

## 10. Open Questions (Implementation-Time, not Design)

1. **Does `HybridAssembler` already accept a `fact_source` filter?**
   The first implementation step is to grep for it. If not, a small
   additive enum parameter lands as Task 1.
2. **`on_session_end` coverage.** Spec 1 shipped the registration API.
   Implementation must verify that all session-termination paths
   (graceful close, timeout, crash recovery, user explicit close) fire
   it. Where they do not, the lazy path covers; this is a soft
   prioritization.
3. **Per-session dedup home — assembler vs tool?** Lean toward inside the
   tool (assembler stays generic). Decide during implementation based on
   what's easiest given current assembler API surface.
4. **LLM mock infrastructure for concurrency tests.** The compactor tests
   already have a mock; reuse it.
5. **Lazy synthesis input window cap.** Spec sets 8 000 tokens / 50 turns
   in §4.1 Path 3. The exact numbers are placeholder defaults — implementer
   confirms with measured prompt sizes during the first few real sessions
   and tunes once if needed.

These are intentionally not resolved in design — locking them down
forces premature specifics.

---

## 11. Relationship to Spec C (Cross-process Safety)

- Spec B reuses existing `memory_facts` write paths, which already handle
  multi-process consistency through SQLite's locking. No new concurrency
  surface is introduced.
- Spec B accepts a tiny race window where two processes lazy-synthesize
  the same session and one's LLM call is wasted. Tightening this is
  Spec C's territory.
- **Spec B is independently shippable.** Spec C's eventual landing
  benefits Spec B automatically with no rework.

---

## 12. Post-launch Signals

Operational metrics to watch (not part of v1 implementation, recorded
here for the operations team):

- Lazy-path trigger rate over time (should trend down as more sessions
  pick up `/end-summary` via session_end).
- LLM call budget for lazy + session_end combined (target: ≤ 20 % of
  current compactor LLM-call volume).
- Subjective recall quality vs pre-Spec-B baseline (manual eval; no
  metric proxy in v1).

---

## 13. Status

- **State**: draft (this document).
- **Next step**: invoke `superpowers:writing-plans` skill to produce the
  task-by-task implementation plan.
- **Pre-requisite**: user approval of this spec.
- **Roadmap link**: roadmap row updated post-implementation, mirroring
  Spec A's pattern.
