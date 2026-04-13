---
title: "Memory Evolution Spec 1: Capture Hooks"
date: 2026-04-13
status: approved
parent: docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md
related_refs:
  - docs/reference/memory/NOTES.md
  - docs/reference/memory/RAW_MEMORY.md
  - docs/reference/memory/RETRIEVAL.md
---

# Spec 1: Memory Capture Hooks

Plug three information-loss boundaries in Aleph's memory flow by adding three new **producers** to the existing `raw_memories → CompressionService → notes` pipeline. No new abstractions — only new enum variants, specialised extraction prompts, and trigger calls at the right seams.

---

## 1. Problem

Three boundaries in the current runtime silently lose information:

| ID | Boundary | Current behaviour | Loss |
|----|----------|-------------------|------|
| G1 | `session_compactor` replaces old messages with a summary | Raw chunks are preserved for `recall_context` semantic search, but **no LLM fact extraction** fires at compression time. The `CompressionService` runs on its own idle/turn schedule, decoupled from compaction events. | Decisions/preferences/facts that lived in the soon-to-be-dropped turns may never reach `notes`. They remain "searchable" in raw form but are never promoted to structured knowledge. |
| G2 | A sub-agent finishes and returns to its parent | Transcripts are stored in `subagent-transcript/` note category, but no extraction pipeline runs against them. | Lessons learned by the sub-agent (tool usage, failure modes, domain findings) never become long-term knowledge for the parent. |
| G3 | A conversation actually ends (gateway disconnect **A** or LLM-signalled task completion **C**) | Aleph has no explicit "session boundary" hook distinct from idle/turn compression triggers. | End-of-session digests (user preferences, unfinished tasks, project-state snapshots) are never written. |

---

## 2. Non-goals

- Not porting hermes-agent's `MemoryProvider` ABC — Aleph's memory modules are already decomposed. Pluggable backends are Spec 4, YAGNI-gated.
- No `reflect` / synthesis operation — Spec 2.
- No `<memory-context>` fencing or injection modes — Spec 3.
- No `on_memory_write` mirror hook — needed only when external backends exist (Spec 4).
- No pre-fetch queue redesign — Aleph's assembler hydration solves the same problem differently.

---

## 3. Architecture

### 3.1 Data flow

```
PRODUCERS (new)                                    CONSUMER (existing, extended)
───────────────                                    ───────────────────────────────

G1  session_compactor::compact_chunk              ┌─────────────────────────────┐
    ─before summarise─► raw_memories(PreCompress) │                             │
                                                  │                             │
G2  multi_agent::on_subagent_complete             │  CompressionService         │
    ────────────────► raw_memories(Delegation{..})│    ::process_batch(source)  │
                                                  │                             │
G3a gateway::session::on_close / on_timeout       │  source-specialised prompt: │
    ────────────────► raw_memories(               │    PreCompress → RESCUE     │
                        SessionEnd{Disconnect})   │    Delegation  → LESSON     │
                                                  │    SessionEnd/Disconnect    │
G3c tool::session_complete (new)                  │                → DIGEST     │
    ────────────────► raw_memories(               │    SessionEnd/TaskDone      │
                        SessionEnd{TaskDone})     │                → RETRO      │
                                                  │                             │
                                                  │  → NoteUpdate[]             │
                                                  │  → NoteIndexer::{write,     │
                                                  │                  append}    │
                                                  └─────────────────────────────┘
```

### 3.2 Key invariants

- **Producers never call LLMs.** They persist raw content to `raw_memories`, return immediately, and let `CompressionService` (already async) perform extraction.
- **Existing `raw_memories` sources stay backward-compatible.** `SessionCompressed | Transcript | ToolOutput | Attachment` continue to use `PROMPT_GENERIC`. Only the three new variants route to specialised prompts.
- **Existing `session_compactor::store_raw_chunk` (the `aleph://session/.../raw/` index) is retained.** It serves `recall_context` search; it is a different concern from pre-compression extraction.

---

## 4. Data model changes

### 4.1 `RawMemorySource` (additive)

`src/memory/store/raw_memory.rs`:

```rust
pub enum RawMemorySource {
    // existing
    SessionCompressed,
    Transcript,
    ToolOutput,
    Attachment,

    // new — Spec 1
    PreCompress,
    Delegation { child_agent_id: String },
    SessionEnd { reason: SessionEndReason },
}

pub enum SessionEndReason {
    Disconnect, // gateway close / idle timeout
    TaskDone,   // LLM called the `session_complete` tool
}
```

### 4.2 SQLite schema

Add a nullable `source_detail TEXT` column (JSON payload for enum variants carrying data). `init_schema` performs an idempotent `ALTER TABLE ... ADD COLUMN` inside a migration helper; re-runs are no-ops. `RawMemorySource::from_str` and `::as_str` are updated to round-trip the new variants via `source` + optional `source_detail` parsing.

Existing rows (all carrying simple variants without payload) keep `source_detail = NULL` and round-trip unchanged.

---

## 5. Trigger points

