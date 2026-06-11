# Real-time Memory — Design Spec

**Date:** 2026-06-11
**Status:** Approved (brainstorming) → ready for writing-plans
**Scope:** Memory subsystem (`src/memory/`), gateway session lifecycle, one
prompt layer. **No `src/harness/` changes (R10-safe).**

---

## 0. Thesis

Claude Code updates its memory **the instant** work concludes — "merged
a11cc2211, memory updated" / "that error was because X, storing the lesson now."
Aleph defers the equivalent steps to turn-thresholds, idle timeouts, and the
nightly dream cycle. A user often finishes one session and immediately opens a
related one; if memory isn't consolidated in time, recall can't help. This spec
makes memory consolidation **real-time** across three pillars that share one
foundation.

| Pillar | What | Current gap |
|---|---|---|
| **1. Keyword linking** | Notes link to related notes via shared keywords/entities | Linking is embedding-gated → 17 notes / 0 links live |
| **2. Session-end flush** | On session end, compress + link immediately | Compression waits for 20-turn / idle; linking waits for nightly dream |
| **3. Error → lesson** | On any error, immediately persist a lesson note | Self-found errors wait for session-end reflection |

---

## PILLAR 1 — Keyword Linking

### 1.1 Problem
The linking path is gated on `related` being non-empty, and `related` comes
solely from `gather_related()` — an **embedding-based** hybrid search. When the
embedding endpoint is unreachable (a documented recurring incident)
`related = ∅` → every `Create` is linkless → no links. Live DB confirms: agent
`main` has **17 notes / 0 links / 0 `[[ ]]` in bodies** (100% orphans), even
though the running `.app` binary (2026-06-11 13:25) postdates the link-weaving
merge `18fea96d7` (2026-06-10 21:38). So it's not a stale binary — the logic
genuinely produces nothing. The obvious clusters (geopolitics 5-way, Dreame
2-way) sit unlinked.

### 1.2 Decisions (locked)
1. **Mechanism:** keyword/entity primary + embedding supplement (like
   Understand-Anything: deterministic where possible, LLM for semantics).
2. **Link signal:** LLM extracts an entity/keyword set per note; sets that
   overlap past a threshold get linked.
3. **Delivery:** one engine, two entrypoints (creation path + reworked
   NoteWeave dream stage).

### 1.3 Engine — `KeywordLinker` (`src/memory/notes/`)
- **Extraction (LLM — R7/R9):** for a batch of notes, the LLM extracts one set
  of entity/topic keywords per note (3–6 each, from title/summary/facts, not
  limited to `tags`). One batched call, not per-note.
- **Keyword persistence:** write the set into note frontmatter as a new
  `keywords:` field (human-readable, no schema migration). Creating a new note
  then only requires extracting the new note's keywords and comparing against
  stored sets — no re-extraction of the corpus.
- **Pairing (deterministic code):** two notes link when their keyword sets
  overlap past a threshold — share ≥1 specific entity OR ≥2 generic keywords
  (tunable).
- **Embedding supplement (optional second gate):** borderline pairs get a
  cosine tiebreak; if embedding is unavailable the step is skipped and the
  pure-keyword result stands (P7 graceful degradation).
- **Relation label:** `notes_links.relation` is filled with the connecting
  keyword/entity (canvas edges carry semantics, richer than bare "related").

### 1.4 Entrypoint A — Creation path (`ingestor.rs`)
Rework `enforce_link_contract` from embedding-gated to **keyword-first**:
candidate source no longer relies solely on `gather_related`. Embedding
available → still used; embedding down → pull candidates via keyword / `notes_fts`
(local). New note's keywords → overlap → `Link` op / merged into `Create.links`.
Still routes through `RefTable` anti-hallucination, bidirectional `notes_links`
writes, and `[[ ]]` body writes.

### 1.5 Entrypoint B — NoteWeave dream stage (`src/memory/dreaming/stages/note_weave.rs`)
Rework the current 409-line embedding-orphan detector into **keyword-overlap
relinking** reusing the same `KeywordLinker` (DRY). This is also the backfill
engine.

### 1.6 Backfill the live 17
`try_run_now()` exists; the gateway `dreaming` handler exposes an RPC to force a
dream cycle. Flow: implement → rebuild `aleph-server` → swap the `.app` binary
and relaunch → force one dream cycle → `notes_links` populated.