| Hook | Trigger file (expected) | Insertion |
|------|-------------------------|-----------|
| G1 pre_compress | `src/memory/session_compactor/mod.rs` (around `compact_chunk` / before the summarisation call) | Before the chunk is compressed, build a single `raw_memories(PreCompress)` row containing the verbatim role-prefixed text of the chunk. Then run existing summary logic unchanged. |
| G2 delegation | `src/multi_agent/…` (sub-agent completion path, exact file confirmed during implementation) | On sub-agent finish, emit `raw_memories(Delegation { child_agent_id })` with payload: delegation prompt + sub-agent final output + agent_ids. |
| G3-A disconnect | `src/gateway/session/…` (`on_close` / idle-timeout handler) | When a session closes, collect the untracked tail of the conversation and emit `raw_memories(SessionEnd { Disconnect })`. |
| G3-C task_done | New `src/builtin_tools/session_complete.rs` | Tool handler writes `raw_memories(SessionEnd { TaskDone })` with the `outcome` string + tail of recent messages, returns `{"ok": true}` immediately. Does NOT terminate the session. |

**All four producers are synchronous-write / async-extract.** Each writes exactly one `raw_memories` row, then returns. `CompressionService` consumes rows on its normal schedule (extended below to honour the new sources).

---

## 6. Extraction prompts

`src/memory/compression/extractor.rs` gains per-source dispatch. Prompt bodies will be refined in the writing-plans phase; intent:

| Source variant | Prompt focus |
|----------------|--------------|
| `PreCompress` | **RESCUE.** "This batch is about to be dropped from live context. Extract decisions, user preferences, unfinished tasks, important facts. Err on over-extraction." |
| `Delegation { .. }` | **LESSON.** "Parent agent delegated a task; sub-agent returned a result. What is worth promoting to the parent's long-term knowledge? Tool usage patterns, failure modes, domain findings." |
| `SessionEnd { Disconnect }` | **DIGEST.** "Session ended (disconnect/timeout). Distill user preferences (`preference`), project progress (`project`), unfinished items (`plan`)." |
| `SessionEnd { TaskDone }` | **RETROSPECTIVE.** "LLM marked this task complete. Retro: what went right? What would the next similar task benefit from? Favour `lesson` category." |
| anything else (existing variants) | `PROMPT_GENERIC` — unchanged, backward-compatible. |

Category names appearing in prompts are **hints**, not contracts — the LLM still selects the final category.

---

## 7. `session_complete` tool

New tool registered under `src/builtin_tools/session_complete.rs`:

```rust
#[derive(JsonSchema, Deserialize)]
pub struct SessionCompleteArgs {
    pub outcome: String,
    pub key_learnings: Option<Vec<String>>,
}
```

Tool description (exposed to LLM): "Call when you believe a self-contained task has just completed. Triggers a memory retrospective of the task. Does not end the conversation."

Behaviour:
1. Collect the tail of the current conversation (bounded by a configurable window, e.g. last N turns or last task boundary).
2. Write `raw_memories(SessionEnd { TaskDone }, source_detail = outcome + learnings)`.
3. Return `{"ok": true}` so the LLM can continue.

This is the R8 **LLM-sovereignty** path for G3-C: the model decides when a task is done, not a heuristic. G3-A disconnect is the fallback when the LLM forgets.

---

## 8. Cleanup (prevent shit-pile accumulation)

Every subsystem touched gets reviewed for dead/overlapping code. Tentative list (confirmed in plan phase):

| Current code | Disposition |
|--------------|-------------|
| `session_compactor::store_raw_chunk` (stores `aleph://session/.../raw/{seq}`) | **Keep.** Serves `recall_context`; orthogonal to new hook. |
| `compression::signal_detector` and `compression::trigger` | **Audit.** If they already handle the new sources via existing logic, no change. If they duplicate what the `CompressionService` batch loop now does, remove duplication. |
| Any ad-hoc sub-agent-result handling elsewhere in `multi_agent/` | **Consolidate** through the new `on_subagent_complete` emission point; delete scattered one-offs. |
| `compression::extractor` single-prompt assumption | **Fork the dispatch**, keep old prompt as `PROMPT_GENERIC` for backward-compat, add four specialised prompts. |

---

## 9. Testing strategy

- **Unit**: each producer — given an input, asserts one `raw_memories` row with the correct source + detail is persisted.
- **Integration**: end-to-end — fire a hook, let `CompressionService::process_batch` run, assert expected `notes_index` + `notes_fts` + (optionally) `notes_vec_*` rows appear.
- **Prompt snapshots**: a snapshot file per `PROMPT_*` constant so accidental changes are flagged.
- **Concurrency (`loom_concurrency.rs`)**: new case "compaction trigger and `session_complete` land in the same tick" — no double-write, no dropped row.
- **Migration**: re-run `init_schema` on a DB that already has `raw_memories` — idempotent, no data loss.

---

## 10. Compliance with architectural redlines

| Redline | Check |
|---------|-------|
| R3 Core minimalism | No new heavy deps; only enum variants + prompts + table column. |
| R8 LLM sovereignty | All "should this be extracted?" / "which category?" decisions stay in the LLM. Code only routes. |
| R9 Everything is a tool | `session_complete` added as a tool for LLM-visible task-boundary signalling. |
| R10 Intelligence in the prompt | Four specialised system prompts carry the semantic difference between sources. |

No redline violated. No conflict with design principles P1–P8.

---

## 11. Open questions (to resolve in plan phase)

- Exact byte/token budget of the "tail of conversation" snapshotted by G3-A and G3-C — configurable or fixed?
- `CompressionService` priority: should `PreCompress` rows jump the queue ahead of `Transcript`? Likely yes, since `PreCompress` carries time-sensitive semantics.
- Deduplication: if G1 emits `PreCompress` and then idle-compression would later have batched the same underlying content, do we need a dedup key on `raw_memories`?
- Exact file location for G2 trigger inside `multi_agent/` — confirmed during plan-phase code exploration.