### 1.7 Canvas verification (zero panel change)
Confirmed: `graph.query → get_graph_data` sources edges directly from
`notes_links`. Populate links → the canvas renders edges automatically.
Playwright (bootstrap-url auth) opens panel memory/graph mode and screenshots
the two clusters.

---

## PILLAR 2 — Real-time Session-end Flush

### 2.1 Problem
Compression (raw→notes) triggers on a 20-turn threshold / idle timeout / an
immediate correction signal (`execute.rs:601`). A short session (<20 turns, no
correction) ends **without** compressing its raws into notes. The session-end
hook (`emit.rs:68`) only writes an `/end-summary` raw — it does not force
compression or linking. So a back-to-back follow-on session can't recall the
prior one's content as consolidated, linked notes.

### 2.2 Mechanism — async with readiness gate (locked)
On session conclude, **spawn** an immediate flush for that agent:
`compress_to_notes` (drain pending raws → notes) + `KeywordLinker` (link the new
notes). Non-blocking to the user.

Add a per-agent **flush-state registry** (mirrors the existing session-keyed
`scratchpad_registry` precedent): records `flush_in_progress` / `last_flushed`.
When a new session starts (agent_init / first turn), it checks the registry; if
the previous session's flush is still running, it **awaits it briefly (bounded)**
before assembling context — so a fast follow-on session sees consolidated
memory, while a normal session never blocks. Lives in memory subsystem +
gateway session lifecycle. **No harness change (R10).**

---

## PILLAR 3 — Error → Immediate Lesson Capture

### 3.1 Problem
- **User correction** ("wrong"/"no"/"actually"…): `signal_detector` →
  `CompressionSignal::Correction` → `Immediate` compress. ✅ already real-time,
  but produces a generic note, not a structured lesson.
- **Session-level lessons:** `SessionReflector` distills lessons at **session
  end** (one LLM call, debounced) → `feedback/lessons` notes.
- **AI self-discovered error mid-task:** **no mechanism** — waits for
  session-end reflection. ❌ not real-time.

### 3.2 Mechanism — prompt-empowered (locked, R7/R8/R9)
No error-detector middleware (that would replace LLM judgment with code → R7
violation). Instead, extend `MemoryProtocolLayer`
(`src/thinker/layers/memory_protocol.rs`, priority 1745) with a soft nudge:
**when the model recognizes an error — its own or one the user corrected — it
immediately calls `note_manage` to write a `feedback/lessons` note containing
the error cause (why) and how to avoid it (how-to-apply).** `note_manage` create
is a direct, immediate write and surfaces related notes; with Pillar 1 the
lesson note auto-links into the relevant cluster. This is the Aleph version of
Claude Code's "storing this lesson now" — zero new middleware, zero extra
reasoning tax, gets stronger as the model does (R10 future-proof).

---

## Success criteria (verifiable)

**Pillar 1**
- ✅ The 17 notes' clusters (geopolitics 5-way, Dreame 2-way) have rows in
  `notes_links` and `[[ ]]` in bodies.
- ✅ Creation path still links when **embedding is unavailable** (integration
  test simulating embedding-down).
- ✅ `KeywordLinker` overlap-algorithm unit tests + NoteWeave keyword-relink test.
- ✅ Canvas screenshot shows the connecting edges.

**Pillar 2**
- ✅ A short (<20-turn) session, on conclude, flushes its raws into linked notes
  without a dream cycle (test).
- ✅ Readiness gate: a follow-on session awaits an in-progress flush within a
  bounded window (test).

**Pillar 3**
- ✅ `MemoryProtocolLayer` emits the lesson-capture nudge (prompt test).
- ✅ A lesson note written via `note_manage` mid-session is immediately
  recallable and linked (integration test).

## Decisions taken on the user's behalf (overridable)
1. Keywords stored in frontmatter `keywords:` (not a new table).
2. `relation` filled with the connecting keyword (not bare "related").
3. Threshold ≥1 specific entity / ≥2 generic keywords.
4. Flush-state registry is per-agent in-memory (mirrors `scratchpad_registry`).

## Out of scope
- No panel/canvas code changes (edges already render from `notes_links`).
- No `src/harness/` changes (R10).
- No change to the embedding pipeline itself; it becomes a supplement, not a
  dependency.
- No deterministic "error detector" — error recognition stays with the LLM (R7).
